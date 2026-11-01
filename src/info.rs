//! `fussy-git info` — everything about one repository in a single call.
//!
//! With no argument it describes the repository containing the current
//! directory, which is the first question an agent (or a person in an
//! unfamiliar checkout) asks: what is this, is it where it belongs, what branch
//! am I on, is it clean, and what else is checked out from it. When the current
//! directory is a linked worktree, the branch and working-tree fields describe
//! that worktree and `worktree` names it; everything else describes the
//! repository that owns it.

use std::fmt::Write as _;
use std::path::{Path, PathBuf};

use anyhow::Result;
use serde::Serialize;

use crate::config::Config;
use crate::identity::Identity;
use crate::query::{self, real};
use crate::{fsops, get, git, resolve, ui};

#[derive(Debug, Clone, Serialize)]
pub struct Remote {
    pub name: String,
    pub url: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct WorktreeRef {
    pub path: PathBuf,
    pub branch: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct Info {
    /// The main repository.
    pub path: PathBuf,
    /// The linked worktree being described, when the query was a path inside
    /// one.
    pub worktree: Option<PathBuf>,
    /// The managed root containing the repository, if any.
    pub root: Option<PathBuf>,
    pub identity: Option<Identity>,
    /// Where the repository belongs according to its remote.
    pub canonical_path: Option<PathBuf>,
    /// The repository already sits at `canonical_path`.
    pub in_place: bool,
    pub remotes: Vec<Remote>,
    pub branch: Option<String>,
    /// The revision `worktree add` and merge checks treat as the default
    /// branch, e.g. `origin/main`.
    pub default_branch: Option<String>,
    pub upstream: Option<String>,
    pub ahead: u32,
    pub behind: u32,
    pub dirty: bool,
    pub stashes: usize,
    /// Linked worktrees of the repository (the main checkout excluded).
    pub worktrees: Vec<WorktreeRef>,
}

/// Describe the repository `query` names, or the one containing the current
/// directory when `query` is `None`.
pub fn run(cfg: &Config, query: Option<&str>) -> Result<Info> {
    let query = query.unwrap_or(".");
    let repo = query::repo_for(cfg, query)?;
    let here = if query::is_path_like(query) {
        let path = PathBuf::from(shellexpand::tilde(query.trim()).into_owned());
        git::toplevel(&path)?
    } else {
        repo.clone()
    };
    let worktree = (real(&here) != real(&repo)).then(|| here.clone());

    let remotes: Vec<Remote> = git::remotes(&repo)?
        .into_iter()
        .map(|(name, url)| Remote { name, url })
        .collect();
    let preferred = git::default_remote_name(&repo).unwrap_or_else(|| "origin".to_string());
    let identity: Option<Identity> = git::primary_remote(&repo, &preferred)?
        .and_then(|(_, url)| resolve::identity_from_url(&url).ok());
    let canonical_path = identity.as_ref().map(|id| get::canonical_path(cfg, id));
    let in_place = canonical_path
        .as_ref()
        .is_some_and(|c| real(c) == real(&repo));

    let (ahead, behind) = git::ahead_behind(&here)?.unwrap_or((0, 0));
    let worktrees = git::worktrees(&repo)?
        .into_iter()
        .skip(1)
        .map(|w| WorktreeRef {
            path: w.path,
            branch: w.branch,
        })
        .collect();

    Ok(Info {
        root: fsops::managed_root(cfg, &repo).map(Path::to_path_buf),
        path: repo.clone(),
        worktree,
        identity,
        canonical_path,
        in_place,
        remotes,
        branch: git::current_branch(&here)?,
        default_branch: git::default_branch(&repo),
        upstream: git::stdout(
            Some(&here),
            [
                "rev-parse",
                "--abbrev-ref",
                "--symbolic-full-name",
                "@{upstream}",
            ],
        )
        .ok()
        .filter(|s| !s.is_empty()),
        ahead,
        behind,
        dirty: git::is_dirty(&here)?,
        stashes: git::stash_count(&repo)?,
        worktrees,
    })
}

impl Info {
    /// Aligned `key  value` lines for a terminal.
    pub fn render(&self, cfg: &Config) -> String {
        let mut rows: Vec<(&str, String)> = Vec::new();
        let none = || ui::dim("none");
        rows.push((
            "identity",
            self.identity
                .as_ref()
                .map(|i| i.to_string())
                .unwrap_or_else(none),
        ));
        rows.push(("path", self.path.display().to_string()));
        if let Some(w) = &self.worktree {
            rows.push(("worktree", w.display().to_string()));
        }
        rows.push((
            "placement",
            match (&self.canonical_path, self.in_place) {
                (_, true) => ui::ok("in place"),
                (Some(c), false) => {
                    ui::warn(&format!("misplaced, belongs at {}", ui::repo_name(cfg, c)))
                }
                (None, false) => ui::dim("unknown (no usable remote)"),
            },
        ));
        for r in &self.remotes {
            rows.push(("remote", format!("{}  {}", r.name, r.url)));
        }
        rows.push((
            "branch",
            self.branch
                .clone()
                .unwrap_or_else(|| "detached".to_string()),
        ));
        rows.push(("default", self.default_branch.clone().unwrap_or_else(none)));
        rows.push((
            "upstream",
            match &self.upstream {
                Some(u) => format!("{u}  +{} -{}", self.ahead, self.behind),
                None => none(),
            },
        ));
        let mut state = Vec::new();
        if self.dirty {
            state.push(ui::warn("dirty"));
        }
        if self.stashes > 0 {
            state.push(ui::warn(&format!("stash {}", self.stashes)));
        }
        rows.push((
            "state",
            if state.is_empty() {
                ui::ok("clean")
            } else {
                state.join(", ")
            },
        ));
        for w in &self.worktrees {
            rows.push((
                "worktree",
                format!(
                    "{}  {}",
                    w.branch.as_deref().unwrap_or("detached"),
                    ui::dim(&w.path.display().to_string())
                ),
            ));
        }

        let width = rows.iter().map(|(k, _)| k.len()).max().unwrap_or(0);
        let mut out = String::new();
        for (k, v) in rows {
            let _ = writeln!(out, "{}  {v}", ui::dim(&format!("{k:<width$}")));
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::{add_worktree, make_repo};

    #[test]
    fn describes_a_repo_and_one_of_its_worktrees() {
        let tmp = tempfile::tempdir().unwrap();
        let root = real(tmp.path()).join("git");
        let repo = root.join("github.com/me/tool");
        make_repo(&repo, Some("git@github.com:me/tool.git"));
        let wt = tmp.path().join("wt");
        add_worktree(&repo, &wt, "side");
        let mut cfg = Config::default();
        cfg.root = root.clone();
        cfg.roots = vec![root.clone()];

        let info = run(&cfg, Some(&repo.to_string_lossy())).unwrap();
        assert_eq!(info.identity.unwrap().to_string(), "github.com/me/tool");
        assert!(info.in_place);
        assert_eq!(info.root.as_deref(), Some(root.as_path()));
        assert_eq!(info.branch.as_deref(), Some("main"));
        assert_eq!(info.worktrees.len(), 1);
        assert!(info.worktree.is_none());

        let from_wt = run(&cfg, Some(&wt.to_string_lossy())).unwrap();
        assert_eq!(real(&from_wt.path), real(&repo));
        assert_eq!(from_wt.branch.as_deref(), Some("side"));
        assert!(from_wt.worktree.is_some());
    }
}
