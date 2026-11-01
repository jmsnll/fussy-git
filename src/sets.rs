//! Named sets of repositories: `[sets]` in the config, addressed as `@name`.
//!
//! A set gives a name to "the system I am working on" — the API, the web
//! client and the infrastructure repo — so `get @payments` clones all of them
//! and `status @payments`, `exec @payments -- …` act on exactly those. Members
//! are ordinary `get` targets, resolved with the same aliases and default host.

use anyhow::{anyhow, Context, Result};

use crate::config::Config;
use crate::identity::Identity;
use crate::resolve;

/// The set name in `token` when it is a set reference (`@name`).
pub fn set_name(token: &str) -> Option<&str> {
    token.trim().strip_prefix('@')
}

/// The members of set `name` exactly as configured, for passing to `get`
/// (which needs the original URL, not just the identity it resolves to).
pub fn targets<'a>(cfg: &'a Config, name: &str) -> Result<&'a [String]> {
    cfg.sets.get(name).map(Vec::as_slice).ok_or_else(|| {
        let known: Vec<String> = cfg.sets.keys().map(|k| format!("@{k}")).collect();
        if known.is_empty() {
            anyhow!("no set named @{name} (no `[sets]` are configured)")
        } else {
            anyhow!("no set named @{name} (known: {})", known.join(", "))
        }
    })
}

/// Resolve every member of set `name` to its identity, in config order.
pub fn members(cfg: &Config, name: &str) -> Result<Vec<Identity>> {
    targets(cfg, name)?
        .iter()
        .map(|t| {
            resolve::identity_from_target(cfg, t)
                .with_context(|| format!("set @{name}: resolving member {t:?}"))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg(body: &str) -> (tempfile::TempDir, Config) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("c.toml");
        std::fs::write(&path, body).unwrap();
        let cfg = Config::load_from(&path).unwrap();
        (dir, cfg)
    }

    #[test]
    fn members_resolve_through_aliases_and_default_host() {
        let (_d, cfg) = cfg(r#"
            [hosts."gitlab.com"]
            alias = ["gl"]
            [sets]
            payments = ["acme/api", "gl:acme/web"]
            "#);
        let ids: Vec<String> = members(&cfg, "payments")
            .unwrap()
            .iter()
            .map(|i| i.to_string())
            .collect();
        assert_eq!(ids, vec!["github.com/acme/api", "gitlab.com/acme/web"]);
    }

    #[test]
    fn unknown_set_lists_known_ones() {
        let (_d, cfg) = cfg("[sets]\na = [\"x/y\"]\n");
        let err = members(&cfg, "b").unwrap_err().to_string();
        assert!(err.contains("@a"), "{err}");
    }

    #[test]
    fn set_reference_syntax() {
        assert_eq!(set_name("@infra"), Some("infra"));
        assert_eq!(set_name("infra"), None);
    }
}
