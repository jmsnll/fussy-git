//! `fussy-git list` — enumerate managed repositories from the index.
//!
//! Served from [`index::load_or_build`], so it is instant unless the tree
//! changed. Text filters (`query`/`host`/`owner`) are applied against the cached
//! metadata; the git-state filters (`--dirty`, `--unpushed`) shell out per repo
//! and run sequentially — batching those across a worker pool is the ops
//! module's job, not this one.

use std::io::{self, Write};

use anyhow::Result;

use crate::config;
use crate::git;
use crate::index::{self, RepoEntry};

/// Output shape for [`run`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ListFormat {
    /// One human-readable line per repo (`host/owner/repo`).
    Table,
    /// Absolute path per line, for scripting.
    Paths,
    /// Pretty-printed JSON array of the matching entries.
    Json,
}

/// Filters for [`run`], mirroring the CLI flags.
#[derive(Default)]
pub struct ListFilter {
    pub query: Option<String>,
    pub host: Option<String>,
    pub owner: Option<String>,
    pub dirty: bool,
    pub unpushed: bool,
}

/// List managed repositories to stdout.
pub fn run(cfg: &config::Config, filter: &ListFilter, format: ListFormat) -> Result<()> {
    let index = index::load_or_build(cfg)?;
    let stdout = io::stdout();
    let mut lock = stdout.lock();
    render(&index, filter, format, &mut lock)
}

fn render<W: Write>(
    index: &index::Index,
    filter: &ListFilter,
    format: ListFormat,
    w: &mut W,
) -> Result<()> {
    let mut selected = index.filter(
        filter.query.as_deref(),
        filter.host.as_deref(),
        filter.owner.as_deref(),
    );
    if filter.dirty {
        selected.retain(|e| git::is_dirty(&e.path).unwrap_or(false));
    }
    if filter.unpushed {
        selected
            .retain(|e| matches!(git::ahead_behind(&e.path), Ok(Some((ahead, _))) if ahead > 0));
    }

    match format {
        ListFormat::Table => {
            for e in &selected {
                // Dim the host/owner path so the repository name stands out when
                // scanning the column. Falls back to the plain rel path.
                match (&e.host, &e.owner, &e.repo) {
                    (Some(host), Some(owner), Some(repo)) => {
                        writeln!(w, "{}{repo}", crate::ui::dim(&format!("{host}/{owner}/")))?;
                    }
                    _ => writeln!(w, "{}", e.rel.display())?,
                }
            }
        }
        ListFormat::Paths => {
            for e in &selected {
                writeln!(w, "{}", e.path.display())?;
            }
        }
        ListFormat::Json => {
            let owned: Vec<RepoEntry> = selected.iter().map(|e| (*e).clone()).collect();
            writeln!(w, "{}", serde_json::to_string_pretty(&owned)?)?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::index::Index;
    use crate::testutil::make_repo;
    use std::path::{Path, PathBuf};

    fn entry(root: &Path, host: &str, owner: &str, repo: &str) -> RepoEntry {
        let rel = PathBuf::from(format!("{host}/{owner}/{repo}"));
        RepoEntry {
            path: root.join(&rel),
            root: root.to_path_buf(),
            rel,
            host: Some(host.to_string()),
            owner: Some(owner.to_string()),
            repo: Some(repo.to_string()),
            remote_url: Some(format!("git@{host}:{owner}/{repo}.git")),
        }
    }

    fn out(index: &Index, filter: &ListFilter, format: ListFormat) -> String {
        let mut buf = Vec::new();
        render(index, filter, format, &mut buf).unwrap();
        String::from_utf8(buf).unwrap()
    }

    #[test]
    fn paths_and_json_shape_and_text_filters() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        let index = Index::from_entries(vec![
            entry(root, "github.com", "alice", "one"),
            entry(root, "gitlab.com", "bob", "two"),
        ]);

        let paths = out(&index, &ListFilter::default(), ListFormat::Paths);
        assert_eq!(paths.lines().count(), 2);
        assert!(paths.contains(root.join("github.com/alice/one").to_str().unwrap()));

        let json = out(&index, &ListFilter::default(), ListFormat::Json);
        let v: serde_json::Value = serde_json::from_str(&json).unwrap();
        let arr = v.as_array().unwrap();
        assert_eq!(arr.len(), 2);
        // Stable order by slug: github.com sorts before gitlab.com.
        assert_eq!(arr[0]["repo"], "one");
        assert!(arr[0]["path"].is_string());

        let by_query = out(
            &index,
            &ListFilter {
                query: Some("BOB/two".to_string()),
                ..Default::default()
            },
            ListFormat::Paths,
        );
        assert_eq!(by_query.lines().count(), 1);
        assert!(by_query.contains("bob/two"));

        let by_host = out(
            &index,
            &ListFilter {
                host: Some("github.com".to_string()),
                ..Default::default()
            },
            ListFormat::Table,
        );
        assert_eq!(by_host.trim(), "github.com/alice/one");
    }

    #[test]
    fn dirty_filter_keeps_only_dirtied_repos() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        make_repo(
            &root.join("github.com/alice/one"),
            Some("git@github.com:alice/one.git"),
        );
        make_repo(
            &root.join("github.com/bob/two"),
            Some("git@github.com:bob/two.git"),
        );
        let index = Index::from_entries(vec![
            entry(root, "github.com", "alice", "one"),
            entry(root, "github.com", "bob", "two"),
        ]);

        let clean = out(
            &index,
            &ListFilter {
                dirty: true,
                ..Default::default()
            },
            ListFormat::Paths,
        );
        assert!(clean.trim().is_empty());

        std::fs::write(root.join("github.com/bob/two/wip.txt"), "x").unwrap();
        let dirty = out(
            &index,
            &ListFilter {
                dirty: true,
                ..Default::default()
            },
            ListFormat::Paths,
        );
        assert_eq!(dirty.lines().count(), 1);
        assert!(dirty.contains("bob/two"));
    }
}
