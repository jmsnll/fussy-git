//! Walking the managed roots to discover repositories and classify each one
//! against where it *should* live.
//!
//! Cross-repo verdicts (`Duplicate`, `Collision`) are not decided here — they
//! need the whole set and are computed by the reconciler. `scan` produces the
//! per-repo facts it builds on.

use std::path::{Path, PathBuf};

use anyhow::Result;
use walkdir::WalkDir;

use crate::config::Config;
use crate::git;
use crate::identity::Identity;
use crate::resolve::identity_from_url;

/// How a discovered repo relates to its canonical location.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Class {
    /// Sitting exactly where its remote says it should.
    Ok,
    /// Wrong place; `canonical_rel` is where it belongs (relative to a root).
    Misplaced { canonical_rel: PathBuf },
    /// No remote at all — never moved automatically; a candidate for `adopt`.
    NoRemote,
    /// Has a remote, but its URL could not be parsed into an identity.
    UnparseableRemote { url: String, reason: String },
    /// A linked worktree — recognised so it is never treated as a stray clone.
    LinkedWorktree,
}

/// One repository found under a managed root.
#[derive(Debug, Clone)]
pub struct Discovered {
    /// Absolute path to the repo.
    pub path: PathBuf,
    /// The managed root it was found under.
    pub root: PathBuf,
    /// `path` relative to `root`.
    pub rel: PathBuf,
    /// Name of the remote used to derive the identity, for example `origin`.
    pub remote_name: Option<String>,
    /// URL of that remote.
    pub remote_url: Option<String>,
    /// Canonical identity, when a remote could be resolved.
    pub identity: Option<Identity>,
    pub class: Class,
}

impl Discovered {
    /// Canonical path this repo should occupy, absolute, under the primary root.
    pub fn canonical_path(&self, primary_root: &Path) -> Option<PathBuf> {
        match &self.class {
            Class::Misplaced { canonical_rel } => Some(primary_root.join(canonical_rel)),
            Class::Ok => Some(self.path.clone()),
            _ => None,
        }
    }
}

/// Discover and classify every repo under `cfg.roots`.
pub fn scan(cfg: &Config) -> Result<Vec<Discovered>> {
    let mut found = Vec::new();
    for root in &cfg.roots {
        if !root.is_dir() {
            continue;
        }
        scan_root(cfg, root, &mut found);
    }
    Ok(found)
}

fn scan_root(cfg: &Config, root: &Path, out: &mut Vec<Discovered>) {
    let mut it = WalkDir::new(root).follow_links(false).into_iter();
    while let Some(entry) = it.next() {
        let Ok(entry) = entry else { continue };
        if !entry.file_type().is_dir() {
            continue;
        }
        let dir = entry.path();

        if dir != root && should_skip_dir(cfg, dir) {
            it.skip_current_dir();
            continue;
        }

        if git::is_repo_root(dir) {
            out.push(classify(cfg, root, dir));
            // Do not descend into a repository. Submodules and nested checkouts
            // belong to the superproject, not to fussy-git.
            it.skip_current_dir();
        }
    }
}

fn should_skip_dir(cfg: &Config, dir: &Path) -> bool {
    if cfg.is_ignored(dir) {
        return true;
    }
    match dir.file_name().and_then(|n| n.to_str()) {
        Some(name) => name.starts_with('.') || name == "node_modules",
        None => false,
    }
}

fn classify(cfg: &Config, root: &Path, dir: &Path) -> Discovered {
    let rel = dir.strip_prefix(root).unwrap_or(dir).to_path_buf();
    let mut d = Discovered {
        path: dir.to_path_buf(),
        root: root.to_path_buf(),
        rel: rel.clone(),
        remote_name: None,
        remote_url: None,
        identity: None,
        class: Class::NoRemote,
    };

    if git::is_linked_worktree(dir) {
        d.class = Class::LinkedWorktree;
        return d;
    }

    let preferred = git::default_remote_name(dir).unwrap_or_else(|| "origin".to_string());
    let primary = git::primary_remote(dir, &preferred).unwrap_or_default();

    let Some((name, url)) = primary else {
        d.class = Class::NoRemote;
        return d;
    };
    d.remote_name = Some(name);
    d.remote_url = Some(url.clone());

    match identity_from_url(&url) {
        Ok(identity) => {
            let template = cfg.template_for(&identity.host);
            let canonical_rel = PathBuf::from(template.render(&identity, None));
            d.class = if rel == canonical_rel {
                Class::Ok
            } else {
                Class::Misplaced { canonical_rel }
            };
            d.identity = Some(identity);
        }
        Err(e) => {
            d.class = Class::UnparseableRemote {
                url: url.clone(),
                reason: e.to_string(),
            };
        }
    }

    d
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::{add_worktree, make_repo};
    use std::fs;

    struct Fixture {
        _tmp: tempfile::TempDir,
        root: PathBuf,
        cfg: Config,
    }

    fn fixture() -> Fixture {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("git");
        fs::create_dir_all(&root).unwrap();
        let cfg_path = tmp.path().join(".fussy-git.toml");
        fs::write(
            &cfg_path,
            format!("root = {:?}\nignore = [\"**/skipme/**\"]\n", root),
        )
        .unwrap();
        let cfg = Config::load_from(&cfg_path).unwrap();
        Fixture {
            _tmp: tmp,
            root,
            cfg,
        }
    }

    #[test]
    fn classifies_correct_misplaced_and_no_remote() {
        let fx = fixture();
        make_repo(
            &fx.root.join("github.com/jmsnll/right"),
            Some("git@github.com:jmsnll/right.git"),
        );
        make_repo(
            &fx.root.join("wrong-place"),
            Some("git@github.com:jmsnll/wrong.git"),
        );
        make_repo(&fx.root.join("local-only"), None);

        let mut found = scan(&fx.cfg).unwrap();
        found.sort_by_key(|d| d.rel.clone());

        let by_rel = |r: &str| {
            found
                .iter()
                .find(|d| d.rel == Path::new(r))
                .unwrap_or_else(|| panic!("missing {r}"))
        };

        assert_eq!(by_rel("github.com/jmsnll/right").class, Class::Ok);
        assert_eq!(
            by_rel("wrong-place").class,
            Class::Misplaced {
                canonical_rel: PathBuf::from("github.com/jmsnll/wrong")
            }
        );
        assert_eq!(by_rel("local-only").class, Class::NoRemote);
    }

    #[test]
    fn does_not_descend_into_repos_or_ignored_dirs() {
        let fx = fixture();
        make_repo(
            &fx.root.join("github.com/me/outer"),
            Some("git@github.com:me/outer.git"),
        );
        // A nested repo inside the outer one must not be reported.
        make_repo(
            &fx.root.join("github.com/me/outer/vendor/inner"),
            Some("git@github.com:me/inner.git"),
        );
        make_repo(
            &fx.root.join("skipme/hidden"),
            Some("git@github.com:me/hidden.git"),
        );

        let found = scan(&fx.cfg).unwrap();
        let rels: Vec<_> = found.iter().map(|d| d.rel.clone()).collect();
        assert_eq!(rels, vec![PathBuf::from("github.com/me/outer")]);
    }

    #[test]
    fn linked_worktree_is_flagged_not_treated_as_clone() {
        let fx = fixture();
        let main = fx.root.join("github.com/me/proj");
        make_repo(&main, Some("git@github.com:me/proj.git"));
        let wt = fx.root.join("worktrees/proj-feature");
        add_worktree(&main, &wt, "feature");

        let found = scan(&fx.cfg).unwrap();
        let wt_entry = found
            .iter()
            .find(|d| d.path == wt || d.rel == Path::new("worktrees/proj-feature"))
            .expect("worktree discovered");
        assert_eq!(wt_entry.class, Class::LinkedWorktree);
    }

    #[test]
    fn unparseable_remote_is_reported() {
        let fx = fixture();
        make_repo(&fx.root.join("weird"), Some("not-a-url"));
        let found = scan(&fx.cfg).unwrap();
        let weird = found.iter().find(|d| d.rel == Path::new("weird")).unwrap();
        assert!(matches!(weird.class, Class::UnparseableRemote { .. }));
    }
}
