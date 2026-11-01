//! `fussy-git worktree` — linked worktrees at predictable paths.
//!
//! Worktrees are how parallel work on one repository happens: several editor
//! sessions, or several agents, each on its own branch. Left to themselves they
//! land wherever whoever created them chose. This module gives them the same
//! treatment clones get — a path derived from identity:
//!
//! ```text
//! <worktree_root>/<host>/<owner>/<repo>/<branch>
//! ```
//!
//! where `worktree_root` defaults to `<root>/.worktrees` (a dot-directory, so
//! `scan` never mistakes a worktree for a stray clone) and `/` in a branch name
//! becomes `-`.
//!
//! [`prune`] cleans up what parallel work leaves behind. A worktree is a
//! candidate when its directory is gone, when its branch has been merged into
//! the default branch, or when its upstream branch was deleted (the usual
//! aftermath of a merged pull request). A candidate with uncommitted changes,
//! or a locked one, is never removed. Like `reconcile`, prune is a dry run
//! unless asked to apply.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use anyhow::{bail, Context, Result};
use serde::Serialize;

use crate::config::Config;
use crate::query::{self, real};
use crate::{bulk, fsops, git, hooks, ops, ui};

/// Per-repository timeout when inspecting worktrees.
const INSPECT_TIMEOUT: Duration = Duration::from_secs(60);

/// A linked worktree and the facts that decide whether it can be pruned.
#[derive(Debug, Clone, Serialize)]
pub struct Worktree {
    /// The owning repository, relative to its managed root.
    pub repo: String,
    pub repo_path: PathBuf,
    pub path: PathBuf,
    /// Checked-out branch, or `None` when detached.
    pub branch: Option<String>,
    pub head: String,
    /// Lives under `worktree_root`, i.e. was made by `worktree add`.
    pub managed: bool,
    pub locked: bool,
    /// The worktree directory no longer exists.
    pub missing: bool,
    /// Uncommitted changes; `None` when the directory is missing.
    pub dirty: Option<bool>,
    /// The branch has commits of its own and all of them are in the default
    /// branch.
    pub merged: bool,
    /// The branch's upstream was deleted on the remote.
    pub upstream_gone: bool,
}

/// The path `worktree add` uses for `branch` of the repository at `repo`.
pub fn path_for(cfg: &Config, repo: &Path, branch: &str) -> PathBuf {
    cfg.worktree_root
        .join(repo_rel(cfg, repo))
        .join(branch.replace('/', "-"))
}

/// The repository's path relative to its managed root, or — for a repository
/// outside every root — the canonical path its remote implies, or failing
/// that its directory name.
fn repo_rel(cfg: &Config, repo: &Path) -> PathBuf {
    let repo = real(repo);
    for root in &cfg.roots {
        if let Ok(rel) = repo.strip_prefix(real(root)) {
            return rel.to_path_buf();
        }
    }
    if let Ok(Some((_, url))) = git::primary_remote(&repo, "origin") {
        if let Ok(id) = crate::resolve::identity_from_url(&url) {
            return PathBuf::from(cfg.template_for(&id.host).render(&id, None));
        }
    }
    PathBuf::from(repo.file_name().unwrap_or_default())
}

// --- add -------------------------------------------------------------------

/// What [`add`] did.
#[derive(Debug, Clone, Serialize)]
pub struct Added {
    pub path: PathBuf,
    pub repo: String,
    pub repo_path: PathBuf,
    pub branch: String,
    /// False when a worktree for the branch already existed and was reused.
    pub created: bool,
    /// The branch was created by this call (rather than checked out).
    pub new_branch: bool,
    /// The revision a new branch was started from.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub base: Option<String>,
    pub warnings: Vec<String>,
}

/// Create (or find) the worktree for `branch` of the repository `query` names.
///
/// * A worktree that already has `branch` checked out is returned as is, so
///   `add` is idempotent.
/// * An existing local branch is checked out; a branch that exists only on
///   `origin` is created tracking it; otherwise a new branch is started from
///   `base`, defaulting to the repository's default branch, with no upstream.
pub fn add(cfg: &Config, query: &str, branch: &str, base: Option<&str>) -> Result<Added> {
    let repo = query::repo_for(cfg, query)?;
    let name = ui::repo_name(cfg, &repo);

    if let Some(existing) = git::worktrees(&repo)?
        .into_iter()
        .find(|w| w.branch.as_deref() == Some(branch))
    {
        return Ok(Added {
            path: existing.path,
            repo: name,
            repo_path: repo,
            branch: branch.to_string(),
            created: false,
            new_branch: false,
            base: None,
            warnings: Vec::new(),
        });
    }

    let path = path_for(cfg, &repo, branch);
    if path.exists() {
        bail!(
            "{} already exists but is not a worktree for {branch:?}",
            path.display()
        );
    }
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("creating {}", parent.display()))?;
    }
    let path_arg = path.to_string_lossy().into_owned();

    let (new_branch, start) = if git::ref_exists(&repo, &format!("refs/heads/{branch}")) {
        git::run(Some(&repo), ["worktree", "add", &path_arg, branch])?;
        (false, None)
    } else if git::ref_exists(&repo, &format!("refs/remotes/origin/{branch}")) {
        let upstream = format!("origin/{branch}");
        git::run(
            Some(&repo),
            [
                "worktree", "add", "--track", "-b", branch, &path_arg, &upstream,
            ],
        )?;
        (true, Some(upstream))
    } else {
        let start = base
            .map(str::to_string)
            .or_else(|| git::default_branch(&repo))
            .unwrap_or_else(|| "HEAD".to_string());
        git::run(
            Some(&repo),
            [
                "worktree",
                "add",
                "--no-track",
                "-b",
                branch,
                &path_arg,
                &start,
            ],
        )?;
        (true, Some(start))
    };

    let warnings = hooks::run("post_worktree", &cfg.hooks.post_worktree, &path);
    for w in &warnings {
        eprintln!("{} {w}", ui::warn_prefix());
    }

    Ok(Added {
        path,
        repo: name,
        repo_path: repo,
        branch: branch.to_string(),
        created: true,
        new_branch,
        base: start,
        warnings,
    })
}

// --- list ------------------------------------------------------------------

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
    let entries = git::worktrees(repo)?;
    let default = git::default_branch(repo);
    let default_short = default
        .as_deref()
        .map(|d| d.strip_prefix("origin/").unwrap_or(d).to_string());
    // Merged means merged into the default branch on the remote *or* into the
    // local branch of the same name, since work is often merged locally first.
    let mut merge_targets: Vec<String> = default.iter().cloned().collect();
    if let Some(short) = &default_short {
        if Some(short) != default.as_ref() && git::ref_exists(repo, &format!("refs/heads/{short}"))
        {
            merge_targets.push(short.clone());
        }
    }
    let managed_root = real(&cfg.worktree_root);
    let name = ui::repo_name(cfg, repo);

    // The first entry is the main worktree; bare entries have no checkout.
    Ok(entries
        .into_iter()
        .skip(1)
        .filter(|e| !e.bare)
        .map(|e| {
            let missing = e.prunable || !e.path.exists();
            let dirty = if missing {
                None
            } else {
                git::is_dirty(&e.path).ok()
            };
            let (merged, upstream_gone) = match (&e.branch, &default) {
                (Some(b), Some(_)) if Some(b) != default_short.as_ref() => (
                    git::branch_reflog_len(repo, b) > 1
                        && merge_targets.iter().any(|t| git::is_ancestor(repo, b, t)),
                    git::upstream_gone(repo, b),
                ),
                (Some(b), None) => (false, git::upstream_gone(repo, b)),
                _ => (false, false),
            };
            Worktree {
                repo: name.clone(),
                repo_path: repo.to_path_buf(),
                managed: real(&e.path).starts_with(&managed_root),
                path: e.path,
                branch: e.branch,
                head: e.head,
                locked: e.locked,
                missing,
                dirty,
                merged,
                upstream_gone,
            }
        })
        .collect())
}

// --- prune -----------------------------------------------------------------

/// Why a worktree is a prune candidate.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PruneReason {
    /// The directory is gone; only git's bookkeeping remains.
    Missing,
    /// The branch is merged into the default branch.
    Merged,
    /// The branch's upstream was deleted on the remote.
    UpstreamGone,
}

impl std::fmt::Display for PruneReason {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            PruneReason::Missing => "directory missing",
            PruneReason::Merged => "branch merged",
            PruneReason::UpstreamGone => "upstream branch deleted",
        })
    }
}

/// A worktree [`prune`] would remove.
#[derive(Debug, Clone, Serialize)]
pub struct PruneCandidate {
    #[serde(flatten)]
    pub worktree: Worktree,
    pub reason: PruneReason,
}

/// A worktree that would be a candidate but is protected.
#[derive(Debug, Clone, Serialize)]
pub struct Protected {
    pub path: PathBuf,
    pub reason: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct Failure {
    pub path: PathBuf,
    pub error: String,
}

/// Options for [`prune`].
#[derive(Debug, Clone, Default)]
pub struct PruneOptions {
    /// Actually remove the candidates. The default is a dry run.
    pub apply: bool,
    /// Also consider worktrees outside `worktree_root`. Missing worktrees are
    /// always considered, wherever they were.
    pub all: bool,
    pub query: Option<String>,
}

/// What [`prune`] found and, with `apply`, did.
#[derive(Debug, Clone, Default, Serialize)]
pub struct PruneReport {
    pub applied: bool,
    pub candidates: Vec<PruneCandidate>,
    pub protected: Vec<Protected>,
    pub removed: Vec<PathBuf>,
    /// Merged branches deleted (with `git branch -d`) after their worktree.
    pub deleted_branches: Vec<String>,
    pub failed: Vec<Failure>,
}

/// Find prune candidates and, when `opts.apply`, remove them.
pub fn prune(cfg: &Config, opts: &PruneOptions) -> Result<PruneReport> {
    let mut report = PruneReport {
        applied: opts.apply,
        ..Default::default()
    };
    for wt in list(cfg, opts.query.as_deref())? {
        match verdict(&wt, opts.all) {
            Verdict::Keep => {}
            Verdict::Protect(reason) => report.protected.push(Protected {
                path: wt.path,
                reason,
            }),
            Verdict::Prune(reason) => report.candidates.push(PruneCandidate {
                worktree: wt,
                reason,
            }),
        }
    }
    if opts.apply {
        for c in &report.candidates.clone() {
            apply_one(cfg, c, &mut report);
        }
    }
    Ok(report)
}

enum Verdict {
    Keep,
    Protect(String),
    Prune(PruneReason),
}

fn verdict(wt: &Worktree, all: bool) -> Verdict {
    if wt.missing {
        return if wt.locked {
            Verdict::Protect("locked".to_string())
        } else {
            Verdict::Prune(PruneReason::Missing)
        };
    }
    if !wt.managed && !all {
        return Verdict::Keep;
    }
    let reason = if wt.merged {
        PruneReason::Merged
    } else if wt.upstream_gone {
        PruneReason::UpstreamGone
    } else {
        return Verdict::Keep;
    };
    if wt.locked {
        Verdict::Protect(format!("{reason}, but locked"))
    } else if wt.dirty != Some(false) {
        Verdict::Protect(format!("{reason}, but has uncommitted changes"))
    } else {
        Verdict::Prune(reason)
    }
}

fn apply_one(cfg: &Config, c: &PruneCandidate, report: &mut PruneReport) {
    let wt = &c.worktree;
    let repo = wt.repo_path.as_path();
    let result = match c.reason {
        PruneReason::Missing => git::run(Some(repo), ["worktree", "prune"]).map(|_| ()),
        PruneReason::Merged | PruneReason::UpstreamGone => {
            let path = wt.path.to_string_lossy().into_owned();
            git::run(Some(repo), ["worktree", "remove", &path]).map(|_| ())
        }
    };
    if let Err(e) = result {
        report.failed.push(Failure {
            path: wt.path.clone(),
            error: format!("{e:#}"),
        });
        return;
    }
    report.removed.push(wt.path.clone());
    if wt.managed {
        fsops::prune_empty_dirs(wt.path.parent(), &real(&cfg.worktree_root));
    }
    // Only a merged branch is deleted, and only with `-d`, which git refuses
    // for unmerged work. An upstream-gone branch may hold squash-merged or
    // abandoned commits, so it is kept.
    if c.reason == PruneReason::Merged {
        if let Some(b) = &wt.branch {
            if git::run(Some(repo), ["branch", "-d", b]).is_ok() {
                report.deleted_branches.push(b.clone());
            }
        }
    }
}

// --- remove ----------------------------------------------------------------

/// What [`remove`] did.
#[derive(Debug, Clone, Serialize)]
pub struct Removed {
    pub path: PathBuf,
    pub repo_path: PathBuf,
    /// The branch that was checked out; it is kept.
    pub branch: Option<String>,
}

/// Remove the linked worktree containing `path`. git refuses when it has
/// uncommitted changes unless `force`. The branch is kept.
pub fn remove(cfg: &Config, path: &Path, force: bool) -> Result<Removed> {
    let top = git::toplevel(path)
        .with_context(|| format!("{} is not inside a git worktree", path.display()))?;
    if !git::is_linked_worktree(&top) {
        bail!(
            "{} is a repository, not a linked worktree (use `fussy-git remove` for a repository)",
            top.display()
        );
    }
    let repo = git::main_worktree(&top)?;
    let branch = git::current_branch(&top)?;
    let top_arg = top.to_string_lossy().into_owned();
    let mut args = vec!["worktree", "remove"];
    if force {
        args.push("--force");
    }
    args.push(&top_arg);
    git::run(Some(&repo), args)?;

    let root = real(&cfg.worktree_root);
    if real(top.parent().unwrap_or(&top)).starts_with(&root) {
        fsops::prune_empty_dirs(Some(&real(top.parent().unwrap_or(&top))), &root);
    }
    Ok(Removed {
        path: top,
        repo_path: repo,
        branch,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::{self, make_repo};

    struct Fx {
        _tmp: tempfile::TempDir,
        cfg: Config,
        repo: PathBuf,
    }

    fn fixture() -> Fx {
        let tmp = tempfile::tempdir().unwrap();
        let root = real(tmp.path()).join("git");
        let repo = root.join("github.com/me/app");
        make_repo(&repo, Some("git@github.com:me/app.git"));
        let mut cfg = Config::default();
        cfg.root = root.clone();
        cfg.roots = vec![root.clone()];
        cfg.worktree_root = root.join(".worktrees");
        Fx {
            _tmp: tmp,
            cfg,
            repo,
        }
    }

    fn commit(dir: &Path, file: &str) {
        std::fs::write(dir.join(file), file).unwrap();
        testutil::git(dir, &["add", "."]);
        testutil::git(dir, &["commit", "-qm", file, "--no-verify"]);
    }

    #[test]
    fn add_uses_the_canonical_path_and_is_idempotent() {
        let fx = fixture();
        let repo_q = fx.repo.to_string_lossy().into_owned();
        let added = add(&fx.cfg, &repo_q, "feat/login", None).unwrap();
        assert_eq!(
            added.path,
            fx.cfg.worktree_root.join("github.com/me/app/feat-login")
        );
        assert!(added.created && added.new_branch);
        assert_eq!(added.base.as_deref(), Some("main"));
        assert!(git::is_linked_worktree(&added.path));

        let again = add(&fx.cfg, &repo_q, "feat/login", None).unwrap();
        assert!(!again.created);
        assert_eq!(real(&again.path), real(&added.path));
    }

    #[test]
    fn fresh_branch_is_not_merged_but_merged_branch_is_pruned() {
        let fx = fixture();
        let repo_q = fx.repo.to_string_lossy().into_owned();
        let fresh = add(&fx.cfg, &repo_q, "fresh", None).unwrap();
        let done = add(&fx.cfg, &repo_q, "done", None).unwrap();

        // `done` gets a commit that is then merged into main; `fresh` is
        // untouched and must survive even though it is trivially an ancestor.
        commit(&done.path, "work.txt");
        testutil::git(&fx.repo, &["merge", "-q", "--ff-only", "done"]);

        let dry = prune(&fx.cfg, &PruneOptions::default()).unwrap();
        let paths: Vec<PathBuf> = dry
            .candidates
            .iter()
            .map(|c| real(&c.worktree.path))
            .collect();
        assert_eq!(paths, vec![real(&done.path)]);
        assert_eq!(dry.candidates[0].reason, PruneReason::Merged);
        assert!(done.path.exists(), "a dry run removes nothing");

        let applied = prune(
            &fx.cfg,
            &PruneOptions {
                apply: true,
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(applied.removed.len(), 1);
        assert_eq!(applied.deleted_branches, vec!["done".to_string()]);
        assert!(!done.path.exists());
        assert!(fresh.path.exists());
    }

    #[test]
    fn dirty_merged_worktree_is_protected() {
        let fx = fixture();
        let repo_q = fx.repo.to_string_lossy().into_owned();
        let wt = add(&fx.cfg, &repo_q, "wip", None).unwrap();
        commit(&wt.path, "a.txt");
        testutil::git(&fx.repo, &["merge", "-q", "--ff-only", "wip"]);
        std::fs::write(wt.path.join("scratch.txt"), "unsaved").unwrap();

        let report = prune(&fx.cfg, &PruneOptions::default()).unwrap();
        assert!(report.candidates.is_empty());
        assert_eq!(report.protected.len(), 1);
        assert!(report.protected[0].reason.contains("uncommitted"));
    }

    #[test]
    fn remove_deletes_the_worktree_and_keeps_the_branch() {
        let fx = fixture();
        let repo_q = fx.repo.to_string_lossy().into_owned();
        let wt = add(&fx.cfg, &repo_q, "task", None).unwrap();
        let removed = remove(&fx.cfg, &wt.path, false).unwrap();
        assert_eq!(removed.branch.as_deref(), Some("task"));
        assert!(!wt.path.exists());
        assert!(git::ref_exists(&fx.repo, "refs/heads/task"));
        assert!(
            !fx.cfg.worktree_root.join("github.com").exists(),
            "emptied parents under the worktree root are pruned"
        );

        let err = remove(&fx.cfg, &fx.repo, false).unwrap_err().to_string();
        assert!(err.contains("not a linked worktree"), "{err}");
    }
}
