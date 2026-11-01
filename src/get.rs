//! `fussy-git get` — clone a repository straight into its canonical location.
//!
//! The path is derived from the resolved [`Identity`] and the host's template,
//! exactly as the reconciler would compute it, so a repo fetched with `get`
//! never needs reconciling afterwards. `get` is idempotent: pointed at a repo
//! that is already in place it does nothing (or, with `--update`, fast-forwards
//! it).

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::Serialize;

use crate::config;
use crate::git;
use crate::hooks;
use crate::identity::Identity;
use crate::resolve;

/// Options for [`run`], mirroring the CLI flags.
#[derive(Default)]
pub struct GetOptions {
    /// Clone a specific branch (`git clone --branch`).
    pub branch: Option<String>,
    /// Shallow clone (`git clone --depth 1`).
    pub shallow: bool,
    /// When the repository already exists, fast-forward it instead of doing nothing.
    pub update: bool,
}

/// What [`run`] did.
#[derive(Debug, Clone, Serialize)]
pub struct GetResult {
    /// The canonical path, which now holds the repository.
    pub path: PathBuf,
    pub identity: Identity,
    /// True when this call cloned the repository; false when it already existed.
    pub created: bool,
    /// True when `--update` fast-forwarded an existing repository.
    pub updated: bool,
    /// Non-fatal notes, such as a failing `post_get` hook.
    pub warnings: Vec<String>,
}

/// Clone `target` into its canonical path under `cfg.root`.
///
/// `target` may be a full URL, an scp-like remote, or a shorthand
/// (`owner/repo`, `gh:owner/repo`, `host.tld/owner/repo`).
///
/// The clone is made into a hidden sibling directory and renamed into place, so
/// a failed clone never leaves a half-written repository behind, and two
/// concurrent `get`s of the same target (say, from two agents) cannot
/// interleave: the loser of the rename discards its copy and reports the
/// winner's.
pub fn run(cfg: &config::Config, target: &str, opts: &GetOptions) -> Result<GetResult> {
    let id = resolve::identity_from_target(cfg, target)
        .with_context(|| format!("resolving target {target:?}"))?;
    let path = canonical_path(cfg, &id);

    if path.exists() && git::is_repo_root(&path) {
        let mut updated = false;
        if opts.update {
            eprintln!("fussy-git: updating {}", path.display());
            if !git::run_inherited(Some(&path), ["pull", "--ff-only"])? {
                anyhow::bail!("git pull --ff-only failed in {}", path.display());
            }
            updated = true;
        }
        return Ok(GetResult {
            path,
            identity: id,
            created: false,
            updated,
            warnings: Vec::new(),
        });
    }

    let url = if is_url(target) {
        target.trim().to_string()
    } else {
        clone_url(cfg, &id)
    };

    let mut extra: Vec<&str> = Vec::new();
    if let Some(branch) = &opts.branch {
        extra.push("--branch");
        extra.push(branch);
    }
    if opts.shallow {
        extra.push("--depth");
        extra.push("1");
    }

    let staging = staging_path(&path);
    if let Err(e) = git::clone(&url, &staging, &extra) {
        let _ = std::fs::remove_dir_all(&staging);
        return Err(e.context(format!("cloning {url} into {}", path.display())));
    }

    if let Err(e) = std::fs::rename(&staging, &path) {
        let _ = std::fs::remove_dir_all(&staging);
        if git::is_repo_root(&path) {
            // Someone else finished cloning the same repository first.
            return Ok(GetResult {
                path,
                identity: id,
                created: false,
                updated: false,
                warnings: Vec::new(),
            });
        }
        return Err(anyhow::Error::new(e).context(format!(
            "moving the fresh clone into {} (is something else there?)",
            path.display()
        )));
    }

    let warnings = hooks::run("post_get", &cfg.hooks.post_get, &path);
    for w in &warnings {
        eprintln!("fussy-git: {w}");
    }

    Ok(GetResult {
        path,
        identity: id,
        created: true,
        updated: false,
        warnings,
    })
}

/// Where `id` lives under the primary root.
pub fn canonical_path(cfg: &config::Config, id: &Identity) -> PathBuf {
    cfg.root.join(cfg.template_for(&id.host).render(id, None))
}

/// A hidden, per-process sibling of `path` to clone into. Hidden so that a scan
/// running mid-clone skips it.
fn staging_path(path: &Path) -> PathBuf {
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| "repo".to_string());
    path.with_file_name(format!(".{name}.fussy-git-{}", std::process::id()))
}

/// True when `target` is itself a remote URL rather than a shorthand.
fn is_url(target: &str) -> bool {
    let t = target.trim();
    t.contains("://") || is_scp_like(t)
}

/// scp-like syntax: a `:` before any `/`, and no `://`.
fn is_scp_like(s: &str) -> bool {
    if s.contains("://") {
        return false;
    }
    match (s.find(':'), s.find('/')) {
        (Some(colon), Some(slash)) => colon < slash,
        (Some(_), None) => true,
        _ => false,
    }
}

/// Build a clone URL from the host rule, used when `target` was a shorthand.
///
/// An explicit `clone_scheme` wins; otherwise `ssh = true` selects the scp-like
/// form and everything else (including no host rule at all) falls back to
/// `https`.
fn clone_url(cfg: &config::Config, id: &Identity) -> String {
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::{self, make_repo};

    /// A bare repo on disk seeded with one commit on `main`, plus the
    /// `file://`-scheme URL that resolves to a sensible identity.
    fn bare_remote(tmp: &Path, name: &str) -> String {
        let bare = tmp.join(format!("remotes/{name}.git"));
        std::fs::create_dir_all(&bare).unwrap();
        testutil::git(&bare, &["init", "-q", "--bare", "-b", "main"]);

        let seed = tmp.join(format!("seed-{name}"));
        make_repo(&seed, None);
        // `file://<host><abs path>` — git's file transport ignores the host, but
        // identity parsing gets a real-looking `example.com`.
        let url = format!("file://example.com{}", bare.display());
        testutil::git(&seed, &["remote", "add", "origin", &url]);
        testutil::git(&seed, &["push", "-q", "origin", "main"]);
        url
    }

    fn cfg_for(tmp: &Path) -> config::Config {
        let p = tmp.join("f.toml");
        std::fs::write(&p, format!("root = {:?}\n", tmp.join("dest"))).unwrap();
        config::Config::load_from(&p).unwrap()
    }

    #[test]
    fn clones_into_canonical_path_and_is_idempotent() {
        let tmp = tempfile::tempdir().unwrap();
        let url = bare_remote(tmp.path(), "thing");
        let cfg = cfg_for(tmp.path());

        let first = run(&cfg, &url, &GetOptions::default()).unwrap();
        let dest = first.path.clone();
        assert!(first.created);
        assert!(dest.starts_with(cfg.root.join("example.com")), "{dest:?}");
        assert_eq!(dest.file_name().unwrap(), "thing");
        assert!(git::is_repo_root(&dest));
        assert!(dest.join("README.md").is_file());

        // No staging directory is left beside the clone.
        let leftovers: Vec<_> = std::fs::read_dir(dest.parent().unwrap())
            .unwrap()
            .flatten()
            .filter(|e| e.file_name().to_string_lossy().starts_with('.'))
            .collect();
        assert!(leftovers.is_empty(), "{leftovers:?}");

        // Second call is a no-op returning the same path.
        let again = run(&cfg, &url, &GetOptions::default()).unwrap();
        assert_eq!(again.path, dest);
        assert!(!again.created);
    }

    #[test]
    fn update_fast_forwards_an_existing_clone() {
        let tmp = tempfile::tempdir().unwrap();
        let url = bare_remote(tmp.path(), "proj");
        let cfg = cfg_for(tmp.path());

        let dest = run(&cfg, &url, &GetOptions::default()).unwrap().path;
        assert!(!dest.join("NEW.md").exists());

        // Push a new commit upstream via an independent clone.
        let pusher = tmp.path().join("pusher");
        testutil::git(tmp.path(), &["clone", "-q", &url, pusher.to_str().unwrap()]);
        std::fs::write(pusher.join("NEW.md"), "new\n").unwrap();
        testutil::git(&pusher, &["add", "."]);
        testutil::git(&pusher, &["commit", "-qm", "add new", "--no-verify"]);
        testutil::git(&pusher, &["push", "-q", "origin", "main"]);

        let opts = GetOptions {
            update: true,
            ..Default::default()
        };
        let dest2 = run(&cfg, &url, &opts).unwrap();
        assert_eq!(dest2.path, dest);
        assert!(dest2.updated);
        assert!(
            dest.join("NEW.md").is_file(),
            "update should have pulled the new commit"
        );
    }

    #[test]
    fn shorthand_without_host_rule_builds_https_url() {
        let cfg = config::Config::default();
        let id = resolve::identity_from_target(&cfg, "jmsnll/fussy-git").unwrap();
        assert_eq!(
            clone_url(&cfg, &id),
            "https://github.com/jmsnll/fussy-git.git"
        );
    }

    #[test]
    fn ssh_host_rule_builds_scp_like_url() {
        let tmp = tempfile::tempdir().unwrap();
        let p = tmp.path().join("f.toml");
        std::fs::write(
            &p,
            "root = \"/tmp/x\"\n[hosts.\"github.com\"]\nssh = true\n",
        )
        .unwrap();
        let cfg = config::Config::load_from(&p).unwrap();
        let id = resolve::identity_from_target(&cfg, "jmsnll/fussy-git").unwrap();
        assert_eq!(clone_url(&cfg, &id), "git@github.com:jmsnll/fussy-git.git");
    }
}
