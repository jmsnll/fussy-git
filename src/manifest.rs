//! Loading a repository manifest (`repos.toml`) and writing one from the current
//! tree.
//!
//! The manifest is to `fussy-git` what a `Brewfile` is to Homebrew: one file,
//! checked into your dotfiles, that lists the repositories a machine should
//! have. [`fussy_git::sync`](crate::sync) makes the tree match it.
//!
//! Discovery order (first hit wins), mirroring [`config`](crate::config):
//!
//! 1. an explicit `--manifest <path>`
//! 2. `$FUSSY_GIT_MANIFEST`
//! 3. `repos.toml` in the current directory or any ancestor
//! 4. `$XDG_CONFIG_HOME/fussy-git/repos.toml` (falls back to
//!    `~/.config/fussy-git/repos.toml`)
//!
//! v1 is deliberately minimal: a flat `repos = [ "..." ]` array of targets in
//! any form [`get`](crate::get) already accepts. Per-repo tables (`branch`,
//! `remotes`, `tags`) are a later addition. The manifest never carries
//! executable content — lifecycle hooks stay in the machine-local `config.toml`.

use std::env;
use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};
use serde::Deserialize;

use crate::config::Config;
use crate::scan::{self, Class};

pub const MANIFEST_ENV: &str = "FUSSY_GIT_MANIFEST";
pub const PROJECT_MANIFEST_NAME: &str = "repos.toml";

/// One entry in the manifest: a repository the machine should have.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    /// The target as written: `owner/repo`, `host/owner/repo`, `alias:owner/repo`
    /// or any URL form — whatever [`get`](crate::get) accepts.
    pub target: String,
}

/// A parsed manifest.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Manifest {
    pub entries: Vec<Entry>,
    /// Absolute path the manifest was loaded from, if any.
    pub source: Option<PathBuf>,
}

impl Manifest {
    /// Parse manifest text. Unknown keys are rejected, matching `config.rs`.
    pub fn parse(text: &str) -> Result<Self> {
        let raw: RawManifest = toml::from_str(text).context("parsing manifest")?;
        let entries = raw
            .repos
            .into_iter()
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .map(|target| Entry { target })
            .collect();
        Ok(Manifest {
            entries,
            source: None,
        })
    }

    /// Load a specific manifest file.
    pub fn load_from(path: &Path) -> Result<Self> {
        let text = std::fs::read_to_string(path)
            .with_context(|| format!("reading manifest {}", path.display()))?;
        let mut manifest =
            Self::parse(&text).with_context(|| format!("in manifest {}", path.display()))?;
        manifest.source = Some(path.to_path_buf());
        Ok(manifest)
    }

    /// Discover and load a manifest. Returns `Ok(None)` when no manifest exists
    /// anywhere in the search path and none was requested explicitly.
    pub fn discover(cli_override: Option<&Path>) -> Result<Option<Self>> {
        let env_manifest = env::var_os(MANIFEST_ENV).map(PathBuf::from);
        let cwd = env::current_dir().context("determining the current directory")?;
        Self::discover_with(
            cli_override,
            env_manifest.as_deref(),
            &cwd,
            xdg_config_home().as_deref(),
        )
    }

    /// The discovery logic with every ambient input passed in explicitly, so it
    /// can be exercised without touching process-global state.
    fn discover_with(
        cli_override: Option<&Path>,
        env_manifest: Option<&Path>,
        start: &Path,
        xdg_config: Option<&Path>,
    ) -> Result<Option<Self>> {
        if let Some(path) = cli_override {
            if !path.is_file() {
                bail!("--manifest {} is not a file", path.display());
            }
            return Ok(Some(Self::load_from(path)?));
        }

        if let Some(path) = env_manifest {
            if !path.is_file() {
                bail!(
                    "{MANIFEST_ENV} points at {}, which is not a file",
                    path.display()
                );
            }
            return Ok(Some(Self::load_from(path)?));
        }

        for dir in start.ancestors() {
            let candidate = dir.join(PROJECT_MANIFEST_NAME);
            if candidate.is_file() {
                return Ok(Some(Self::load_from(&candidate)?));
            }
        }

        if let Some(base) = xdg_config {
            let candidate = base.join("fussy-git").join(PROJECT_MANIFEST_NAME);
            if candidate.is_file() {
                return Ok(Some(Self::load_from(&candidate)?));
            }
        }

        Ok(None)
    }
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawManifest {
    #[serde(default)]
    repos: Vec<String>,
}

fn xdg_config_home() -> Option<PathBuf> {
    env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .or_else(|| dirs::home_dir().map(|h| h.join(".config")))
}

/// Walk the managed tree and render a manifest for it, using the shortest
/// unambiguous target form for each repository (`owner/repo` when the host is
/// `default_host`, otherwise `host/owner/repo`).
///
/// Output is sorted and de-duplicated, one entry per line, so re-running
/// produces a minimal diff. v1 records identities only — not the checked-out
/// branch or extra remotes.
pub fn dump_tree(cfg: &Config) -> Result<String> {
    let discovered = scan::scan(cfg)?;

    let mut targets: Vec<String> = discovered
        .iter()
        .filter(|d| !matches!(d.class, Class::LinkedWorktree))
        .filter_map(|d| d.identity.as_ref())
        .map(|id| {
            if id.host == cfg.default_host {
                id.project_path()
            } else {
                format!("{}/{}", id.host, id.project_path())
            }
        })
        .collect();
    targets.sort();
    targets.dedup();

    let mut out = String::from("repos = [\n");
    for target in &targets {
        out.push_str("  ");
        out.push_str(&escape_toml_string(target));
        out.push_str(",\n");
    }
    out.push_str("]\n");
    Ok(out)
}

/// A TOML basic string. Target strings are a tame `[A-Za-z0-9._/@:~+-]` subset in
/// practice, but escape defensively so `dump` output is always valid TOML.
fn escape_toml_string(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for ch in s.chars() {
        match ch {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\t' => out.push_str("\\t"),
            '\r' => out.push_str("\\r"),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::make_repo;

    #[test]
    fn parses_a_flat_repos_array() {
        let m = Manifest::parse(
            r#"
            repos = [
              "jmsnll/fussy-git",
              "github.com/rust-lang/rust",
              "gh:jmsnll/dotfiles",
              "git@gitlab.com:acme/backend/service.git",
            ]
            "#,
        )
        .unwrap();
        assert_eq!(m.entries.len(), 4);
        assert_eq!(m.entries[0].target, "jmsnll/fussy-git");
        assert_eq!(
            m.entries[3].target,
            "git@gitlab.com:acme/backend/service.git"
        );
    }

    #[test]
    fn empty_manifest_is_valid() {
        assert!(Manifest::parse("").unwrap().entries.is_empty());
        assert!(Manifest::parse("repos = []\n").unwrap().entries.is_empty());
    }

    #[test]
    fn blank_entries_are_dropped() {
        let m = Manifest::parse("repos = [\"a/b\", \"  \", \"c/d\"]\n").unwrap();
        assert_eq!(
            m.entries
                .iter()
                .map(|e| e.target.as_str())
                .collect::<Vec<_>>(),
            vec!["a/b", "c/d"]
        );
    }

    #[test]
    fn rejects_unknown_keys() {
        assert!(Manifest::parse("nonsense = true\n").is_err());
        // `[[repo]]` tables are a v2 addition; until then they are rejected
        // loudly rather than silently ignored.
        assert!(Manifest::parse("[[repo]]\nurl = \"a/b\"\n").is_err());
    }

    #[test]
    fn load_from_records_the_source_path() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("repos.toml");
        std::fs::write(&path, "repos = [\"me/thing\"]\n").unwrap();
        let m = Manifest::load_from(&path).unwrap();
        assert_eq!(m.source.as_deref(), Some(path.as_path()));
    }

    #[test]
    fn discover_walks_up_to_a_project_manifest() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("repos.toml");
        std::fs::write(&path, "repos = [\"me/thing\"]\n").unwrap();
        let nested = dir.path().join("a/b");
        std::fs::create_dir_all(&nested).unwrap();

        let found = Manifest::discover_with(None, None, &nested, None)
            .unwrap()
            .unwrap();
        assert_eq!(found.entries.len(), 1);
    }

    #[test]
    fn discover_prefers_the_cli_override() {
        let dir = tempfile::tempdir().unwrap();
        let project = dir.path().join("repos.toml");
        std::fs::write(&project, "repos = [\"from/project\"]\n").unwrap();
        let explicit = dir.path().join("other.toml");
        std::fs::write(&explicit, "repos = [\"from/flag\", \"and/another\"]\n").unwrap();

        let found = Manifest::discover_with(Some(&explicit), None, dir.path(), None)
            .unwrap()
            .unwrap();
        assert_eq!(found.entries.len(), 2);
        assert_eq!(found.entries[0].target, "from/flag");
    }

    #[test]
    fn discover_falls_back_to_xdg_then_returns_none() {
        let dir = tempfile::tempdir().unwrap();
        let empty = dir.path().join("empty");
        std::fs::create_dir_all(&empty).unwrap();
        assert!(Manifest::discover_with(None, None, &empty, Some(&empty))
            .unwrap()
            .is_none());

        let xdg = dir.path().join("cfg");
        std::fs::create_dir_all(xdg.join("fussy-git")).unwrap();
        std::fs::write(xdg.join("fussy-git/repos.toml"), "repos = [\"x/y\"]\n").unwrap();
        let found = Manifest::discover_with(None, None, &empty, Some(&xdg))
            .unwrap()
            .unwrap();
        assert_eq!(found.entries.len(), 1);
    }

    #[test]
    fn discover_rejects_a_missing_env_manifest() {
        let dir = tempfile::tempdir().unwrap();
        let missing = dir.path().join("nope.toml");
        let err = Manifest::discover_with(None, Some(&missing), dir.path(), None).unwrap_err();
        assert!(format!("{err:#}").contains("FUSSY_GIT_MANIFEST"));
    }

    #[test]
    fn dump_uses_shortest_form_and_is_sorted() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("git");
        std::fs::create_dir_all(&root).unwrap();
        let cfg_path = tmp.path().join(".fussy-git.toml");
        std::fs::write(&cfg_path, format!("root = {root:?}\n")).unwrap();
        let cfg = Config::load_from(&cfg_path).unwrap();

        make_repo(
            &root.join("github.com/jmsnll/fussy-git"),
            Some("git@github.com:jmsnll/fussy-git.git"),
        );
        make_repo(
            &root.join("gitlab.com/acme/backend/api"),
            Some("https://gitlab.com/acme/backend/api.git"),
        );

        let text = dump_tree(&cfg).unwrap();
        assert_eq!(
            text,
            "repos = [\n  \"gitlab.com/acme/backend/api\",\n  \"jmsnll/fussy-git\",\n]\n"
        );

        // Re-parsing the dump yields the same set of targets.
        let reparsed = Manifest::parse(&text).unwrap();
        assert_eq!(reparsed.entries.len(), 2);
    }
}
