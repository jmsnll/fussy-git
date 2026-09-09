//! A thin, guarded wrapper around the user's `git` binary, plus a few local
//! read-only inspection helpers.
//!
//! Everything that touches the network or authentication goes through the real
//! `git` executable so the user's `~/.ssh/config`, credential helpers,
//! `url.insteadOf` rules, signing and proxies all apply. Linking libgit2 would
//! bypass that configuration, so this crate does not.

use std::ffi::OsStr;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

use anyhow::{bail, Context, Result};

/// Run `git <args>` in `cwd`, or the current directory when `None`, capturing
/// output. Returns an error if git cannot be spawned or exits non-zero.
pub fn run<I, S>(cwd: Option<&Path>, args: I) -> Result<Output>
where
    I: IntoIterator<Item = S>,
    S: AsRef<OsStr>,
{
    let mut cmd = Command::new("git");
    if let Some(dir) = cwd {
        cmd.current_dir(dir);
    }
    // A credential prompt during a bulk scan would hang the whole run.
    cmd.env("GIT_TERMINAL_PROMPT", "0");
    cmd.args(args);
    cmd.stdin(Stdio::null());

    let output = cmd.output().context("spawning `git` (is it on PATH?)")?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        bail!(
            "git {} failed: {}",
            cmd.get_args()
                .map(|a| a.to_string_lossy().into_owned())
                .collect::<Vec<_>>()
                .join(" "),
            stderr.trim()
        );
    }
    Ok(output)
}

/// Run git and return trimmed stdout as a `String`.
pub fn stdout<I, S>(cwd: Option<&Path>, args: I) -> Result<String>
where
    I: IntoIterator<Item = S>,
    S: AsRef<OsStr>,
{
    let out = run(cwd, args)?;
    Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
}

/// Run git streaming stdout/stderr straight to the terminal (for `clone`,
/// `fetch`, `pull` where the user wants progress). Returns whether it succeeded.
pub fn run_inherited<I, S>(cwd: Option<&Path>, args: I) -> Result<bool>
where
    I: IntoIterator<Item = S>,
    S: AsRef<OsStr>,
{
    let mut cmd = Command::new("git");
    if let Some(dir) = cwd {
        cmd.current_dir(dir);
    }
    cmd.env("GIT_TERMINAL_PROMPT", "0");
    cmd.args(args);
    cmd.stdin(Stdio::null());
    let status = cmd.status().context("spawning `git` (is it on PATH?)")?;
    Ok(status.success())
}

/// Is `path` the top level of a git working tree or bare repo? True when it
/// contains a `.git` directory or file (worktrees use a `.git` *file*), or is
/// itself a bare repo.
pub fn is_repo_root(path: &Path) -> bool {
    let dot_git = path.join(".git");
    if dot_git.exists() {
        return true;
    }
    // Bare repo: has HEAD + objects/ + refs/ directly.
    path.join("HEAD").is_file() && path.join("objects").is_dir() && path.join("refs").is_dir()
}

/// A linked worktree has a `.git` *file* (pointing at the real gitdir), not a
/// directory.
pub fn is_linked_worktree(path: &Path) -> bool {
    path.join(".git").is_file()
}

/// `(name, url)` for every configured remote, in git's order.
pub fn remotes(repo: &Path) -> Result<Vec<(String, String)>> {
    let text = stdout(Some(repo), ["remote", "-v"])?;
    let mut seen = Vec::new();
    for line in text.lines() {
        // `origin\tgit@github.com:me/x.git (fetch)`
        let mut it = line.split_whitespace();
        let (Some(name), Some(url)) = (it.next(), it.next()) else {
            continue;
        };
        if !seen.iter().any(|(n, _): &(String, String)| n == name) {
            seen.push((name.to_string(), url.to_string()));
        }
    }
    Ok(seen)
}

/// The remote whose URL should drive the canonical path. Preference order:
/// `preferred` (usually `origin`, or `checkout.defaultRemote`), then `origin`,
/// then `upstream`, then the first remote.
pub fn primary_remote(repo: &Path, preferred: &str) -> Result<Option<(String, String)>> {
    let remotes = remotes(repo)?;
    if remotes.is_empty() {
        return Ok(None);
    }
    for wanted in [preferred, "origin", "upstream"] {
        if let Some(hit) = remotes.iter().find(|(n, _)| n == wanted) {
            return Ok(Some(hit.clone()));
        }
    }
    Ok(remotes.into_iter().next())
}

/// `checkout.defaultRemote`, if the user set one.
pub fn default_remote_name(repo: &Path) -> Option<String> {
    stdout(Some(repo), ["config", "--get", "checkout.defaultRemote"])
        .ok()
        .filter(|s| !s.is_empty())
}

/// True if the working tree has staged or unstaged changes or untracked files.
pub fn is_dirty(repo: &Path) -> Result<bool> {
    let text = stdout(Some(repo), ["status", "--porcelain"])?;
    Ok(!text.is_empty())
}

/// Number of stash entries.
pub fn stash_count(repo: &Path) -> Result<usize> {
    let text = stdout(Some(repo), ["stash", "list"])?;
    Ok(text.lines().filter(|l| !l.is_empty()).count())
}

/// Current branch name, or `None` on a detached HEAD.
pub fn current_branch(repo: &Path) -> Result<Option<String>> {
    let text = stdout(Some(repo), ["symbolic-ref", "--quiet", "--short", "HEAD"]);
    match text {
        Ok(b) if !b.is_empty() => Ok(Some(b)),
        _ => Ok(None),
    }
}

/// `(ahead, behind)` of the upstream tracking branch, or `None` if there is no
/// upstream.
pub fn ahead_behind(repo: &Path) -> Result<Option<(u32, u32)>> {
    let out = run(
        Some(repo),
        ["rev-list", "--left-right", "--count", "HEAD...@{upstream}"],
    );
    let Ok(out) = out else { return Ok(None) };
    let text = String::from_utf8_lossy(&out.stdout);
    let mut nums = text.split_whitespace();
    match (nums.next(), nums.next()) {
        (Some(a), Some(b)) => Ok(Some((a.parse().unwrap_or(0), b.parse().unwrap_or(0)))),
        _ => Ok(None),
    }
}

/// True if a rebase, merge, cherry-pick, bisect or revert is in progress.
pub fn operation_in_progress(repo: &Path) -> bool {
    let git_dir = match stdout(Some(repo), ["rev-parse", "--git-dir"]) {
        Ok(d) if !d.is_empty() => {
            let p = PathBuf::from(&d);
            if p.is_absolute() {
                p
            } else {
                repo.join(p)
            }
        }
        _ => repo.join(".git"),
    };
    [
        "rebase-merge",
        "rebase-apply",
        "MERGE_HEAD",
        "CHERRY_PICK_HEAD",
        "BISECT_LOG",
        "REVERT_HEAD",
    ]
    .iter()
    .any(|marker| git_dir.join(marker).exists())
}

/// Absolute paths of every linked worktree of `repo` (excludes the main one).
pub fn linked_worktrees(repo: &Path) -> Result<Vec<PathBuf>> {
    let text = stdout(Some(repo), ["worktree", "list", "--porcelain"])?;
    let main = std::fs::canonicalize(repo).unwrap_or_else(|_| repo.to_path_buf());
    let mut out = Vec::new();
    for line in text.lines() {
        if let Some(path) = line.strip_prefix("worktree ") {
            let p = PathBuf::from(path);
            let canon = std::fs::canonicalize(&p).unwrap_or_else(|_| p.clone());
            if canon != main {
                out.push(p);
            }
        }
    }
    Ok(out)
}

/// Clone `url` into `dest` with progress streamed to the terminal.
pub fn clone(url: &str, dest: &Path, extra: &[&str]) -> Result<()> {
    if let Some(parent) = dest.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("creating {}", parent.display()))?;
    }
    let mut args: Vec<&OsStr> = vec![OsStr::new("clone")];
    args.extend(extra.iter().map(OsStr::new));
    args.push(OsStr::new(url));
    let dest_os = dest.as_os_str();
    args.push(dest_os);
    if !run_inherited(None, args)? {
        bail!("git clone {url} failed");
    }
    Ok(())
}

/// Clone `url` into `dest`, **capturing** output. On failure the returned error
/// carries git's stderr. Used where progress must not stream to the terminal —
/// the parallel clones of `sync --apply`.
pub fn clone_captured(url: &str, dest: &Path, extra: &[&str]) -> Result<()> {
    if let Some(parent) = dest.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("creating {}", parent.display()))?;
    }
    let mut args: Vec<&OsStr> = vec![OsStr::new("clone")];
    args.extend(extra.iter().map(OsStr::new));
    args.push(OsStr::new(url));
    args.push(dest.as_os_str());
    run(None, args)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::{self, add_remote, add_worktree, make_repo};

    #[test]
    fn detects_repo_root() {
        let dir = tempfile::tempdir().unwrap();
        let repo = dir.path().join("repo");
        make_repo(&repo, Some("git@github.com:me/scratch.git"));
        assert!(is_repo_root(&repo));
        assert!(!is_repo_root(dir.path()));
    }

    #[test]
    fn reads_remotes_and_primary() {
        let dir = tempfile::tempdir().unwrap();
        let repo = dir.path().join("repo");
        make_repo(&repo, Some("git@github.com:me/scratch.git"));
        let remotes = remotes(&repo).unwrap();
        assert_eq!(
            remotes,
            vec![(
                "origin".to_string(),
                "git@github.com:me/scratch.git".to_string()
            )]
        );
        let primary = primary_remote(&repo, "origin").unwrap().unwrap();
        assert_eq!(primary.0, "origin");
    }

    #[test]
    fn primary_prefers_upstream_over_first_when_no_origin() {
        let dir = tempfile::tempdir().unwrap();
        let repo = dir.path().join("repo");
        make_repo(&repo, None);
        add_remote(&repo, "fork", "git@github.com:me/fork.git");
        add_remote(&repo, "upstream", "git@github.com:orig/fork.git");
        let primary = primary_remote(&repo, "origin").unwrap().unwrap();
        assert_eq!(primary.0, "upstream");
    }

    #[test]
    fn dirty_detection() {
        let dir = tempfile::tempdir().unwrap();
        let repo = dir.path().join("repo");
        make_repo(&repo, None);
        assert!(!is_dirty(&repo).unwrap());
        std::fs::write(repo.join("README.md"), "changed").unwrap();
        assert!(is_dirty(&repo).unwrap());
    }

    #[test]
    fn branch_and_operation_state() {
        let dir = tempfile::tempdir().unwrap();
        let repo = dir.path().join("repo");
        make_repo(&repo, None);
        assert_eq!(current_branch(&repo).unwrap().as_deref(), Some("main"));
        assert!(!operation_in_progress(&repo));
    }

    #[test]
    fn no_linked_worktrees_by_default() {
        let dir = tempfile::tempdir().unwrap();
        let repo = dir.path().join("repo");
        make_repo(&repo, None);
        assert!(linked_worktrees(&repo).unwrap().is_empty());
    }

    #[test]
    fn linked_worktree_is_seen_and_flagged() {
        let dir = tempfile::tempdir().unwrap();
        let repo = dir.path().join("repo");
        make_repo(&repo, None);
        let wt = dir.path().join("scratch-wt");
        add_worktree(&repo, &wt, "feature");
        let linked = linked_worktrees(&repo).unwrap();
        assert_eq!(linked.len(), 1);
        assert!(is_linked_worktree(&wt));
    }

    #[test]
    fn run_reports_failure() {
        let err = run(None, ["definitely-not-a-git-command"]).unwrap_err();
        assert!(err.to_string().contains("failed"));
    }

    #[test]
    fn stash_count_reflects_stashes() {
        let dir = tempfile::tempdir().unwrap();
        let repo = dir.path().join("repo");
        make_repo(&repo, None);
        assert_eq!(stash_count(&repo).unwrap(), 0);
        std::fs::write(repo.join("README.md"), "wip").unwrap();
        testutil::git(&repo, &["stash", "-u"]);
        assert_eq!(stash_count(&repo).unwrap(), 1);
    }
}
