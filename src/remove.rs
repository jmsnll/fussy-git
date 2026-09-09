//! `fussy-git remove` — delete one managed repository, with safety checks.
//!
//! Resolution and preflight ([`plan`]) are split from the deletion itself
//! ([`execute`]) so the CLI can show what will happen and prompt for
//! confirmation in between. [`plan`] never mutates anything; [`execute`] does
//! the recursive remove (via [`fsops::remove_tree`], which also prunes the
//! parent directories it empties) and then refreshes the listing index.

use std::path::PathBuf;

use anyhow::{bail, Context, Result};

use crate::config;
use crate::fsops;
use crate::index::{self, RepoEntry};
use crate::preflight::{self, Safety};

/// The outcome of resolving a `remove` query: exactly one repository, plus any
/// reasons deleting it might lose work.
#[derive(Debug, Clone)]
pub struct RemovePlan {
    /// Absolute path to the repository that would be deleted.
    pub path: PathBuf,
    /// `host/owner/repo`, when the repository's remote could be resolved.
    pub slug: Option<String>,
    /// Human-readable reasons the delete is risky: a dirty tree, unpushed
    /// commits, stash entries, an operation in progress, or linked worktrees.
    /// Empty means a clean delete.
    pub blockers: Vec<String>,
}

impl RemovePlan {
    /// The best label for the repository: its slug, or the path if unknown.
    pub fn label(&self) -> String {
        self.slug
            .clone()
            .unwrap_or_else(|| self.path.display().to_string())
    }
}

/// Resolve `query` to a single managed repository and inspect it.
///
/// `query` is matched case-insensitively as a substring of each repository's
/// `host/owner/repo` slug and its `owner/repo` (falling back to the path
/// relative to its root when there is no remote). Zero matches, or more than
/// one, is an error — ambiguity is never resolved by `--force`.
pub fn plan(cfg: &config::Config, query: &str) -> Result<RemovePlan> {
    let index = index::load_or_build(cfg).context("loading the repository index")?;
    let needle = query.to_lowercase();

    let hits: Vec<&RepoEntry> = index
        .entries()
        .iter()
        .filter(|entry| {
            haystacks(entry)
                .iter()
                .any(|hay| hay.to_lowercase().contains(&needle))
        })
        .collect();

    match hits.as_slice() {
        [] => bail!("no repo matches {query:?}"),
        [entry] => {
            let safety = preflight::inspect(&entry.path)
                .with_context(|| format!("inspecting {}", entry.path.display()))?;
            Ok(RemovePlan {
                path: entry.path.clone(),
                slug: entry.slug(),
                blockers: blockers(&safety),
            })
        }
        many => {
            let mut candidates: Vec<String> = many.iter().map(|entry| label(entry)).collect();
            candidates.sort();
            candidates.dedup();
            bail!(
                "{query:?} matches {} repositories:\n{}\nbe more specific",
                candidates.len(),
                candidates.join("\n"),
            );
        }
    }
}

/// Delete the repository described by `plan`.
///
/// Refuses when `plan.blockers` is non-empty unless `force` is set. Afterwards
/// the listing index is refreshed on a best-effort basis (a failure there is a
/// warning, not an error).
pub fn execute(cfg: &config::Config, plan: &RemovePlan, force: bool) -> Result<()> {
    if !plan.blockers.is_empty() && !force {
        bail!(
            "refusing to remove {}:\n{}\npass --force to remove anyway",
            plan.label(),
            plan.blockers
                .iter()
                .map(|b| format!("  - {b}"))
                .collect::<Vec<_>>()
                .join("\n"),
        );
    }

    fsops::remove_tree(cfg, &plan.path)
        .with_context(|| format!("removing {}", plan.path.display()))?;

    if let Err(e) = index::refresh(cfg) {
        eprintln!("warning: could not refresh the index after removal: {e:#}");
    }

    Ok(())
}

/// Strings a query is matched against for one entry: the slug and `owner/repo`
/// when a remote resolved, otherwise the path relative to its root.
fn haystacks(entry: &RepoEntry) -> Vec<String> {
    match entry.slug() {
        Some(slug) => match (&entry.owner, &entry.repo) {
            (Some(owner), Some(repo)) => vec![slug, format!("{owner}/{repo}")],
            _ => vec![slug],
        },
        None => vec![entry.rel.to_string_lossy().into_owned()],
    }
}

/// A stable display label for an entry in the ambiguity error.
fn label(entry: &RepoEntry) -> String {
    entry
        .slug()
        .unwrap_or_else(|| entry.rel.to_string_lossy().into_owned())
}

/// Turn preflight [`Safety`] facts into `remove`'s blocker phrasing.
fn blockers(safety: &Safety) -> Vec<String> {
    let mut out = Vec::new();
    if safety.dirty {
        out.push("uncommitted changes".to_string());
    }
    if safety.ahead > 0 {
        out.push(format!("{} unpushed commit(s)", safety.ahead));
    }
    if safety.stashes > 0 {
        let noun = if safety.stashes == 1 {
            "entry"
        } else {
            "entries"
        };
        out.push(format!("{} stash {noun}", safety.stashes));
    }
    if safety.op_in_progress {
        out.push("operation in progress".to_string());
    }
    for wt in &safety.linked_worktrees {
        out.push(format!("linked worktree: {}", wt.display()));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::make_repo;
    use std::path::Path;

    fn cfg_with_root(root: &Path) -> config::Config {
        let mut c = config::Config::default();
        c.root = root.to_path_buf();
        c.roots = vec![root.to_path_buf()];
        c
    }

    fn tmp_root() -> (tempfile::TempDir, PathBuf) {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("git");
        std::fs::create_dir_all(&root).unwrap();
        (tmp, root)
    }

    #[test]
    fn unique_match_is_deleted_and_empty_parents_pruned() {
        let (_tmp, root) = tmp_root();
        let cfg = cfg_with_root(&root);
        let repo = root.join("github.com/me/alpha");
        make_repo(&repo, Some("git@github.com:me/alpha.git"));

        let plan = plan(&cfg, "alpha").unwrap();
        assert_eq!(plan.slug.as_deref(), Some("github.com/me/alpha"));
        assert!(plan.blockers.is_empty());

        execute(&cfg, &plan, false).unwrap();

        assert!(!repo.exists());
        assert!(!root.join("github.com").exists(), "emptied parents pruned");
        assert!(root.exists(), "the managed root is never pruned");
    }

    #[test]
    fn ambiguous_query_errors_listing_candidates() {
        let (_tmp, root) = tmp_root();
        let cfg = cfg_with_root(&root);
        make_repo(
            &root.join("github.com/alice/widgets"),
            Some("git@github.com:alice/widgets.git"),
        );
        make_repo(
            &root.join("github.com/bob/widgets"),
            Some("git@github.com:bob/widgets.git"),
        );

        let err = format!("{:#}", plan(&cfg, "widgets").unwrap_err());
        assert!(err.contains("github.com/alice/widgets"), "{err}");
        assert!(err.contains("github.com/bob/widgets"), "{err}");
        assert!(err.contains("be more specific"), "{err}");
    }

    #[test]
    fn no_match_errors() {
        let (_tmp, root) = tmp_root();
        let cfg = cfg_with_root(&root);
        make_repo(
            &root.join("github.com/me/alpha"),
            Some("git@github.com:me/alpha.git"),
        );

        let err = format!("{:#}", plan(&cfg, "nonesuch").unwrap_err());
        assert!(err.contains("no repo matches"), "{err}");
    }

    #[test]
    fn dirty_repo_blocks_unless_forced() {
        let (_tmp, root) = tmp_root();
        let cfg = cfg_with_root(&root);
        let repo = root.join("github.com/me/wip");
        make_repo(&repo, Some("git@github.com:me/wip.git"));
        std::fs::write(repo.join("scratch.txt"), "wip").unwrap();

        let plan = plan(&cfg, "wip").unwrap();
        assert!(
            plan.blockers.iter().any(|b| b.contains("uncommitted")),
            "{:?}",
            plan.blockers
        );

        let err = format!("{:#}", execute(&cfg, &plan, false).unwrap_err());
        assert!(err.contains("--force"), "{err}");
        assert!(repo.exists(), "nothing deleted when blocked");

        execute(&cfg, &plan, true).unwrap();
        assert!(!repo.exists());
    }

    #[test]
    fn clean_repo_removes_without_force() {
        let (_tmp, root) = tmp_root();
        let cfg = cfg_with_root(&root);
        let repo = root.join("github.com/me/clean");
        make_repo(&repo, Some("git@github.com:me/clean.git"));

        let plan = plan(&cfg, "clean").unwrap();
        assert!(plan.blockers.is_empty());
        execute(&cfg, &plan, false).unwrap();
        assert!(!repo.exists());
    }
}
