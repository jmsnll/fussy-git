//! Typed errors for resolving a query to exactly one repository.
//!
//! `cd`, `remove`, `info` and `worktree add` all take a substring query that
//! must name a single repository. When it does not, the CLI needs more than a
//! message: with `--json` it reports the candidates so a caller (a script, or
//! an agent) can pick one without another round trip. [`MatchError`] carries
//! that structure through `anyhow` and is recovered with `downcast_ref`.

use std::fmt;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::Serialize;

use crate::config::Config;
use crate::{git, index};

/// One repository a query could have meant.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Candidate {
    /// `host/owner/repo`, or the path relative to its root when there is no
    /// usable remote.
    pub slug: String,
    pub path: PathBuf,
}

/// A query matched no repository, or more than one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MatchError {
    NoMatch {
        query: String,
    },
    Ambiguous {
        query: String,
        candidates: Vec<Candidate>,
    },
}

impl fmt::Display for MatchError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            MatchError::NoMatch { query } => write!(f, "no repo matches {query:?}"),
            MatchError::Ambiguous { query, candidates } => {
                writeln!(f, "{query:?} matches {} repositories:", candidates.len())?;
                for c in candidates {
                    writeln!(f, "{}", c.slug)?;
                }
                write!(f, "be more specific")
            }
        }
    }
}

impl std::error::Error for MatchError {}

impl MatchError {
    /// A stable machine-readable code for JSON error output.
    pub fn code(&self) -> &'static str {
        match self {
            MatchError::NoMatch { .. } => "no_match",
            MatchError::Ambiguous { .. } => "ambiguous",
        }
    }
}

/// Reduce `matches` to the single hit, or the appropriate [`MatchError`].
/// Candidates are sorted and de-duplicated by path.
pub fn exactly_one(query: &str, mut matches: Vec<Candidate>) -> Result<Candidate, MatchError> {
    matches.sort_by(|a, b| a.slug.cmp(&b.slug).then_with(|| a.path.cmp(&b.path)));
    matches.dedup_by(|a, b| a.path == b.path);
    match matches.len() {
        0 => Err(MatchError::NoMatch {
            query: query.to_string(),
        }),
        1 => Ok(matches.pop().expect("length checked")),
        _ => Err(MatchError::Ambiguous {
            query: query.to_string(),
            candidates: matches,
        }),
    }
}

/// True when `query` should be read as a filesystem path rather than a name
/// query: `.`, `..`, or anything starting with `/`, `./`, `../` or `~`.
pub fn is_path_like(query: &str) -> bool {
    let q = query.trim();
    q == "." || q == ".." || ["/", "./", "../", "~"].iter().any(|p| q.starts_with(p))
}

/// Resolve `query` to the main repository it names. A path-like query (see
/// [`is_path_like`]) names the repository containing that path — for a linked
/// worktree, the repository that owns it — so `.` means "the repository I am
/// in". Anything else is resolved against the index.
pub fn repo_for(cfg: &Config, query: &str) -> Result<PathBuf> {
    if is_path_like(query) {
        let path = PathBuf::from(shellexpand::tilde(query.trim()).into_owned());
        let top = git::toplevel(&path)
            .with_context(|| format!("{} is not inside a git repository", path.display()))?;
        return git::main_worktree(&top);
    }
    let index = index::load_or_build(cfg).context("loading the repository index")?;
    Ok(index.resolve(query)?.path.clone())
}

/// `path` with symlinks resolved when possible, so paths git reports (always
/// real paths) compare equal to configured ones (`/var` vs `/private/var` on
/// macOS, a symlinked home directory).
pub fn real(path: &Path) -> PathBuf {
    std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cand(slug: &str) -> Candidate {
        Candidate {
            slug: slug.to_string(),
            path: PathBuf::from(format!("/git/{slug}")),
        }
    }

    #[test]
    fn one_match_is_returned() {
        let hit = exactly_one("a", vec![cand("github.com/x/a")]).unwrap();
        assert_eq!(hit.slug, "github.com/x/a");
    }

    #[test]
    fn duplicate_paths_collapse_to_one() {
        let hit = exactly_one("a", vec![cand("github.com/x/a"), cand("github.com/x/a")]).unwrap();
        assert_eq!(hit.slug, "github.com/x/a");
    }

    #[test]
    fn zero_and_many_are_typed() {
        assert_eq!(exactly_one("q", vec![]).unwrap_err().code(), "no_match");
        let err =
            exactly_one("x", vec![cand("github.com/x/b"), cand("github.com/x/a")]).unwrap_err();
        assert_eq!(err.code(), "ambiguous");
        let MatchError::Ambiguous { candidates, .. } = &err else {
            panic!("expected ambiguous");
        };
        assert_eq!(
            candidates[0].slug, "github.com/x/a",
            "candidates are sorted"
        );
        assert!(err.to_string().contains("be more specific"));
    }

    #[test]
    fn path_like_queries() {
        for q in [".", "..", "./x", "../x", "/abs", "~/git/x"] {
            assert!(is_path_like(q), "{q}");
        }
        for q in ["fussy", "jmsnll/fussy-git", "github.com/a/b"] {
            assert!(!is_path_like(q), "{q}");
        }
    }
}
