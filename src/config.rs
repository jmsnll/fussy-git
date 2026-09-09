//! Loading and validating `config.toml`.
//!
//! Discovery order (first hit wins):
//!
//! 1. `$FUSSY_GIT_CONFIG`
//! 2. `.fussy-git.toml` in the current directory or any ancestor
//! 3. `$XDG_CONFIG_HOME/fussy-git/config.toml` (falls back to
//!    `~/.config/fussy-git/config.toml`)
//!
//! With no file found, [`Config::default`] applies: root `~/git`, layout
//! `{host}/{owner}/{repo}`, shorthand host `github.com`.
//!
//! This file holds only fussy-git's *own* policy. Anything git already knows —
//! SSH `Host` aliases, `url.insteadOf`, credential helpers — is read live from
//! git/ssh config elsewhere, never mirrored here.

use std::collections::BTreeMap;
use std::env;
use std::path::{Path, PathBuf};

use anyhow::{anyhow, bail, Context, Result};
use globset::{Glob, GlobSet, GlobSetBuilder};
use serde::Deserialize;

use crate::template::{Template, DEFAULT_TEMPLATE};

pub const CONFIG_ENV: &str = "FUSSY_GIT_CONFIG";
pub const PROJECT_CONFIG_NAME: &str = ".fussy-git.toml";

/// What to do when two distinct identities want the same on-disk path (the
/// classic case: a case-insensitive filesystem folding `Owner/Repo` and
/// `owner/repo` together).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum OnCollision {
    /// Refuse and report; make the user decide. The safe default.
    #[default]
    Fail,
    /// Append a disambiguating suffix to the later arrival.
    Suffix,
    /// Leave the colliding repo where it is and carry on.
    Skip,
}

/// Whether `get` on a repo you cannot push to should create a fork.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ForkPolicy {
    #[default]
    Prompt,
    Always,
    Never,
}

/// Per-host overrides, keyed in the file by the canonical host
/// (`[hosts."github.com"]`).
#[derive(Debug, Clone)]
pub struct HostRule {
    pub host: String,
    pub aliases: Vec<String>,
    pub template: Option<Template>,
    /// Prefer rewriting/creating remotes in scp-like SSH form.
    pub ssh: bool,
    /// Scheme to use when cloning (`"ssh"` or `"https"`). `None` leaves the
    /// choice to git and its `insteadOf` rules.
    pub clone_scheme: Option<String>,
}

/// Shell commands run after lifecycle events. Each runs in the repository
/// directory.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Hooks {
    pub post_get: Vec<String>,
    pub post_move: Vec<String>,
    pub post_reconcile: Vec<String>,
}

/// A fully validated configuration.
#[derive(Debug, Clone)]
pub struct Config {
    /// Primary root: where new clones land. Absolute, `~` expanded.
    pub root: PathBuf,
    /// Every managed root (primary first), absolute and de-duplicated. These are
    /// searched by `scan`/`reconcile` but only `root` is written to.
    pub roots: Vec<PathBuf>,
    pub jobs: usize,
    pub default_template: Template,
    /// Host assumed for the bare `owner/repo` shorthand.
    pub default_host: String,
    pub on_collision: OnCollision,
    pub fork: ForkPolicy,
    pub hooks: Hooks,
    hosts: Vec<HostRule>,
    ignore_globs: Vec<String>,
    ignore_set: GlobSet,
    /// Absolute path the config was loaded from, if any.
    pub source: Option<PathBuf>,
}

impl Default for Config {
    fn default() -> Self {
        Config {
            root: default_root(),
            roots: vec![default_root()],
            jobs: default_jobs(),
            default_template: Template::parse(DEFAULT_TEMPLATE)
                .expect("built-in template is valid"),
            default_host: default_host(),
            on_collision: OnCollision::default(),
            fork: ForkPolicy::default(),
            hooks: Hooks::default(),
            hosts: Vec::new(),
            ignore_globs: Vec::new(),
            ignore_set: GlobSet::empty(),
            source: None,
        }
    }
}

impl Config {
    /// Discover and load a config, or fall back to [`Config::default`].
    pub fn load() -> Result<Self> {
        let cwd = env::current_dir().context("determining the current directory")?;
        match discover(&cwd)? {
            Some(path) => Self::load_from(&path),
            None => Ok(Config::default()),
        }
    }

    /// Load a specific config file.
    pub fn load_from(path: &Path) -> Result<Self> {
        let text = std::fs::read_to_string(path)
            .with_context(|| format!("reading config {}", path.display()))?;
        let raw: RawConfig =
            toml::from_str(&text).with_context(|| format!("parsing config {}", path.display()))?;
        let mut config = raw
            .into_config()
            .with_context(|| format!("validating config {}", path.display()))?;
        config.source = Some(path.to_path_buf());
        Ok(config)
    }

    /// Resolve a host token that might be an alias (`gh`, `work`) to its
    /// canonical host. An unknown token is returned unchanged, since it may be a
    /// real hostname that has no rule of its own.
    pub fn resolve_host_alias<'a>(&'a self, token: &'a str) -> &'a str {
        for rule in &self.hosts {
            if rule.host == token || rule.aliases.iter().any(|a| a == token) {
                return &rule.host;
            }
        }
        token
    }

    /// The template for a given canonical host, falling back to the default.
    pub fn template_for(&self, host: &str) -> &Template {
        self.hosts
            .iter()
            .find(|r| r.host == host)
            .and_then(|r| r.template.as_ref())
            .unwrap_or(&self.default_template)
    }

    pub fn host_rule(&self, host: &str) -> Option<&HostRule> {
        self.hosts.iter().find(|r| r.host == host)
    }

    pub fn hosts(&self) -> &[HostRule] {
        &self.hosts
    }

    pub fn ignore_globs(&self) -> &[String] {
        &self.ignore_globs
    }

    /// True if `path` (expected absolute) matches any `ignore` glob. Globs are
    /// matched against the path both as given and with `~` re-collapsed, so
    /// `~/git/scratch/**` works.
    pub fn is_ignored(&self, path: &Path) -> bool {
        if self.ignore_set.is_empty() {
            return false;
        }
        if self.ignore_set.is_match(path) {
            return true;
        }
        if let Some(collapsed) = collapse_home(path) {
            return self.ignore_set.is_match(collapsed);
        }
        false
    }
}

// --- raw (on-disk) shapes -----------------------------------------------------

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawConfig {
    root: Option<String>,
    #[serde(default)]
    roots: Vec<String>,
    jobs: Option<usize>,
    default_template: Option<String>,
    default_host: Option<String>,
    on_collision: Option<OnCollision>,
    fork: Option<ForkPolicy>,
    #[serde(default)]
    ignore: Vec<String>,
    #[serde(default)]
    hosts: BTreeMap<String, RawHost>,
    #[serde(default)]
    hooks: Hooks,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawHost {
    #[serde(default)]
    alias: Vec<String>,
    template: Option<String>,
    #[serde(default)]
    ssh: bool,
    clone_scheme: Option<String>,
}

impl RawConfig {
    fn into_config(self) -> Result<Config> {
        let root = match self.root {
            Some(r) => expand(&r)?,
            None => default_root(),
        };

        let mut roots = vec![root.clone()];
        for r in &self.roots {
            let p = expand(r)?;
            if !roots.contains(&p) {
                roots.push(p);
            }
        }

        let default_template = match self.default_template {
            Some(t) => Template::parse(&t).map_err(|e| anyhow!("default_template: {e}"))?,
            None => Template::parse(DEFAULT_TEMPLATE).expect("built-in template is valid"),
        };

        let mut hosts = Vec::with_capacity(self.hosts.len());
        let mut seen_aliases: BTreeMap<String, String> = BTreeMap::new();
        for (host, raw) in self.hosts {
            let template = match raw.template {
                Some(t) => Some(
                    Template::parse(&t).map_err(|e| anyhow!("hosts.\"{host}\".template: {e}"))?,
                ),
                None => None,
            };
            for alias in &raw.alias {
                if let Some(other) = seen_aliases.insert(alias.clone(), host.clone()) {
                    bail!("alias {alias:?} is claimed by both {other:?} and {host:?}");
                }
            }
            if let Some(scheme) = &raw.clone_scheme {
                if !matches!(scheme.as_str(), "ssh" | "https" | "http" | "git") {
                    bail!(
                        "hosts.\"{host}\".clone_scheme must be ssh|https|http|git, got {scheme:?}"
                    );
                }
            }
            hosts.push(HostRule {
                host,
                aliases: raw.alias,
                template,
                ssh: raw.ssh,
                clone_scheme: raw.clone_scheme,
            });
        }

        let ignore_set = build_glob_set(&self.ignore)?;

        Ok(Config {
            root,
            roots,
            jobs: self.jobs.unwrap_or_else(default_jobs).max(1),
            default_template,
            default_host: self.default_host.unwrap_or_else(default_host),
            on_collision: self.on_collision.unwrap_or_default(),
            fork: self.fork.unwrap_or_default(),
            hooks: self.hooks,
            hosts,
            ignore_globs: self.ignore,
            ignore_set,
            source: None,
        })
    }
}

// --- helpers ----------------------------------------------------------------

fn discover(start: &Path) -> Result<Option<PathBuf>> {
    if let Some(explicit) = env::var_os(CONFIG_ENV) {
        let path = PathBuf::from(explicit);
        if !path.is_file() {
            bail!(
                "{CONFIG_ENV} points at {}, which is not a file",
                path.display()
            );
        }
        return Ok(Some(path));
    }

    for dir in start.ancestors() {
        let candidate = dir.join(PROJECT_CONFIG_NAME);
        if candidate.is_file() {
            return Ok(Some(candidate));
        }
    }

    let xdg = env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .or_else(|| dirs::home_dir().map(|h| h.join(".config")));
    if let Some(base) = xdg {
        let candidate = base.join("fussy-git").join("config.toml");
        if candidate.is_file() {
            return Ok(Some(candidate));
        }
    }

    Ok(None)
}

fn build_glob_set(patterns: &[String]) -> Result<GlobSet> {
    let mut builder = GlobSetBuilder::new();
    for pat in patterns {
        let expanded = shellexpand::tilde(pat).into_owned();
        let glob = Glob::new(&expanded).with_context(|| format!("bad ignore glob {pat:?}"))?;
        builder.add(glob);
        // Also add the raw (unexpanded) form so `~/...` still matches a literal
        // path that was itself given with `~`.
        if expanded != *pat {
            if let Ok(raw) = Glob::new(pat) {
                builder.add(raw);
            }
        }
    }
    builder.build().context("compiling ignore globs")
}

fn expand(path: &str) -> Result<PathBuf> {
    let expanded = shellexpand::full(path).map_err(|e| anyhow!("expanding {path:?}: {e}"))?;
    let p = PathBuf::from(expanded.into_owned());
    if p.is_absolute() {
        Ok(p)
    } else if let Ok(abs) = std::path::absolute(&p) {
        Ok(abs)
    } else {
        Ok(p)
    }
}

/// Re-collapse a leading `$HOME` into `~` so `~/...` ignore globs match.
fn collapse_home(path: &Path) -> Option<PathBuf> {
    let home = dirs::home_dir()?;
    let rest = path.strip_prefix(&home).ok()?;
    Some(PathBuf::from("~").join(rest))
}

fn default_root() -> PathBuf {
    dirs::home_dir()
        .map(|h| h.join("git"))
        .unwrap_or_else(|| PathBuf::from("git"))
}

fn default_host() -> String {
    "github.com".to_string()
}

/// Two workers per core keeps disk and network busy without stampeding a remote
/// host; the cap stops a large machine from opening dozens of SSH connections
/// at once.
fn default_jobs() -> usize {
    std::thread::available_parallelism()
        .map(|n| (n.get() * 2).min(16))
        .unwrap_or(8)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn write_config(body: &str) -> (tempfile::TempDir, PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(".fussy-git.toml");
        let mut f = std::fs::File::create(&path).unwrap();
        f.write_all(body.as_bytes()).unwrap();
        (dir, path)
    }

    #[test]
    fn default_config_is_sane() {
        let c = Config::default();
        assert!(c.root.ends_with("git"));
        assert_eq!(c.roots, vec![c.root.clone()]);
        assert_eq!(c.default_host, "github.com");
        assert_eq!(c.default_template.as_str(), "{host}/{owner}/{repo}");
        assert_eq!(c.on_collision, OnCollision::Fail);
        assert!(c.jobs >= 1);
    }

    #[test]
    fn parses_a_full_config() {
        let (_dir, path) = write_config(
            r#"
            root = "/tmp/git"
            roots = ["/tmp/git", "/tmp/work"]
            jobs = 4
            default_host = "gitlab.com"
            on_collision = "suffix"
            fork = "always"
            ignore = ["**/node_modules/**"]

            [hosts."github.com"]
            alias = ["gh", "github"]
            ssh = true

            [hosts."gitlab.internal.acme"]
            alias = ["work"]
            template = "acme/{group_path}/{repo}"
            clone_scheme = "ssh"

            [hooks]
            post_get = ["direnv allow"]
            "#,
        );
        let c = Config::load_from(&path).unwrap();
        assert_eq!(c.root, PathBuf::from("/tmp/git"));
        assert_eq!(
            c.roots,
            vec![PathBuf::from("/tmp/git"), PathBuf::from("/tmp/work")]
        );
        assert_eq!(c.jobs, 4);
        assert_eq!(c.default_host, "gitlab.com");
        assert_eq!(c.on_collision, OnCollision::Suffix);
        assert_eq!(c.fork, ForkPolicy::Always);
        assert_eq!(c.hooks.post_get, vec!["direnv allow".to_string()]);
        assert_eq!(c.source.as_deref(), Some(path.as_path()));
    }

    #[test]
    fn resolves_host_aliases() {
        let (_dir, path) = write_config(
            r#"
            [hosts."github.com"]
            alias = ["gh", "github"]
            [hosts."gitlab.internal.acme"]
            alias = ["work", "acme"]
            template = "acme/{group_path}/{repo}"
            "#,
        );
        let c = Config::load_from(&path).unwrap();
        assert_eq!(c.resolve_host_alias("gh"), "github.com");
        assert_eq!(c.resolve_host_alias("work"), "gitlab.internal.acme");
        assert_eq!(c.resolve_host_alias("github.com"), "github.com");
        assert_eq!(c.resolve_host_alias("unknown.example"), "unknown.example");
    }

    #[test]
    fn template_falls_back_to_default() {
        let (_dir, path) = write_config(
            r#"
            [hosts."gitlab.internal.acme"]
            template = "acme/{group_path}/{repo}"
            "#,
        );
        let c = Config::load_from(&path).unwrap();
        assert_eq!(
            c.template_for("gitlab.internal.acme").as_str(),
            "acme/{group_path}/{repo}"
        );
        assert_eq!(
            c.template_for("github.com").as_str(),
            "{host}/{owner}/{repo}"
        );
    }

    #[test]
    fn duplicate_alias_is_rejected() {
        let (_dir, path) = write_config(
            r#"
            [hosts."github.com"]
            alias = ["gh"]
            [hosts."gitlab.com"]
            alias = ["gh"]
            "#,
        );
        let err = format!("{:#}", Config::load_from(&path).unwrap_err());
        assert!(err.contains("alias"), "{err}");
    }

    #[test]
    fn rejects_unknown_keys() {
        let (_dir, path) = write_config("nonsense = true\n");
        assert!(Config::load_from(&path).is_err());
    }

    #[test]
    fn rejects_bad_template() {
        let (_dir, path) = write_config("default_template = \"{host}/{owner}\"\n");
        let err = format!("{:#}", Config::load_from(&path).unwrap_err());
        assert!(err.contains("default_template"), "{err}");
    }

    #[test]
    fn ignore_matches_expanded_and_tilde_paths() {
        let (_dir, path) =
            write_config("ignore = [\"~/git/scratch/**\", \"**/node_modules/**\"]\n");
        let c = Config::load_from(&path).unwrap();
        let home = dirs::home_dir().unwrap();
        assert!(c.is_ignored(&home.join("git/scratch/thing")));
        assert!(c.is_ignored(Path::new("/anywhere/node_modules/pkg")));
        assert!(!c.is_ignored(&home.join("git/github.com/a/b")));
    }

    #[test]
    fn discover_walks_up_to_project_config() {
        let (dir, path) = write_config("root = \"/tmp/x\"\n");
        let nested = dir.path().join("a/b/c");
        std::fs::create_dir_all(&nested).unwrap();
        // No env override in this test's environment.
        let found = discover(&nested).unwrap();
        assert_eq!(found.as_deref(), Some(path.as_path()));
    }
}
