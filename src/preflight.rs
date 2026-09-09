//! Safety checks run before any destructive reconcile operation.
//!
//! [`inspect`] gathers the facts about a repository that decide whether it is
//! safe to move; [`Safety`] turns those facts into hard *blockers* (which stop a
//! move outright) and softer *warnings* (surfaced but not fatal).
//!
//! A move is a pure filesystem rename — it never touches repository contents —
//! so unpushed commits, stashes and a dirty tree all survive it and are only
//! warnings. A move in the middle of a rebase/merge, or of a repo that has
//! linked worktrees pointing back at its old path, would leave things broken,
//! so those are blockers.

use std::path::{Path, PathBuf};

use anyhow::Result;

use crate::git;

/// A snapshot of everything about a repository that bears on whether it can be
/// safely relocated.
#[derive(Debug, Clone)]
pub struct Safety {
    /// Working tree has staged/unstaged changes or untracked files.
    pub dirty: bool,
    /// Commits on `HEAD` not present on its upstream (0 when there is no
    /// upstream).
    pub ahead: u32,
    /// Number of stash entries.
    pub stashes: usize,
    /// A rebase, merge, cherry-pick, bisect or revert is in progress.
    pub op_in_progress: bool,
    /// Absolute paths of linked worktrees (the main worktree is excluded).
    pub linked_worktrees: Vec<PathBuf>,
}

/// Inspect `repo` (its top-level working-tree directory) and collect its
/// [`Safety`] facts.
pub fn inspect(repo: &Path) -> Result<Safety> {
    let dirty = git::is_dirty(repo)?;
    let ahead = git::ahead_behind(repo)?.map(|(a, _)| a).unwrap_or(0);
    let stashes = git::stash_count(repo)?;
    let op_in_progress = git::operation_in_progress(repo);
    let linked_worktrees = git::linked_worktrees(repo)?;
    Ok(Safety {
        dirty,
        ahead,
        stashes,
        op_in_progress,
        linked_worktrees,
    })
}

impl Safety {
    /// Hard stops for a *move*: conditions under which relocating the repo would
    /// leave it (or something pointing at it) broken.
    pub fn blockers(&self) -> Vec<String> {
        let mut out = Vec::new();
        if self.op_in_progress {
            out.push("an operation is in progress (rebase/merge/cherry-pick)".to_string());
        }
        if !self.linked_worktrees.is_empty() {
            out.push(format!(
                "{} linked worktree(s) reference this path",
                self.linked_worktrees.len()
            ));
        }
        out
    }

    /// Conditions worth reporting that a rename nonetheless preserves.
    pub fn warnings(&self) -> Vec<String> {
        let mut out = Vec::new();
        if self.dirty {
            out.push("uncommitted changes in the working tree".to_string());
        }
        if self.ahead > 0 {
            out.push(format!("{} unpushed commit(s)", self.ahead));
        }
        if self.stashes > 0 {
            out.push(format!("{} stash entry(ies)", self.stashes));
        }
        out
    }

    /// Whether the repo can be moved now: no blockers, and either dirtiness is
    /// explicitly allowed or the tree is clean.
    pub fn movable(&self, allow_dirty: bool) -> bool {
        self.blockers().is_empty() && (allow_dirty || !self.dirty)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::{add_worktree, make_repo};

    #[test]
    fn clean_repo_is_movable_with_no_flags() {
        let tmp = tempfile::tempdir().unwrap();
        let repo = tmp.path().join("repo");
        make_repo(&repo, Some("git@github.com:me/x.git"));

        let s = inspect(&repo).unwrap();
        assert!(!s.dirty);
        assert_eq!(s.ahead, 0);
        assert!(s.blockers().is_empty());
        assert!(s.warnings().is_empty());
        assert!(s.movable(false));
        assert!(s.movable(true));
    }

    #[test]
    fn dirty_repo_blocks_a_move_unless_allowed() {
        let tmp = tempfile::tempdir().unwrap();
        let repo = tmp.path().join("repo");
        make_repo(&repo, None);
        std::fs::write(repo.join("scratch.txt"), "wip").unwrap();

        let s = inspect(&repo).unwrap();
        assert!(s.dirty);
        assert!(s.blockers().is_empty(), "dirty is not a hard blocker");
        assert!(!s.warnings().is_empty());
        assert!(!s.movable(false));
        assert!(s.movable(true));
    }

    #[test]
    fn a_linked_worktree_is_a_hard_blocker() {
        let tmp = tempfile::tempdir().unwrap();
        let repo = tmp.path().join("repo");
        make_repo(&repo, None);
        add_worktree(&repo, &tmp.path().join("wt"), "feature");

        let s = inspect(&repo).unwrap();
        assert_eq!(s.linked_worktrees.len(), 1);
        assert!(!s.blockers().is_empty());
        assert!(!s.movable(true));
        assert!(!s.movable(false));
    }

    #[test]
    fn an_operation_in_progress_is_a_hard_blocker() {
        let tmp = tempfile::tempdir().unwrap();
        let repo = tmp.path().join("repo");
        make_repo(&repo, None);
        // Simulate an interrupted rebase without needing a real conflict.
        std::fs::create_dir(repo.join(".git").join("rebase-merge")).unwrap();

        let s = inspect(&repo).unwrap();
        assert!(s.op_in_progress);
        assert!(!s.blockers().is_empty());
        assert!(!s.movable(true));
    }
}
