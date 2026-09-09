//! A fast listing cache for the managed repositories.
//!
//! [`scan::scan`] is thorough but walks every root from scratch; `list` and the
//! browser want an answer now. This module keeps a JSON snapshot of the last
//! scan on disk and reuses it while it still looks current.
//!
//! Freshness is decided by directory mtimes, never by trusting the file blindly.
//! The cache is valid only when it records exactly the configured roots and,
//! for every root, the recorded mtime still matches the live mtime of that root
//! *and* of each immediate `<root>/<hostdir>`. A new clone lands in (or creates)
//! an owner directory whose parent hostdir mtime changes too, so one level of
//! depth is enough to notice it. Anything else — missing file, unparseable
//! JSON, changed roots, bumped mtime — triggers a full [`build`].

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

use anyhow::Result;
use serde::{Deserialize, Serialize};

use crate::config;
use crate::scan::{self, Discovered};

/// The on-disk cache format version. Bump when the shape below changes.
const CACHE_VERSION: u32 = 1;

/// One managed repository, flattened for cheap listing and JSON output.
#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct RepoEntry {
    /// Absolute path to the repository on disk.
    pub path: PathBuf,
    /// The managed root it was found under.
    pub root: PathBuf,
    /// `path` relative to `root`.
    pub rel: PathBuf,
    /// Canonical host, when a remote could be resolved.
    pub host: Option<String>,
    /// Joined owner path (`jmsnll`, or `acme/backend` for a subgroup).
    pub owner: Option<String>,
    /// Repository name.
    pub repo: Option<String>,
    /// URL of the remote the identity was derived from.
    pub remote_url: Option<String>,
}

impl RepoEntry {
    /// `host/owner/repo`, when every part is known.
    pub fn slug(&self) -> Option<String> {
        match (&self.host, &self.owner, &self.repo) {
            (Some(h), Some(o), Some(r)) => Some(format!("{h}/{o}/{r}")),
            _ => None,
        }
    }

    /// `owner/repo`, when both are known.
    fn owner_repo(&self) -> Option<String> {
        match (&self.owner, &self.repo) {
            (Some(o), Some(r)) => Some(format!("{o}/{r}")),
            _ => None,
        }
    }

    fn from_discovered(d: &Discovered) -> Self {
        let (host, owner, repo) = match &d.identity {
            Some(id) => (
                Some(id.host.clone()),
                Some(id.owner_path()),
                Some(id.repo.clone()),
            ),
            None => (None, None, None),
        };
        RepoEntry {
            path: d.path.clone(),
            root: d.root.clone(),
            rel: d.rel.clone(),
            host,
            owner,
            repo,
            remote_url: d.remote_url.clone(),
        }
    }
}

/// Recorded mtimes for one root and its immediate host directories.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
struct RootMeta {
    root: PathBuf,
    /// Root directory mtime, nanoseconds since the epoch (0 if unavailable).
    mtime_ns: u64,
    /// `<hostdir name> -> mtime_ns` for every immediate subdirectory.
    host_dirs: BTreeMap<String, u64>,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
struct CacheFile {
    version: u32,
    roots: Vec<RootMeta>,
    entries: Vec<RepoEntry>,
}

/// A materialised list of managed repositories.
pub struct Index {
    entries: Vec<RepoEntry>,
}

impl Index {
    pub fn entries(&self) -> &[RepoEntry] {
        &self.entries
    }

    /// Build an index directly from entries, for tests in other modules.
    #[cfg(test)]
    pub(crate) fn from_entries(entries: Vec<RepoEntry>) -> Self {
        Index { entries }
    }

    /// Filter entries. `query` is a case-insensitive substring tested against
    /// both `slug()` and `owner/repo`; `host` and `owner` are exact matches.
    /// The result is stably ordered by `slug()` then path.
    pub fn filter<'a>(
        &'a self,
        query: Option<&str>,
        host: Option<&str>,
        owner: Option<&str>,
    ) -> Vec<&'a RepoEntry> {
        let needle = query.map(|q| q.to_lowercase());
        let mut out: Vec<&RepoEntry> = self
            .entries
            .iter()
            .filter(|e| {
                if let Some(h) = host {
                    if e.host.as_deref() != Some(h) {
                        return false;
                    }
                }
                if let Some(o) = owner {
                    if e.owner.as_deref() != Some(o) {
                        return false;
                    }
                }
                if let Some(n) = &needle {
                    let hay_slug = e.slug().map(|s| s.to_lowercase());
                    let hay_or = e.owner_repo().map(|s| s.to_lowercase());
                    let hit = hay_slug.as_deref().map(|s| s.contains(n)).unwrap_or(false)
                        || hay_or.as_deref().map(|s| s.contains(n)).unwrap_or(false);
                    if !hit {
                        return false;
                    }
                }
                true
            })
            .collect();
        out.sort_by(|a, b| a.slug().cmp(&b.slug()).then_with(|| a.path.cmp(&b.path)));
        out
    }
}

/// Load the cache if it is still valid, otherwise [`build`].
pub fn load_or_build(cfg: &config::Config) -> Result<Index> {
    load_or_build_at(cfg, &cache_path(cfg))
}

/// Force a fresh scan and rewrite the cache.
pub fn build(cfg: &config::Config) -> Result<Index> {
    build_at(cfg, &cache_path(cfg))
}

/// Alias for [`build`]: bypass the cache and rebuild it.
pub fn refresh(cfg: &config::Config) -> Result<Index> {
    build(cfg)
}

fn load_or_build_at(cfg: &config::Config, cache: &Path) -> Result<Index> {
    match read_valid_cache(cfg, cache) {
        Some(index) => Ok(index),
        None => build_at(cfg, cache),
    }
}

fn build_at(cfg: &config::Config, cache: &Path) -> Result<Index> {
    let discovered = scan::scan(cfg)?;
    let entries: Vec<RepoEntry> = discovered.iter().map(RepoEntry::from_discovered).collect();
    let roots: Vec<RootMeta> = cfg.roots.iter().map(|r| root_meta(r)).collect();

    let file = CacheFile {
        version: CACHE_VERSION,
        roots,
        entries: entries.clone(),
    };
    if let Err(e) = write_cache(cache, &file) {
        eprintln!(
            "warning: could not write index cache {}: {e:#}",
            cache.display()
        );
    }

    Ok(Index { entries })
}

fn read_valid_cache(cfg: &config::Config, cache: &Path) -> Option<Index> {
    let text = std::fs::read_to_string(cache).ok()?;
    let file: CacheFile = serde_json::from_str(&text).ok()?;
    if file.version != CACHE_VERSION {
        return None;
    }

    let recorded: Vec<&PathBuf> = file.roots.iter().map(|r| &r.root).collect();
    if recorded.len() != cfg.roots.len() || !recorded.iter().zip(&cfg.roots).all(|(a, b)| *a == b) {
        return None;
    }
    for rm in &file.roots {
        if root_meta(&rm.root) != *rm {
            return None;
        }
    }

    Some(Index {
        entries: file.entries,
    })
}

fn cache_path(cfg: &config::Config) -> PathBuf {
    dirs::cache_dir()
        .map(|d| d.join("fussy-git").join("index.json"))
        .unwrap_or_else(|| cfg.root.join(".fussy-git-index.json"))
}

fn write_cache(path: &Path, file: &CacheFile) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let json = serde_json::to_string_pretty(file)?;
    std::fs::write(path, json)?;
    Ok(())
}

fn dir_mtime_ns(path: &Path) -> u64 {
    std::fs::metadata(path)
        .and_then(|m| m.modified())
        .ok()
        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
        .map(|d| d.as_nanos() as u64)
        .unwrap_or(0)
}

fn root_meta(root: &Path) -> RootMeta {
    let mut host_dirs = BTreeMap::new();
    if let Ok(rd) = std::fs::read_dir(root) {
        for entry in rd.flatten() {
            if entry.file_type().map(|t| t.is_dir()).unwrap_or(false) {
                let name = entry.file_name().to_string_lossy().into_owned();
                host_dirs.insert(name, dir_mtime_ns(&entry.path()));
            }
        }
    }
    RootMeta {
        root: root.to_path_buf(),
        mtime_ns: dir_mtime_ns(root),
        host_dirs,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::make_repo;

    struct Fixture {
        tmp: tempfile::TempDir,
        root: PathBuf,
        cfg: config::Config,
    }

    fn fixture() -> Fixture {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("git");
        std::fs::create_dir_all(&root).unwrap();
        let cfg_path = tmp.path().join(".fussy-git.toml");
        std::fs::write(&cfg_path, format!("root = {root:?}\n")).unwrap();
        let cfg = config::Config::load_from(&cfg_path).unwrap();
        Fixture { tmp, root, cfg }
    }

    fn cache_file(fx: &Fixture) -> PathBuf {
        fx.tmp.path().join("cache/index.json")
    }

    #[test]
    fn build_then_load_returns_same_entries_from_cache() {
        let fx = fixture();
        make_repo(
            &fx.root.join("github.com/me/alpha"),
            Some("git@github.com:me/alpha.git"),
        );
        let cache = cache_file(&fx);
        let built = build_at(&fx.cfg, &cache).unwrap();
        assert_eq!(built.entries().len(), 1);
        assert_eq!(
            built.entries()[0].slug().as_deref(),
            Some("github.com/me/alpha")
        );

        // Tamper the cache with a sentinel entry: a real rescan would drop it, so
        // seeing it back proves the load came straight from the cache file.
        let mut file: CacheFile =
            serde_json::from_str(&std::fs::read_to_string(&cache).unwrap()).unwrap();
        file.entries.push(RepoEntry {
            path: PathBuf::from("/sentinel"),
            root: fx.root.clone(),
            rel: PathBuf::from("sentinel"),
            host: Some("sentinel.example".into()),
            owner: Some("ghost".into()),
            repo: Some("sentinel".into()),
            remote_url: None,
        });
        std::fs::write(&cache, serde_json::to_string(&file).unwrap()).unwrap();

        let loaded = load_or_build_at(&fx.cfg, &cache).unwrap();
        assert!(loaded
            .entries()
            .iter()
            .any(|e| e.repo.as_deref() == Some("sentinel")));
    }

    #[test]
    fn a_new_clone_invalidates_the_cache() {
        let fx = fixture();
        make_repo(
            &fx.root.join("github.com/me/alpha"),
            Some("git@github.com:me/alpha.git"),
        );
        let cache = cache_file(&fx);
        build_at(&fx.cfg, &cache).unwrap();

        // Ensure the clock has advanced past the recorded mtimes.
        std::thread::sleep(std::time::Duration::from_millis(20));
        // A new host directory bumps the root mtime; a new owner bumps the
        // hostdir mtime. Either way the cache must be discarded.
        make_repo(
            &fx.root.join("gitlab.com/team/beta"),
            Some("git@gitlab.com:team/beta.git"),
        );

        let idx = load_or_build_at(&fx.cfg, &cache).unwrap();
        let repos: Vec<&str> = idx
            .entries()
            .iter()
            .filter_map(|e| e.repo.as_deref())
            .collect();
        assert!(repos.contains(&"alpha"), "{repos:?}");
        assert!(repos.contains(&"beta"), "{repos:?}");
    }

    #[test]
    fn changed_roots_invalidate_the_cache() {
        let fx = fixture();
        make_repo(
            &fx.root.join("github.com/me/alpha"),
            Some("git@github.com:me/alpha.git"),
        );
        let cache = cache_file(&fx);
        build_at(&fx.cfg, &cache).unwrap();

        let other = fx.tmp.path().join("git2");
        std::fs::create_dir_all(&other).unwrap();
        let cfg_path = fx.tmp.path().join("other.toml");
        std::fs::write(
            &cfg_path,
            format!(
                "root = {:?}\nroots = [{:?}, {:?}]\n",
                fx.root, fx.root, other
            ),
        )
        .unwrap();
        let cfg2 = config::Config::load_from(&cfg_path).unwrap();

        assert!(read_valid_cache(&cfg2, &cache).is_none());
    }

    #[test]
    fn filter_matches_query_host_and_owner() {
        let fx = fixture();
        make_repo(
            &fx.root.join("github.com/alice/widgets"),
            Some("git@github.com:alice/widgets.git"),
        );
        make_repo(
            &fx.root.join("github.com/bob/widgets"),
            Some("git@github.com:bob/widgets.git"),
        );
        make_repo(
            &fx.root.join("gitlab.com/alice/gadgets"),
            Some("git@gitlab.com:alice/gadgets.git"),
        );
        let idx = build_at(&fx.cfg, &cache_file(&fx)).unwrap();

        let all = idx.filter(None, None, None);
        assert_eq!(all.len(), 3);
        // Stable order by slug.
        assert_eq!(all[0].slug().as_deref(), Some("github.com/alice/widgets"));

        let widgets = idx.filter(Some("WID"), None, None);
        assert_eq!(widgets.len(), 2);

        let by_host = idx.filter(None, Some("gitlab.com"), None);
        assert_eq!(by_host.len(), 1);
        assert_eq!(by_host[0].repo.as_deref(), Some("gadgets"));

        let by_owner = idx.filter(None, None, Some("alice"));
        assert_eq!(by_owner.len(), 2);

        let combined = idx.filter(Some("gadgets"), Some("gitlab.com"), Some("alice"));
        assert_eq!(combined.len(), 1);

        assert!(idx.filter(Some("nope"), None, None).is_empty());
    }

    #[test]
    fn falls_back_to_build_when_cache_missing() {
        let fx = fixture();
        make_repo(
            &fx.root.join("github.com/me/alpha"),
            Some("git@github.com:me/alpha.git"),
        );
        let missing = fx.tmp.path().join("no/such/index.json");
        let idx = load_or_build_at(&fx.cfg, &missing).unwrap();
        assert_eq!(idx.entries().len(), 1);
        assert!(missing.is_file(), "build should have written the cache");
    }
}
