//! `fussy-git get` — clone a repository straight into its canonical location.
//!
//! The path is derived from the resolved [`Identity`] and the host's template,
//! exactly as the reconciler would compute it, so a repo fetched with `get`
//! never needs reconciling afterwards. `get` is idempotent: pointed at a repo
//! that is already in place it does nothing (or, with `--update`, fast-forwards
//! it).

use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context, Result};

use crate::config;
use crate::git;
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

/// Clone `target` into its canonical path under `cfg.root` and return that path.
///
/// `target` may be a full URL, an scp-like remote, or a shorthand
/// (`owner/repo`, `gh:owner/repo`, `host.tld/owner/repo`).
pub fn run(cfg: &config::Config, target: &str, opts: &GetOptions) -> Result<PathBuf> {
    let id = resolve::identity_from_target(cfg, target)
        .with_context(|| format!("resolving target {target:?}"))?;

    let rel = cfg.template_for(&id.host).render(&id, None);
    let path = cfg.root.join(rel);

    if path.exists() && git::is_repo_root(&path) {
        if opts.update {
            eprintln!("fussy-git: updating {}", path.display());
            if !git::run_inherited(Some(&path), ["pull", "--ff-only"])? {
                anyhow::bail!("git pull --ff-only failed in {}", path.display());
            }
        }
        return Ok(path);
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

    git::clone(&url, &path, &extra)
        .with_context(|| format!("cloning {url} into {}", path.display()))?;

    for warning in post_get_hooks(cfg, &path) {
        eprintln!("fussy-git: {warning}");
    }

    Ok(path)
}

/// What [`clone_into`] did.
pub enum CloneOutcome {
    /// A fresh clone landed at this path.
    Cloned(PathBuf),
    /// A repository was already checked out at the canonical path; nothing done.
    AlreadyPresent(PathBuf),
}

/// Clone `id` into its canonical path under `cfg.root`, **capturing** git's
/// output rather than streaming it. `url` overrides the derived clone URL, used
/// when a manifest entry was itself a URL. Shared by
/// [`sync`](crate::sync); the CLI `get` keeps its progress-streaming path.
///
/// Hooks are *not* run here — the caller decides, so a parallel batch can
/// collect hook warnings into its report instead of interleaving them on stderr.
pub fn clone_into(
    cfg: &config::Config,
    id: &Identity,
    url: Option<&str>,
    opts: &GetOptions,
) -> Result<CloneOutcome> {
    let rel = cfg.template_for(&id.host).render(id, None);
    let path = cfg.root.join(rel);

    if path.exists() && git::is_repo_root(&path) {
        return Ok(CloneOutcome::AlreadyPresent(path));
    }

    let url = match url {
        Some(u) => u.trim().to_string(),
        None => clone_url(cfg, id),
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

    git::clone_captured(&url, &path, &extra)
        .with_context(|| format!("cloning {url} into {}", path.display()))?;

    Ok(CloneOutcome::Cloned(path))
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

/// Run each `post_get` hook in the freshly cloned repo, returning any failures
/// as warning strings. A hook failure is never fatal.
pub fn post_get_hooks(cfg: &config::Config, dir: &Path) -> Vec<String> {
    let mut warnings = Vec::new();
    for cmd in &cfg.hooks.post_get {
        match Command::new("sh")
            .arg("-c")
            .arg(cmd)
            .current_dir(dir)
            .status()
        {
            Ok(status) if status.success() => {}
            Ok(status) => warnings.push(format!("post_get hook {cmd:?} exited with {status}")),
            Err(e) => warnings.push(format!("post_get hook {cmd:?} failed to start: {e}")),
        }
    }
    warnings
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

        let dest = run(&cfg, &url, &GetOptions::default()).unwrap();
        assert!(dest.starts_with(cfg.root.join("example.com")), "{dest:?}");
        assert_eq!(dest.file_name().unwrap(), "thing");
        assert!(git::is_repo_root(&dest));
        assert!(dest.join("README.md").is_file());

        // Second call is a no-op returning the same path.
        let again = run(&cfg, &url, &GetOptions::default()).unwrap();
        assert_eq!(again, dest);
    }

    #[test]
    fn update_fast_forwards_an_existing_clone() {
        let tmp = tempfile::tempdir().unwrap();
        let url = bare_remote(tmp.path(), "proj");
        let cfg = cfg_for(tmp.path());

        let dest = run(&cfg, &url, &GetOptions::default()).unwrap();
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
        assert_eq!(dest2, dest);
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
