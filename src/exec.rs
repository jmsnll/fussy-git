//! `fussy-git exec` — run one command in many repositories, in parallel.
//!
//! The command is executed directly (no shell), once per repository, with the
//! repository as its working directory; pass `sh -c '…'` explicitly for pipes
//! or globbing. Output is captured per repository and reported in input order,
//! so results never interleave. Each run also sees `FUSSY_GIT_REPO` (the path
//! relative to its root, e.g. `github.com/acme/api`) and `FUSSY_GIT_REPO_PATH`.
//!
//! A repository "fails" when the command exits non-zero, cannot be started, or
//! times out; any failure makes the batch exit `4`.

use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::Arc;
use std::time::Duration;

use anyhow::Result;
use serde::Serialize;

use crate::bulk::EXIT_PARTIAL_FAILURE;
use crate::config::Config;
use crate::ops;
use crate::ui::{self, Format};

/// Captured result of running the command in one repository.
#[derive(Debug, Clone)]
pub struct Ran {
    /// `None` when the process was killed by a signal.
    pub exit_code: Option<i32>,
    pub stdout: String,
    pub stderr: String,
}

impl Ran {
    fn succeeded(&self) -> bool {
        self.exit_code == Some(0)
    }
}

#[derive(Serialize)]
struct Row<'a> {
    repo: String,
    path: &'a PathBuf,
    ok: bool,
    exit_code: Option<i32>,
    stdout: &'a str,
    stderr: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<&'a str>,
}

/// Run `argv` in every repository in `repos` and report. Returns the exit code.
pub fn run(
    cfg: &Config,
    repos: Vec<PathBuf>,
    argv: Vec<String>,
    jobs: usize,
    timeout: Duration,
    format: Format,
) -> Result<i32> {
    let names: Vec<String> = repos.iter().map(|r| ui::repo_name(cfg, r)).collect();
    let argv = Arc::new(argv);
    let shared_cfg = Arc::new(cfg.clone());

    let outcomes = ops::for_each_repo(repos, jobs, timeout, move |repo| {
        let out = Command::new(&argv[0])
            .args(&argv[1..])
            .current_dir(repo)
            .env("FUSSY_GIT_REPO", ui::repo_name(&shared_cfg, repo))
            .env("FUSSY_GIT_REPO_PATH", repo)
            .stdin(Stdio::null())
            .output()
            .map_err(|e| anyhow::anyhow!("starting {:?}: {e}", argv[0]))?;
        Ok(Ran {
            exit_code: out.status.code(),
            stdout: String::from_utf8_lossy(&out.stdout).into_owned(),
            stderr: String::from_utf8_lossy(&out.stderr).into_owned(),
        })
    });

    let failed = outcomes
        .iter()
        .filter(|o| !matches!(&o.result, Ok(r) if r.succeeded()))
        .count();

    if format.is_json() {
        let rows: Vec<Row> = outcomes
            .iter()
            .zip(&names)
            .map(|(o, name)| match &o.result {
                Ok(r) => Row {
                    repo: name.clone(),
                    path: &o.repo,
                    ok: r.succeeded(),
                    exit_code: r.exit_code,
                    stdout: &r.stdout,
                    stderr: &r.stderr,
                    error: None,
                },
                Err(e) => Row {
                    repo: name.clone(),
                    path: &o.repo,
                    ok: false,
                    exit_code: None,
                    stdout: "",
                    stderr: "",
                    error: Some(e),
                },
            })
            .collect();
        ui::print_json(&rows)?;
    } else {
        for (o, name) in outcomes.iter().zip(&names) {
            match &o.result {
                Ok(r) => {
                    let status = match r.exit_code {
                        Some(0) => ui::ok("ok"),
                        Some(c) => ui::danger(&format!("exit {c}")),
                        None => ui::danger("killed"),
                    };
                    println!("{}  {status}", ui::header(name));
                    print!("{}", with_newline(&r.stdout));
                    eprint!("{}", with_newline(&r.stderr));
                }
                Err(e) => println!(
                    "{}  {}  {}",
                    ui::header(name),
                    ui::danger("failed"),
                    ui::dim(e)
                ),
            }
        }
        let mut summary = vec![format!("{} ok", outcomes.len() - failed)];
        if failed > 0 {
            summary.push(ui::danger(&format!("{failed} failed")));
        }
        println!("\n{}", summary.join(", "));
    }

    Ok(if failed > 0 { EXIT_PARTIAL_FAILURE } else { 0 })
}

fn with_newline(s: &str) -> String {
    if s.is_empty() || s.ends_with('\n') {
        s.to_string()
    } else {
        format!("{s}\n")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::make_repo;

    #[test]
    fn exit_codes_decide_the_batch_result() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("git");
        let a = root.join("github.com/me/a");
        let b = root.join("github.com/me/b");
        make_repo(&a, None);
        make_repo(&b, None);
        std::fs::write(b.join("marker"), "").unwrap();
        let mut cfg = Config::default();
        cfg.root = root.clone();
        cfg.roots = vec![root];

        let ok = run(
            &cfg,
            vec![a.clone(), b.clone()],
            vec!["true".into()],
            2,
            Duration::from_secs(30),
            Format::Json,
        )
        .unwrap();
        assert_eq!(ok, 0);

        // `test -e marker` succeeds only in `b`.
        let partial = run(
            &cfg,
            vec![a, b],
            vec!["test".into(), "-e".into(), "marker".into()],
            2,
            Duration::from_secs(30),
            Format::Json,
        )
        .unwrap();
        assert_eq!(partial, EXIT_PARTIAL_FAILURE);
    }
}
