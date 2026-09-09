//! `fussy-git adopt` — bring an existing checkout under management.
//!
//! Where `get` clones a *new* repository straight into its canonical location,
//! `adopt` takes a repository that already exists somewhere on disk and moves
//! it to where its remote (or an explicit `--as` identity) says it belongs. It
//! is the manual counterpart to `reconcile`: `reconcile` sweeps the managed
//! roots, `adopt` fixes the single repository it is pointed at — including one
//! that has no remote yet, since `--as` adds an `origin` for it.
//!
//! Like `reconcile`, a move here is a pure filesystem rename (with a
//! copy+verify+delete fallback across filesystems, via [`fsops`]); repository
//! contents are never touched.

use std::path::{Path, PathBuf};

use anyhow::{anyhow, bail, Context, Result};

use crate::config;
use crate::fsops;
use crate::git;
use crate::identity::Identity;
use crate::preflight;
use crate::resolve;

/// Options for [`run`], mirroring the CLI flags.
#[derive(Debug, Clone, Default)]
pub struct AdoptOptions {
    /// Force a target identity: `host/owner/repo`, `alias:owner/repo`, or a URL.
    /// When the repo has no `origin`, one is added pointing at this identity.
    pub as_identity: Option<String>,
    /// Move the repo even if its working tree is dirty.
    pub allow_dirty: bool,
    /// Leave a symlink at the old location pointing at the new one.
    pub leave_symlink: bool,
}

/// Adopt the repository at `path`, moving it to its canonical location under
/// `cfg.root`, and return that canonical path.
///
/// A no-op (returning the same path) when the repo is already canonical.
pub fn run(cfg: &config::Config, path: &Path, opts: &AdoptOptions) -> Result<PathBuf> {
    let path =
        std::fs::canonicalize(path).with_context(|| format!("resolving {}", path.display()))?;
    if !path.is_dir() {
        bail!("{} is not a directory", path.display());
    }
    if !git::is_repo_root(&path) {
        bail!("{} is not a git repository", path.display());
    }

    let origin = git::remotes(&path)?
        .into_iter()
        .find(|(n, _)| n == "origin");

    let id = match &opts.as_identity {
        Some(as_value) => {
            let id = resolve::identity_from_target(cfg, as_value)
                .with_context(|| format!("resolving --as {as_value:?}"))?;
            match &origin {
                None => {
                    let url = remote_url_for(cfg, &id);
                    git::run(
                        Some(path.as_path()),
                        ["remote", "add", "origin", url.as_str()],
                    )
                    .with_context(|| format!("adding origin {url}"))?;
                    eprintln!("fussy-git: added origin {url}");
                }
                Some((_, url)) => {
                    if let Ok(existing) = resolve::identity_from_url(url) {
                        if existing != id {
                            eprintln!(
                                "fussy-git: warning: origin ({existing}) resolves to a \
                                 different identity than --as ({id}); using --as"
                            );
                        }
                    }
                }
            }
            id
        }
        None => {
            let (_, url) = git::primary_remote(&path, "origin")?
                .ok_or_else(|| anyhow!("no remote — pass --as <host/owner/repo>"))?;
            resolve::identity_from_url(&url).with_context(|| format!("parsing remote {url:?}"))?
        }
    };

    let root = std::fs::canonicalize(&cfg.root).unwrap_or_else(|_| cfg.root.clone());
    let rel = cfg.template_for(&id.host).render(&id, None);
    let canonical = root.join(rel);

    if path == canonical {
        eprintln!(
            "fussy-git: {} is already at its canonical path",
            path.display()
        );
        return Ok(canonical);
    }
    if canonical.exists() {
        bail!("target already exists: {}", canonical.display());
    }

    let safety = preflight::inspect(&path)?;
    if !safety.movable(opts.allow_dirty) {
        let mut reasons = safety.blockers();
        if !opts.allow_dirty && safety.dirty {
            reasons.push("uncommitted changes (retry with --allow-dirty)".to_string());
        }
        if reasons.is_empty() {
            reasons.push("not safe to move".to_string());
        }
        bail!("cannot adopt {}: {}", path.display(), reasons.join("; "));
    }

    if let Some(parent) = canonical.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("creating {}", parent.display()))?;
    }

    let old_parent = path.parent().map(Path::to_path_buf);
    fsops::move_dir(&path, &canonical)
        .with_context(|| format!("moving {} to {}", path.display(), canonical.display()))?;

    // Tidy now-empty parent directories, never climbing past a managed root.
    let stop = match fsops::managed_root(cfg, &path) {
        Some(m) => Some(m.to_path_buf()),
        None if path.starts_with(&root) => Some(root.clone()),
        None => None,
    };
    if let Some(stop) = stop {
        fsops::prune_empty_dirs(old_parent.as_deref(), &stop);
    }

    if opts.leave_symlink {
        if let Err(e) = std::os::unix::fs::symlink(&canonical, &path) {
            eprintln!(
                "fussy-git: warning: could not leave a symlink at {}: {e}",
                path.display()
            );
        }
    }

    run_post_move_hooks(cfg, &canonical);

    Ok(canonical)
}

/// Build the URL to point a fresh `origin` at, from the host rule — the same
/// ssh-vs-https decision `get` makes when cloning from a shorthand.
///
/// (Deliberately mirrors `get::clone_url`, which is private to that module; the
/// logic is small and duplicating it avoids widening `get`'s API.)
fn remote_url_for(cfg: &config::Config, id: &Identity) -> String {
    let host = &id.host;
    let owner = id.owner_path();
    let repo = &id.repo;

    let ssh = match cfg.host_rule(host) {
        Some(rule) => match rule.clone_scheme.as_deref() {
            Some("ssh") => true,
            Some(_) => false,
            None => rule.ssh,
        },
        None => false,
    };

    if ssh {
        format!("git@{host}:{owner}/{repo}.git")
    } else {
        format!("https://{host}/{owner}/{repo}.git")
    }
}

/// Run `cfg.hooks.post_move` in `dir`. Failures are warnings on stderr, never
/// fatal.
fn run_post_move_hooks(cfg: &config::Config, dir: &Path) {
    for cmd in &cfg.hooks.post_move {
        match std::process::Command::new("sh")
            .arg("-c")
            .arg(cmd)
            .current_dir(dir)
            .status()
        {
            Ok(s) if s.success() => {}
            Ok(s) => eprintln!("fussy-git: post_move hook `{cmd}` exited with {s}"),
            Err(e) => eprintln!("fussy-git: post_move hook `{cmd}` failed to start: {e}"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::make_repo;

    fn cfg_for(tmp: &Path) -> config::Config {
        let p = tmp.join("f.toml");
        let root = tmp.join("dest");
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(&p, format!("root = {root:?}\n")).unwrap();
        config::Config::load_from(&p).unwrap()
    }

    fn canon_root(cfg: &config::Config) -> PathBuf {
        std::fs::canonicalize(&cfg.root).unwrap()
    }

    #[test]
    fn adopts_stray_repo_with_origin() {
        let tmp = tempfile::tempdir().unwrap();
        let cfg = cfg_for(tmp.path());
        let stray = tmp.path().join("stray");
        make_repo(&stray, Some("git@github.com:me/stray.git"));

        let dest = run(&cfg, &stray, &AdoptOptions::default()).unwrap();
        assert_eq!(dest, canon_root(&cfg).join("github.com/me/stray"));
        assert!(git::is_repo_root(&dest));
        assert!(!stray.exists());
    }

    #[test]
    fn adopt_as_with_no_origin_adds_origin_and_moves() {
        let tmp = tempfile::tempdir().unwrap();
        let cfg = cfg_for(tmp.path());
        let local = tmp.path().join("local-only");
        make_repo(&local, None);

        let opts = AdoptOptions {
            as_identity: Some("me/widget".to_string()),
            ..Default::default()
        };
        let dest = run(&cfg, &local, &opts).unwrap();
        assert_eq!(dest, canon_root(&cfg).join("github.com/me/widget"));

        let remotes = git::remotes(&dest).unwrap();
        assert_eq!(remotes.len(), 1);
        assert_eq!(remotes[0].0, "origin");
        // The exact URL string depends on the user's `url.insteadOf` rules; what
        // matters is that it resolves back to the identity we adopted as.
        assert_eq!(
            resolve::identity_from_url(&remotes[0].1)
                .unwrap()
                .to_string(),
            "github.com/me/widget"
        );
    }

    #[test]
    fn already_canonical_is_a_noop() {
        let tmp = tempfile::tempdir().unwrap();
        let cfg = cfg_for(tmp.path());
        let at = canon_root(&cfg).join("github.com/me/here");
        make_repo(&at, Some("git@github.com:me/here.git"));

        let dest = run(&cfg, &at, &AdoptOptions::default()).unwrap();
        assert_eq!(dest, at);
        assert!(git::is_repo_root(&at));
    }

    #[test]
    fn non_repo_path_errors() {
        let tmp = tempfile::tempdir().unwrap();
        let cfg = cfg_for(tmp.path());
        let plain = tmp.path().join("plain");
        std::fs::create_dir_all(&plain).unwrap();

        assert!(run(&cfg, &plain, &AdoptOptions::default()).is_err());
    }

    #[test]
    fn no_remote_without_as_errors() {
        let tmp = tempfile::tempdir().unwrap();
        let cfg = cfg_for(tmp.path());
        let local = tmp.path().join("local");
        make_repo(&local, None);

        let err = run(&cfg, &local, &AdoptOptions::default()).unwrap_err();
        assert!(err.to_string().contains("no remote"), "{err}");
    }

    #[test]
    fn dirty_repo_errors_unless_allow_dirty() {
        let tmp = tempfile::tempdir().unwrap();
        let cfg = cfg_for(tmp.path());
        let stray = tmp.path().join("stray");
        make_repo(&stray, Some("git@github.com:me/stray.git"));
        std::fs::write(stray.join("wip.txt"), "x").unwrap();

        assert!(run(&cfg, &stray, &AdoptOptions::default()).is_err());
        assert!(stray.join(".git").exists(), "repo left in place on error");

        let opts = AdoptOptions {
            allow_dirty: true,
            ..Default::default()
        };
        let dest = run(&cfg, &stray, &opts).unwrap();
        assert_eq!(dest, canon_root(&cfg).join("github.com/me/stray"));
    }

    #[test]
    fn leave_symlink_leaves_a_traversable_link() {
        let tmp = tempfile::tempdir().unwrap();
        let cfg = cfg_for(tmp.path());
        let stray = tmp.path().join("stray");
        make_repo(&stray, Some("git@github.com:me/stray.git"));

        let opts = AdoptOptions {
            leave_symlink: true,
            ..Default::default()
        };
        let dest = run(&cfg, &stray, &opts).unwrap();

        assert!(stray.symlink_metadata().unwrap().file_type().is_symlink());
        assert_eq!(std::fs::read_link(&stray).unwrap(), dest);
        // Traversable: reaching through the link finds the moved repo.
        assert!(stray.join(".git").exists());
    }

    #[test]
    fn as_overrides_mismatched_origin_but_leaves_it_untouched() {
        let tmp = tempfile::tempdir().unwrap();
        let cfg = cfg_for(tmp.path());
        let stray = tmp.path().join("stray");
        make_repo(&stray, Some("git@github.com:me/wrong.git"));

        let opts = AdoptOptions {
            as_identity: Some("gitlab.com/team/right".to_string()),
            ..Default::default()
        };
        let dest = run(&cfg, &stray, &opts).unwrap();
        assert_eq!(dest, canon_root(&cfg).join("gitlab.com/team/right"));
        assert_eq!(
            git::remotes(&dest).unwrap()[0].1,
            "git@github.com:me/wrong.git"
        );
    }

    #[test]
    fn target_already_exists_errors() {
        let tmp = tempfile::tempdir().unwrap();
        let cfg = cfg_for(tmp.path());
        let stray = tmp.path().join("stray");
        make_repo(&stray, Some("git@github.com:me/stray.git"));
        std::fs::create_dir_all(canon_root(&cfg).join("github.com/me/stray")).unwrap();

        let err = run(&cfg, &stray, &AdoptOptions::default()).unwrap_err();
        assert!(err.to_string().contains("already exists"), "{err}");
    }
}
