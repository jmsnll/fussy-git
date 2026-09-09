//! Bulk git operations across every managed repository: `status`, `pull`,
//! `fetch`.
//!
//! Each command discovers (or is handed) a set of repo paths, runs the work in
//! parallel via [`crate::ops::for_each_repo`], prints a compact human report to
//! **stdout**, and returns a process exit code (`0` when every repository
//! succeeds, `4` when at least one fails). Progress and per-repo warnings go to
//! **stderr**.

use std::path::PathBuf;
use std::time::Duration;

use anyhow::Result;

use crate::config::Config;
use crate::ops::{self, Outcome};
use crate::{git, scan, ui};

/// Per-repo timeout for the local-only `status` scan.
const STATUS_TIMEOUT: Duration = Duration::from_secs(60);
/// Per-repo timeout for network operations (`pull`, `fetch`).
const NETWORK_TIMEOUT: Duration = Duration::from_secs(300);

/// Exit code returned when one or more repos in a batch failed.
pub const EXIT_PARTIAL_FAILURE: i32 = 4;

/// Every real repository under the managed roots, sorted and de-duplicated.
///
/// Linked worktrees are excluded (they are not independent clones); everything
/// else `scan` finds is included, even repos with no or an unparseable remote.
pub fn all_repo_paths(cfg: &Config) -> Result<Vec<PathBuf>> {
    let mut paths: Vec<PathBuf> = scan::scan(cfg)?
        .into_iter()
        .filter(|d| d.class != scan::Class::LinkedWorktree)
        .map(|d| d.path)
        .collect();
    paths.sort();
    paths.dedup();
    Ok(paths)
}

// --- status ----------------------------------------------------------------

/// A one-line summary of a repository's working state.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RepoStatus {
    /// Current branch, or `None` on a detached HEAD.
    pub branch: Option<String>,
    /// Commits ahead of the upstream tracking branch (0 if none / no upstream).
    pub ahead: u32,
    /// Commits behind the upstream tracking branch.
    pub behind: u32,
    /// Working tree has staged, unstaged or untracked changes.
    pub dirty: bool,
    /// Number of stash entries.
    pub stashes: usize,
}

fn collect_statuses(repos: Vec<PathBuf>, jobs: usize) -> Vec<Outcome<RepoStatus>> {
    ops::for_each_repo(repos, jobs, STATUS_TIMEOUT, |repo| {
        let branch = git::current_branch(repo)?;
        let (ahead, behind) = git::ahead_behind(repo)?.unwrap_or((0, 0));
        let dirty = git::is_dirty(repo)?;
        let stashes = git::stash_count(repo)?;
        Ok(RepoStatus {
            branch,
            ahead,
            behind,
            dirty,
            stashes,
        })
    })
}

/// One rendered row of the status table, kept as plain strings so column widths
/// can be measured before any colour is applied.
struct StatusRow {
    name: String,
    branch: String,
    sync: String,
    /// `sync` painted yellow when behind, green when only ahead.
    sync_behind: bool,
    state: String,
    /// `state` painted red for an error, yellow otherwise.
    state_error: bool,
}

fn status_row(cfg: &Config, outcome: &Outcome<RepoStatus>) -> StatusRow {
    let name = ui::repo_name(cfg, &outcome.repo);
    match &outcome.result {
        Ok(st) => {
            let mut sync = String::new();
            if st.ahead > 0 {
                sync.push_str(&format!("+{}", st.ahead));
            }
            if st.behind > 0 {
                if !sync.is_empty() {
                    sync.push(' ');
                }
                sync.push_str(&format!("-{}", st.behind));
            }
            let mut state = Vec::new();
            if st.dirty {
                state.push("dirty".to_string());
            }
            if st.stashes > 0 {
                state.push(format!("stash {}", st.stashes));
            }
            StatusRow {
                name,
                branch: st.branch.clone().unwrap_or_else(|| "detached".to_string()),
                sync,
                sync_behind: st.behind > 0,
                state: state.join(", "),
                state_error: false,
            }
        }
        Err(_) => StatusRow {
            name,
            branch: String::new(),
            sync: String::new(),
            sync_behind: false,
            state: "error".to_string(),
            state_error: true,
        },
    }
}

/// Print a compact aligned status table and return an exit code.
pub fn status(cfg: &Config, repos: Vec<PathBuf>, jobs: usize) -> Result<i32> {
    let outcomes = collect_statuses(repos, jobs);
    for o in &outcomes {
        if let Err(err) = &o.result {
            eprintln!("{}: {}", ui::repo_name(cfg, &o.repo), first_line(err));
        }
    }
    let rows: Vec<StatusRow> = outcomes.iter().map(|o| status_row(cfg, o)).collect();

    let width = |header: &str, pick: &dyn Fn(&StatusRow) -> usize| {
        rows.iter()
            .map(pick)
            .chain(std::iter::once(header.len()))
            .max()
            .unwrap_or(0)
    };
    let name_w = width("REPO", &|r| r.name.chars().count());
    let branch_w = width("BRANCH", &|r| r.branch.chars().count());
    let sync_w = width("SYNC", &|r| r.sync.chars().count());

    println!(
        "{}",
        ui::dim(&format!(
            "{:<name_w$}  {:<branch_w$}  {:<sync_w$}  {}",
            "REPO", "BRANCH", "SYNC", "STATE"
        ))
    );

    let mut failed = 0;
    for row in &rows {
        if row.state_error {
            failed += 1;
        }
        let sync = if row.sync.is_empty() {
            " ".repeat(sync_w)
        } else {
            let painted = if row.sync_behind {
                ui::warn(&row.sync)
            } else {
                ui::ok(&row.sync)
            };
            ui::pad_to(&painted, row.sync.chars().count(), sync_w)
        };
        let state = if row.state_error {
            ui::danger(&row.state)
        } else if row.state.is_empty() {
            String::new()
        } else {
            ui::warn(&row.state)
        };
        let line = format!(
            "{:<name_w$}  {:<branch_w$}  {}  {}",
            row.name, row.branch, sync, state
        );
        println!("{}", line.trim_end());
    }

    Ok(if failed > 0 { EXIT_PARTIAL_FAILURE } else { 0 })
}

// --- pull ------------------------------------------------------------------

/// Outcome of a successful `pull` against one repo.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PullOutcome {
    /// The working tree advanced.
    Updated,
    /// Nothing to do.
    UpToDate,
}

fn run_pull(repos: Vec<PathBuf>, jobs: usize) -> Vec<Outcome<PullOutcome>> {
    ops::for_each_repo(repos, jobs, NETWORK_TIMEOUT, |repo| {
        let out = git::run(Some(repo), ["pull", "--ff-only"])?;
        let text = format!(
            "{}{}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        );
        if text.contains("Already up to date") {
            Ok(PullOutcome::UpToDate)
        } else {
            Ok(PullOutcome::Updated)
        }
    })
}

/// `git pull --ff-only` across `repos`; print per-repo lines plus a summary.
pub fn pull(cfg: &Config, repos: Vec<PathBuf>, jobs: usize) -> Result<i32> {
    let outcomes = run_pull(repos, jobs);
    let name_w = name_width(cfg, &outcomes);

    let (mut updated, mut up_to_date, mut failed) = (0u32, 0u32, 0u32);
    for outcome in &outcomes {
        let name = ui::repo_name(cfg, &outcome.repo);
        match &outcome.result {
            Ok(PullOutcome::Updated) => {
                updated += 1;
                println!("{name:<name_w$}  {}", ui::ok("updated"));
            }
            Ok(PullOutcome::UpToDate) => {
                up_to_date += 1;
                println!("{name:<name_w$}  {}", ui::dim("up to date"));
            }
            Err(err) => {
                failed += 1;
                println!(
                    "{name:<name_w$}  {}  {}",
                    ui::danger("failed"),
                    ui::dim(first_line(err))
                );
            }
        }
    }

    let mut summary = vec![
        format!("{updated} updated"),
        format!("{up_to_date} up to date"),
    ];
    if failed > 0 {
        summary.push(ui::danger(&format!("{failed} failed")));
    }
    println!("\n{}", summary.join(", "));
    Ok(if failed > 0 { EXIT_PARTIAL_FAILURE } else { 0 })
}

// --- fetch ----------------------------------------------------------------

fn run_fetch(repos: Vec<PathBuf>, jobs: usize) -> Vec<Outcome<()>> {
    ops::for_each_repo(repos, jobs, NETWORK_TIMEOUT, |repo| {
        git::run(Some(repo), ["fetch", "--all", "--prune", "--quiet"])?;
        Ok(())
    })
}

/// `git fetch --all --prune` across `repos`; print per-repo lines plus a summary.
pub fn fetch(cfg: &Config, repos: Vec<PathBuf>, jobs: usize) -> Result<i32> {
    let outcomes = run_fetch(repos, jobs);
    let name_w = name_width(cfg, &outcomes);

    let (mut fetched, mut failed) = (0u32, 0u32);
    for outcome in &outcomes {
        let name = ui::repo_name(cfg, &outcome.repo);
        match &outcome.result {
            Ok(()) => {
                fetched += 1;
                println!("{name:<name_w$}  {}", ui::ok("fetched"));
            }
            Err(err) => {
                failed += 1;
                println!(
                    "{name:<name_w$}  {}  {}",
                    ui::danger("failed"),
                    ui::dim(first_line(err))
                );
            }
        }
    }

    let mut summary = vec![format!("{fetched} fetched")];
    if failed > 0 {
        summary.push(ui::danger(&format!("{failed} failed")));
    }
    println!("\n{}", summary.join(", "));
    Ok(if failed > 0 { EXIT_PARTIAL_FAILURE } else { 0 })
}

// --- helpers --------------------------------------------------------------

fn name_width<T>(cfg: &Config, outcomes: &[Outcome<T>]) -> usize {
    outcomes
        .iter()
        .map(|o| ui::repo_name(cfg, &o.repo).chars().count())
        .max()
        .unwrap_or(0)
}

fn first_line(s: &str) -> &str {
    s.lines().find(|l| !l.trim().is_empty()).unwrap_or(s).trim()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::{self, add_worktree, make_repo};
    use std::fs;
    use std::path::Path;

    struct Remote {
        _tmp: tempfile::TempDir,
        bare: PathBuf,
        scratch: PathBuf,
    }

    fn file_url(p: &Path) -> String {
        format!("file://{}", p.display())
    }

    fn set_ident(dir: &Path) {
        testutil::git(dir, &["config", "user.email", "t@example.com"]);
        testutil::git(dir, &["config", "user.name", "T"]);
        testutil::git(dir, &["config", "commit.gpgsign", "false"]);
        testutil::git(dir, &["config", "core.hooksPath", "/dev/null"]);
    }

    /// A bare repo with one commit, plus a scratch dir for helper checkouts.
    fn remote() -> Remote {
        let tmp = tempfile::tempdir().unwrap();
        let bare = tmp.path().join("origin.git");
        fs::create_dir_all(&bare).unwrap();
        testutil::git(&bare, &["init", "-q", "--bare", "-b", "main"]);

        let scratch = tmp.path().join("scratch");
        fs::create_dir_all(&scratch).unwrap();

        let seed = scratch.join("seed");
        make_repo(&seed, None);
        testutil::git(&seed, &["remote", "add", "origin", &file_url(&bare)]);
        testutil::git(&seed, &["push", "-q", "-u", "origin", "main"]);

        Remote {
            _tmp: tmp,
            bare,
            scratch,
        }
    }

    impl Remote {
        fn clone_to(&self, dst: &Path) -> PathBuf {
            testutil::git(
                &self.scratch,
                &["clone", "-q", &file_url(&self.bare), dst.to_str().unwrap()],
            );
            set_ident(dst);
            dst.to_path_buf()
        }

        /// Add one commit to the bare's `main`.
        fn advance(&self, tag: &str) {
            let w = self.scratch.join(format!("advance-{tag}"));
            testutil::git(
                &self.scratch,
                &["clone", "-q", &file_url(&self.bare), w.to_str().unwrap()],
            );
            set_ident(&w);
            fs::write(w.join(format!("{tag}.txt")), tag).unwrap();
            testutil::git(&w, &["add", "."]);
            testutil::git(&w, &["commit", "-qm", tag, "--no-verify"]);
            testutil::git(&w, &["push", "-q", "origin", "main"]);
        }
    }

    fn test_config(root: &Path) -> Config {
        let path = root.join(".fussy-git.toml");
        fs::write(&path, format!("root = {root:?}\n")).unwrap();
        Config::load_from(&path).unwrap()
    }

    #[test]
    fn all_repo_paths_lists_repos_and_skips_worktrees() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("git");
        fs::create_dir_all(&root).unwrap();
        let cfg = test_config(&root);

        let a = root.join("github.com/me/a");
        let b = root.join("github.com/me/b");
        make_repo(&a, Some("git@github.com:me/a.git"));
        make_repo(&b, Some("git@github.com:me/b.git"));
        let wt = root.join("worktrees/a-feature");
        add_worktree(&a, &wt, "feature");

        let paths = all_repo_paths(&cfg).unwrap();
        assert!(paths.contains(&a));
        assert!(paths.contains(&b));
        assert!(!paths.contains(&wt), "linked worktree must be excluded");
        let mut sorted = paths.clone();
        sorted.sort();
        assert_eq!(paths, sorted, "output must be sorted");
    }

    #[test]
    fn fetch_then_pull_update_behind_clones() {
        let rmt = remote();
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("git");
        fs::create_dir_all(&root).unwrap();
        let cfg = test_config(&root);

        let one = rmt.clone_to(&root.join("one"));
        let two = rmt.clone_to(&root.join("two"));
        rmt.advance("feature");

        // fetch: both succeed, none fail.
        let fetched = run_fetch(vec![one.clone(), two.clone()], 4);
        assert_eq!(fetched.iter().filter(|o| o.result.is_ok()).count(), 2);
        assert_eq!(
            fetch(&cfg, vec![one.clone(), two.clone()], 4).unwrap(),
            0,
            "fetch exit code"
        );

        // status now shows both a commit behind.
        let statuses = collect_statuses(vec![one.clone(), two.clone()], 4);
        for o in &statuses {
            let st = o.result.as_ref().unwrap();
            assert_eq!(st.behind, 1, "expected 1 behind for {:?}", o.repo);
            assert_eq!(st.ahead, 0);
        }

        // pull: both fast-forward.
        let pulled = run_pull(vec![one.clone(), two.clone()], 4);
        assert_eq!(
            pulled
                .iter()
                .filter(|o| matches!(o.result, Ok(PullOutcome::Updated)))
                .count(),
            2
        );

        // second pull: nothing to do, still exit 0.
        let again = run_pull(vec![one.clone(), two.clone()], 4);
        assert_eq!(
            again
                .iter()
                .filter(|o| matches!(o.result, Ok(PullOutcome::UpToDate)))
                .count(),
            2
        );
        assert_eq!(pull(&cfg, vec![one, two], 4).unwrap(), 0);
    }

    #[test]
    fn fetch_reports_failures_with_exit_4() {
        let rmt = remote();
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("git");
        fs::create_dir_all(&root).unwrap();
        let cfg = test_config(&root);

        let good = rmt.clone_to(&root.join("good"));
        let bad = root.join("bad");
        make_repo(&bad, Some("file:///no/such/path/nope.git"));

        let outcomes = run_fetch(vec![good.clone(), bad.clone()], 4);
        assert_eq!(outcomes.iter().filter(|o| o.result.is_ok()).count(), 1);
        assert_eq!(outcomes.iter().filter(|o| o.result.is_err()).count(), 1);
        assert_eq!(
            fetch(&cfg, vec![good, bad], 4).unwrap(),
            EXIT_PARTIAL_FAILURE
        );
    }

    #[test]
    fn status_flags_dirty_and_tolerates_missing_upstream() {
        let rmt = remote();
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("git");
        fs::create_dir_all(&root).unwrap();

        let tracked = rmt.clone_to(&root.join("tracked"));
        fs::write(tracked.join("dirty.txt"), "wip").unwrap();

        let no_upstream = root.join("local-only");
        make_repo(&no_upstream, None);

        let statuses = collect_statuses(vec![tracked.clone(), no_upstream.clone()], 4);

        let tracked_st = statuses
            .iter()
            .find(|o| o.repo == tracked)
            .unwrap()
            .result
            .as_ref()
            .unwrap();
        assert!(tracked_st.dirty);
        assert_eq!(tracked_st.branch.as_deref(), Some("main"));

        let local_st = statuses
            .iter()
            .find(|o| o.repo == no_upstream)
            .unwrap()
            .result
            .as_ref()
            .expect("no upstream must not error");
        assert_eq!(local_st.ahead, 0);
        assert_eq!(local_st.behind, 0);
        assert!(!local_st.dirty);
    }

    #[test]
    fn status_exit_code_is_zero_when_all_ok() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("git");
        fs::create_dir_all(&root).unwrap();
        let cfg = test_config(&root);
        let repo = root.join("solo");
        make_repo(&repo, None);
        assert_eq!(status(&cfg, vec![repo], 2).unwrap(), 0);
    }
}
