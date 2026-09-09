//! Turning a remote URL into a canonical [`Identity`], applying the two things
//! that live *outside* pure URL syntax:
//!
//! * **SSH `Host` aliases** — `git@gh-personal:me/x` where `gh-personal` is an
//!   `~/.ssh/config` entry pointing at `github.com`. Resolved with `ssh -G`,
//!   which honours `Match`, `Include` and the rest.
//! * **fussy-git host aliases** — only relevant to CLI shorthand (`gh:me/x`);
//!   [`Config::resolve_host_alias`] handles those first.
//!
//! `url.insteadOf` rewrites are git's concern: the URLs inspected here come
//! from `git remote`, which has already applied them.

use std::collections::HashMap;
use std::sync::Mutex;

use anyhow::Result;

use crate::config::Config;
use crate::identity::{parse_url, Identity, ParseError};

/// Resolve a remote URL string to an [`Identity`], expanding an SSH host alias
/// to its real hostname when one is configured.
pub fn identity_from_url(url: &str) -> Result<Identity, ParseError> {
    let parsed = parse_url(url)?;
    let host = canonical_host(&parsed.host);
    Identity::from_host_and_path(&host, &parsed.path)
}

/// Resolve a CLI target (`owner/repo`, `gh:owner/repo`, a full URL, …) using
/// `cfg` for shorthand host aliases and the default host.
pub fn identity_from_target(cfg: &Config, target: &str) -> Result<Identity, ParseError> {
    let target = target.trim();
    if target.is_empty() {
        return Err(ParseError::Empty);
    }

    // `alias:owner/repo` — a fussy-git host alias, not scp-like (there is no
    // `@`, and the alias resolves to a known host).
    if let Some((maybe_alias, rest)) = target.split_once(':') {
        if !maybe_alias.contains('@')
            && !maybe_alias.is_empty()
            && !rest.starts_with('/')
            && cfg.resolve_host_alias(maybe_alias) != maybe_alias
        {
            let host = cfg.resolve_host_alias(maybe_alias);
            return Identity::from_host_and_path(host, rest);
        }
    }

    if target.contains("://") || is_scp_like(target) {
        return identity_from_url(target);
    }

    // Bare shorthand: `owner/repo`, `host.tld/owner/repo`, or `alias/owner/repo`.
    let first = target.split('/').next().unwrap_or_default();
    let resolved = cfg.resolve_host_alias(first);
    if resolved != first {
        let rest = target.split_once('/').map(|x| x.1).unwrap_or_default();
        return Identity::from_host_and_path(resolved, rest);
    }
    Identity::parse_shorthand(target, &cfg.default_host)
}

fn is_scp_like(s: &str) -> bool {
    if s.contains("://") {
        return false;
    }
    match (s.find(':'), s.find('/')) {
        (Some(c), Some(sl)) => c < sl,
        (Some(_), None) => true,
        _ => false,
    }
}

/// Map a host as written in a URL to its canonical hostname. A host that already
/// looks like a real domain (contains a `.`) is assumed canonical and skips the
/// `ssh -G` call.
pub fn canonical_host(host: &str) -> String {
    if host.contains('.') || host.eq_ignore_ascii_case("localhost") {
        return host.to_string();
    }
    ssh_hostname(host).unwrap_or_else(|| host.to_string())
}

static SSH_CACHE: Mutex<Option<HashMap<String, Option<String>>>> = Mutex::new(None);

/// Ask `ssh -G <alias>` for the real `hostname`. Cached per process. Returns
/// `None` if `ssh` is unavailable or the resolved hostname is unchanged.
fn ssh_hostname(alias: &str) -> Option<String> {
    {
        let mut guard = SSH_CACHE.lock().ok()?;
        let cache = guard.get_or_insert_with(HashMap::new);
        if let Some(hit) = cache.get(alias) {
            return hit.clone();
        }
    }

    let resolved = query_ssh_hostname(alias);
    let stored = match &resolved {
        Some(h) if !h.eq_ignore_ascii_case(alias) => Some(h.clone()),
        _ => None,
    };
    if let Ok(mut guard) = SSH_CACHE.lock() {
        if let Some(cache) = guard.as_mut() {
            cache.insert(alias.to_string(), stored.clone());
        }
    }
    stored
}

fn query_ssh_hostname(alias: &str) -> Option<String> {
    let output = std::process::Command::new("ssh")
        .args(["-G", alias])
        .stdin(std::process::Stdio::null())
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let text = String::from_utf8_lossy(&output.stdout);
    for line in text.lines() {
        let mut it = line.split_whitespace();
        if it.next() == Some("hostname") {
            return it.next().map(|s| s.to_string());
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg_with_alias() -> Config {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(".fussy-git.toml");
        std::fs::write(
            &path,
            r#"
            default_host = "github.com"
            [hosts."gitlab.internal.acme"]
            alias = ["work"]
            template = "acme/{group_path}/{repo}"
            "#,
        )
        .unwrap();
        let c = Config::load_from(&path).unwrap();
        // Leak the tempdir so it outlives the returned Config; acceptable in a test.
        std::mem::forget(dir);
        c
    }

    #[test]
    fn plain_url_identity() {
        let id = identity_from_url("https://github.com/jmsnll/fussy-git.git").unwrap();
        assert_eq!(id.host, "github.com");
        assert_eq!(id.repo, "fussy-git");
    }

    #[test]
    fn target_bare_shorthand_uses_default_host() {
        let c = cfg_with_alias();
        let id = identity_from_target(&c, "jmsnll/fussy-git").unwrap();
        assert_eq!(id.to_string(), "github.com/jmsnll/fussy-git");
    }

    #[test]
    fn target_colon_alias() {
        let c = cfg_with_alias();
        let id = identity_from_target(&c, "work:team/backend/api").unwrap();
        assert_eq!(id.host, "gitlab.internal.acme");
        assert_eq!(id.project_path(), "team/backend/api");
    }

    #[test]
    fn target_slash_alias() {
        let c = cfg_with_alias();
        let id = identity_from_target(&c, "work/team/api").unwrap();
        assert_eq!(id.host, "gitlab.internal.acme");
        assert_eq!(id.project_path(), "team/api");
    }

    #[test]
    fn target_scp_like_url() {
        let c = cfg_with_alias();
        let id = identity_from_target(&c, "git@github.com:jmsnll/fussy-git.git").unwrap();
        assert_eq!(id.to_string(), "github.com/jmsnll/fussy-git");
    }

    #[test]
    fn canonical_host_passes_through_real_domain() {
        assert_eq!(canonical_host("github.com"), "github.com");
    }
}
