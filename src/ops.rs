//! Bounded parallel map over a set of repositories — the reuse primitive every
//! bulk command (`status`, `pull`, `fetch`, …) is built on.
//!
//! [`for_each_repo`] runs a closure against many repos on a fixed-size pool of
//! worker threads and returns one [`Outcome`] per input, **in input order**.
//! Each repo is given its own timeout, a panic or error in the closure is
//! captured rather than propagated, and one hung repo cannot stall the batch.

use std::any::Any;
use std::panic::{self, AssertUnwindSafe};
use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

/// The result of running the mapped closure against a single repository.
#[derive(Debug, Clone)]
pub struct Outcome<T> {
    /// The repository this outcome belongs to (as passed in).
    pub repo: PathBuf,
    /// `Ok(value)` on success; `Err(message)` on error, panic or timeout.
    pub result: Result<T, String>,
}

/// The result of running the mapped closure against a single work item — the
/// generalisation of [`Outcome`] to any `Send + Clone` input, not just a repo
/// path.
#[derive(Debug, Clone)]
pub struct Done<W, T> {
    /// The work item this outcome belongs to (as passed in).
    pub item: W,
    /// `Ok(value)` on success; `Err(message)` on error, panic or timeout.
    pub result: Result<T, String>,
}

/// Run `f` against every path in `repos` on a bounded pool of `max(1, jobs)`
/// worker threads.
///
/// * **Ordering** — the returned vec is 1:1 with `repos` and in the same order,
///   regardless of completion order.
/// * **Per-repo timeout** — each call to `f` runs on its own thread and is
///   awaited with [`mpsc::Receiver::recv_timeout`]. If it does not finish within
///   `timeout` the outcome is `Err("timed out after {timeout:?}")` and the pool
///   moves on. The abandoned thread keeps running `f` in the background; this is
///   acceptable because every git subprocess fussy-git spawns sets
///   `GIT_TERMINAL_PROMPT=0`, so git cannot block forever waiting on a
///   credential prompt and the thread will exit on its own.
/// * **Panics and errors** — a panic inside `f` is caught
///   ([`std::panic::catch_unwind`]) and an `Err(_)` is stringified; either way
///   the batch continues.
pub fn for_each_repo<T, F>(
    repos: Vec<PathBuf>,
    jobs: usize,
    timeout: Duration,
    f: F,
) -> Vec<Outcome<T>>
where
    F: Fn(&Path) -> anyhow::Result<T> + Send + Sync + 'static,
    T: Send + 'static,
{
    let f = Arc::new(f);
    for_each(repos, jobs, timeout, move |p: &PathBuf| f(p.as_path()))
        .into_iter()
        .map(|Done { item, result }| Outcome { repo: item, result })
        .collect()
}

/// The same bounded parallel map as [`for_each_repo`], but over arbitrary
/// `Send + Clone` work items rather than repository paths — used to clone a set
/// of missing repositories for `sync`. Ordering, the per-item timeout, and the
/// panic / error capture are identical.
pub fn for_each<W, T, F>(items: Vec<W>, jobs: usize, timeout: Duration, f: F) -> Vec<Done<W, T>>
where
    W: Send + Clone + 'static,
    F: Fn(&W) -> anyhow::Result<T> + Send + Sync + 'static,
    T: Send + 'static,
{
    let total = items.len();
    if total == 0 {
        return Vec::new();
    }

    type Slots<W, T> = Arc<Mutex<Vec<Option<Done<W, T>>>>>;

    let f = Arc::new(f);
    let work: Vec<(usize, W)> = items.into_iter().enumerate().collect();
    let queue = Arc::new(Mutex::new(work.into_iter()));
    let slots: Slots<W, T> = Arc::new(Mutex::new((0..total).map(|_| None).collect()));

    let worker_count = jobs.max(1).min(total);
    let mut handles = Vec::with_capacity(worker_count);
    for _ in 0..worker_count {
        let queue = Arc::clone(&queue);
        let slots = Arc::clone(&slots);
        let f = Arc::clone(&f);
        handles.push(thread::spawn(move || loop {
            let next = {
                let mut guard = queue.lock().unwrap_or_else(|e| e.into_inner());
                guard.next()
            };
            let Some((idx, item)) = next else { break };
            let result = run_one(&f, &item, timeout);
            let mut guard = slots.lock().unwrap_or_else(|e| e.into_inner());
            guard[idx] = Some(Done { item, result });
        }));
    }
    for handle in handles {
        let _ = handle.join();
    }

    let slots = match Arc::try_unwrap(slots) {
        Ok(m) => m,
        Err(_) => unreachable!("every worker was joined; no other Arc holders remain"),
    };
    slots
        .into_inner()
        .unwrap_or_else(|e| e.into_inner())
        .into_iter()
        .map(|slot| slot.expect("every slot is filled by a worker"))
        .collect()
}

/// Run `f` for one work item on a dedicated thread, bounded by `timeout`.
fn run_one<W, T, F>(f: &Arc<F>, item: &W, timeout: Duration) -> Result<T, String>
where
    W: Send + Clone + 'static,
    F: Fn(&W) -> anyhow::Result<T> + Send + Sync + 'static,
    T: Send + 'static,
{
    let (tx, rx) = mpsc::channel::<Result<T, String>>();
    let f = Arc::clone(f);
    let item = item.clone();
    // Detached on purpose: on timeout the pool stops waiting, but the thread is
    // left to run to completion rather than being force-killed.
    thread::spawn(move || {
        let outcome = match panic::catch_unwind(AssertUnwindSafe(|| (*f)(&item))) {
            Ok(Ok(value)) => Ok(value),
            Ok(Err(err)) => Err(format!("{err:#}")),
            Err(payload) => Err(format!("panicked: {}", panic_message(payload))),
        };
        let _ = tx.send(outcome);
    });

    match rx.recv_timeout(timeout) {
        Ok(result) => result,
        Err(mpsc::RecvTimeoutError::Timeout) => Err(format!("timed out after {timeout:?}")),
        Err(mpsc::RecvTimeoutError::Disconnected) => {
            Err("worker thread ended without producing a result".to_string())
        }
    }
}

fn panic_message(payload: Box<dyn Any + Send>) -> String {
    if let Some(s) = payload.downcast_ref::<&str>() {
        (*s).to_string()
    } else if let Some(s) = payload.downcast_ref::<String>() {
        s.clone()
    } else {
        "unknown panic".to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Instant;

    fn paths(n: usize) -> Vec<PathBuf> {
        (0..n)
            .map(|i| PathBuf::from(format!("/repo/{i}")))
            .collect()
    }

    #[test]
    fn preserves_input_order_and_isolates_errors() {
        let repos = paths(20);
        let out = for_each_repo(repos.clone(), 4, Duration::from_secs(5), |p| {
            let name = p.file_name().unwrap().to_str().unwrap().to_string();
            if name == "7" {
                anyhow::bail!("boom at seven");
            }
            Ok(name)
        });

        assert_eq!(out.len(), 20);
        for (i, outcome) in out.iter().enumerate() {
            assert_eq!(outcome.repo, repos[i]);
            if i == 7 {
                assert!(outcome
                    .result
                    .as_ref()
                    .unwrap_err()
                    .contains("boom at seven"));
            } else {
                assert_eq!(outcome.result.as_ref().unwrap(), &format!("{i}"));
            }
        }
    }

    #[test]
    fn a_slow_repo_times_out_without_blocking_the_rest() {
        let out = for_each_repo(
            vec![PathBuf::from("/slow"), PathBuf::from("/fast")],
            2,
            Duration::from_millis(50),
            |p| {
                if p.ends_with("slow") {
                    thread::sleep(Duration::from_millis(400));
                }
                Ok(p.to_path_buf())
            },
        );

        assert_eq!(out[0].repo, PathBuf::from("/slow"));
        assert!(out[0].result.is_err());
        assert!(out[0].result.as_ref().unwrap_err().starts_with("timed out"));
        assert_eq!(out[1].result.as_ref().unwrap(), &PathBuf::from("/fast"));
    }

    #[test]
    fn work_actually_runs_concurrently() {
        let start = Instant::now();
        let out = for_each_repo(paths(4), 4, Duration::from_secs(5), |_| {
            thread::sleep(Duration::from_millis(200));
            Ok(())
        });
        let elapsed = start.elapsed();

        assert_eq!(out.len(), 4);
        assert!(out.iter().all(|o| o.result.is_ok()));
        assert!(
            elapsed < Duration::from_millis(600),
            "4x200ms sleeps with jobs=4 took {elapsed:?}; not concurrent"
        );
    }

    #[test]
    fn jobs_are_bounded() {
        // With jobs=1 the same four sleeps must serialise.
        let start = Instant::now();
        for_each_repo(paths(4), 1, Duration::from_secs(5), |_| {
            thread::sleep(Duration::from_millis(100));
            Ok(())
        });
        assert!(
            start.elapsed() >= Duration::from_millis(400),
            "jobs=1 should have serialised the work"
        );
    }

    #[test]
    fn panics_are_caught() {
        let out = for_each_repo(vec![PathBuf::from("/a")], 1, Duration::from_secs(5), |_| {
            panic!("kaboom");
            #[allow(unreachable_code)]
            Ok(())
        });
        assert!(out[0].result.as_ref().unwrap_err().contains("kaboom"));
    }

    #[test]
    fn empty_input_is_fine() {
        let out = for_each_repo(Vec::new(), 4, Duration::from_secs(1), |_| Ok(()));
        assert!(out.is_empty());
    }
}
