//! Read-only awareness of linked worktrees, whoever created them.
//!
//! fussy-git decides where *repositories* live; creating, switching, merging
//! and removing single-repository worktrees belongs to tools built for that —
//! Worktrunk (`wt`), Claude Code's `--worktree`, or plain `git worktree`. What fussy-git adds is
//! the view across every repository: [`list`] reports each repository's linked
//! worktrees wherever they are on disk, and [`resolve`] turns `repo@branch`
//! into the checkout of that branch, so `fussy-git cd api@feat-auth` jumps to a
//! worktree made by any of them.
//!
//! Worktrunk's default layout puts worktrees beside the repository
//! (`~/git/github.com/acme/api.feat-auth`), inside a managed root. `scan`
//! classifies those as linked worktrees, and the index leaves them out, so they
//! never masquerade as repositories of their own.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use anyhow::{bail, Result};
use serde::Serialize;

use crate::config::Config;
use crate::query::{self, real};
use crate::{bulk, git, ops, ui};

/// Per-repository timeout when inspecting worktrees.
const INSPECT_TIMEOUT: Duration = Duration::from_secs(60);

/// A linked worktree of a managed repository.
#[derive(Debug, Clone, Serialize)]
pub struct Worktree {
    /// The owning repository, relative to its managed root.
    pub repo: String,
    pub repo_path: PathBuf,
    pub path: PathBuf,
    /// Checked-out branch, or `None` when detached.
    pub branch: Option<String>,
    pub head: String,
    pub locked: bool,
    /// The worktree directory no longer exists; `git worktree prune` (or the
    /// tool that made it) should clean it up.
    pub missing: bool,
    /// Uncommitted changes; `None` when the directory is missing.
    pub dirty: Option<bool>,
}

/// Every linked worktree of the repositories `query` selects (all of them when
/// `None`; `@set` works as in the bulk commands).
pub fn list(cfg: &Config, query: Option<&str>) -> Result<Vec<Worktree>> {
    let repos = bulk::select(cfg, query)?;
    let shared = Arc::new(cfg.clone());
    let outcomes = ops::for_each_repo(repos, cfg.jobs, INSPECT_TIMEOUT, move |repo| {
        inspect_repo(&shared, repo)
    });
    let mut out = Vec::new();
    for o in outcomes {
        match o.result {
            Ok(wts) => out.extend(wts),
            Err(e) => eprintln!("{} {}: {e}", ui::warn_prefix(), ui::repo_name(cfg, &o.repo)),
        }
    }
    Ok(out)
}

fn inspect_repo(cfg: &Config, repo: &Path) -> Result<Vec<Worktree>> {
    let name = ui::repo_name(cfg, repo);
    // The first entry is the main worktree; bare entries have no checkout.
    Ok(git::worktrees(repo)?
        .into_iter()
        .skip(1)
        .filter(|e| !e.bare)
        .map(|e| {
            let missing = e.prunable || !e.path.exists();
            Worktree {
                repo: name.clone(),
                repo_path: repo.to_path_buf(),
                dirty: if missing {
                    None
                } else {
                    git::is_dirty(&e.path).ok()
                },
                path: e.path,
                branch: e.branch,
                head: e.head,
                locked: e.locked,
                missing,
            }
        })
        .collect())
}

/// Split `api@feat-auth` into the repository query and the branch. A leading
/// `@` is a set reference, not a branch selector, and is left alone.
pub fn split_branch(query: &str) -> Option<(&str, &str)> {
    let q = query.trim();
    if q.starts_with('@') || query::is_path_like(q) {
        return None;
    }
    q.rsplit_once('@')
        .filter(|(repo, branch)| !repo.is_empty() && !branch.is_empty())
}

/// The checkout of `branch` in the repository at `repo`: the main checkout when
/// that is where the branch is, otherwise the linked worktree that has it.
/// `/` and `-` are interchangeable, so `feat-auth` finds `feat/auth` — the
/// spelling Worktrunk uses in directory names.
pub fn resolve(repo: &Path, branch: &str) -> Result<PathBuf> {
    let norm = |b: &str| b.replace('/', "-");
    let wanted = norm(branch);
    let entries = git::worktrees(repo)?;
    if let Some(hit) = entries
        .iter()
        .find(|e| e.branch.as_deref().map(norm).as_deref() == Some(wanted.as_str()))
    {
        return Ok(hit.path.clone());
    }
    let available: Vec<&str> = entries.iter().filter_map(|e| e.branch.as_deref()).collect();
    bail!(
        "no worktree has branch {branch:?} checked out in {} (checked out: {}); \
         create one with `wt switch -c {branch}` or `git worktree add`",
        real(repo).display(),
        if available.is_empty() {
            "none".to_string()
        } else {
            available.join(", ")
        }
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::{add_worktree, make_repo};

    #[test]
    fn splits_branch_selectors_but_not_sets_or_paths() {
        assert_eq!(split_branch("api@feat-auth"), Some(("api", "feat-auth")));
        assert_eq!(
            split_branch("acme/api@feat/x"),
            Some(("acme/api", "feat/x"))
        );
        assert_eq!(split_branch("@payments"), None);
        assert_eq!(split_branch("./x@y"), None);
        assert_eq!(split_branch("api@"), None);
        assert_eq!(split_branch("api"), None);
    }

    #[test]
    fn resolves_branches_to_their_checkout() {
        let tmp = tempfile::tempdir().unwrap();
        let root = real(tmp.path()).join("git");
        let repo = root.join("github.com/acme/api");
        make_repo(&repo, Some("git@github.com:acme/api.git"));
        let wt = root.join("github.com/acme/api.feat-auth");
        add_worktree(&repo, &wt, "feat/auth");

        assert_eq!(real(&resolve(&repo, "main").unwrap()), real(&repo));
        assert_eq!(real(&resolve(&repo, "feat/auth").unwrap()), real(&wt));
        assert_eq!(real(&resolve(&repo, "feat-auth").unwrap()), real(&wt));
        let err = resolve(&repo, "nope").unwrap_err().to_string();
        assert!(
            err.contains("feat/auth") && err.contains("wt switch"),
            "{err}"
        );
    }

    #[test]
    fn lists_sibling_worktrees_under_their_repo() {
        let tmp = tempfile::tempdir().unwrap();
        let root = real(tmp.path()).join("git");
        let repo = root.join("github.com/acme/api");
        make_repo(&repo, Some("git@github.com:acme/api.git"));
        add_worktree(
            &repo,
            &root.join("github.com/acme/api.feat-auth"),
            "feat/auth",
        );
        let mut cfg = Config::default();
        cfg.root = root.clone();
        cfg.roots = vec![root];

        let wts = list(&cfg, None).unwrap();
        assert_eq!(wts.len(), 1);
        assert_eq!(wts[0].repo, "github.com/acme/api");
        assert_eq!(wts[0].branch.as_deref(), Some("feat/auth"));
        assert_eq!(wts[0].dirty, Some(false));
    }
}
