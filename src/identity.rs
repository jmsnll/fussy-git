//! Parsing a git remote URL into a canonical [`Identity`] — the `(host, owner,
//! repo)` triple that drives where a repository lives on disk.
//!
//! This module deals only with *syntax*. Two concerns layer on top of it
//! elsewhere:
//!
//! * SSH `Host` alias resolution (`git@gh-personal:me/x` to a real host) — the
//!   config layer runs `ssh -G` before an [`Identity`] is computed.
//! * `url.<base>.insteadOf` rewrites and user-defined `rewrite` rules — applied
//!   to the URL string before it reaches [`parse_url`].

use std::fmt;

use thiserror::Error;

/// Errors produced while parsing a remote URL.
#[derive(Debug, Error, PartialEq, Eq)]
pub enum ParseError {
    #[error("empty remote URL")]
    Empty,
    #[error("could not determine a host from {0:?}")]
    NoHost(String),
    #[error("remote {0:?} has no owner/repo path (need at least `owner/repo`)")]
    NoPath(String),
    #[error("invalid port in {0:?}")]
    BadPort(String),
}

/// The transport a remote URL uses. Retained so a caller can choose which
/// scheme to write when re-pointing a remote; parsing itself treats them
/// uniformly.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Scheme {
    Https,
    Http,
    Ssh,
    /// `git://`
    Git,
    /// scp-like shorthand: `git@github.com:owner/repo`
    ScpLike,
}

/// A remote URL broken into its parts, with the path normalised (no leading or
/// trailing `/`, no `.git` suffix).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParsedRemote {
    pub scheme: Scheme,
    pub user: Option<String>,
    pub host: String,
    pub port: Option<u16>,
    pub path: String,
}

/// The canonical identity of a repository: what host it lives on, the owner (one
/// segment for GitHub-style hosts, several for GitLab subgroups) and the repo
/// name.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Identity {
    pub host: String,
    /// One or more segments. `owner.join("/")` is the "group path".
    pub owner: Vec<String>,
    pub repo: String,
}

impl Identity {
    /// The owner segments joined with `/` — a single owner for GitHub, or the
    /// full group path for a GitLab subgroup.
    pub fn owner_path(&self) -> String {
        self.owner.join("/")
    }

    /// `owner/repo` (or `group/subgroup/repo`).
    pub fn project_path(&self) -> String {
        format!("{}/{}", self.owner_path(), self.repo)
    }

    /// Build an identity from a host plus a `owner[/subgroup...]/repo` path.
    pub fn from_host_and_path(host: &str, path: &str) -> Result<Self, ParseError> {
        let host = host.trim().trim_end_matches('/');
        if host.is_empty() {
            return Err(ParseError::NoHost(path.to_string()));
        }
        let mut segs: Vec<String> = split_path(path);
        if segs.len() < 2 {
            return Err(ParseError::NoPath(path.to_string()));
        }
        let repo = segs.pop().expect("len checked >= 2");
        Ok(Identity {
            host: host.to_string(),
            owner: segs,
            repo,
        })
    }

    /// Parse a full remote URL straight into an identity.
    pub fn parse(url: &str) -> Result<Self, ParseError> {
        let parsed = parse_url(url)?;
        Identity::from_host_and_path(&parsed.host, &parsed.path)
    }

    /// Parse a URL *or* a CLI shorthand. Shorthands:
    ///
    /// * `owner/repo`            — uses `default_host`
    /// * `host.tld/owner/repo`   — host taken from the first segment if it looks
    ///   like a hostname (contains a `.`)
    ///
    /// Host *aliases* (`gh:owner/repo`) are expanded by the config layer before
    /// this is called.
    pub fn parse_shorthand(input: &str, default_host: &str) -> Result<Self, ParseError> {
        let input = input.trim();
        if input.is_empty() {
            return Err(ParseError::Empty);
        }
        if looks_like_url(input) {
            return Identity::parse(input);
        }
        let segs = split_path(input);
        match segs.len() {
            0 | 1 => Err(ParseError::NoPath(input.to_string())),
            2 => Identity::from_host_and_path(default_host, input),
            _ => {
                if segs[0].contains('.') {
                    let (host, rest) = (segs[0].clone(), segs[1..].join("/"));
                    Identity::from_host_and_path(&host, &rest)
                } else {
                    Identity::from_host_and_path(default_host, input)
                }
            }
        }
    }
}

impl fmt::Display for Identity {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}/{}", self.host, self.project_path())
    }
}

/// Distinguishes a URL or scp-like target from a bare `owner/repo` shorthand.
fn looks_like_url(s: &str) -> bool {
    s.contains("://") || is_scp_like(s)
}

/// scp-like syntax is identified by a `:` that precedes any `/`, with no `://`.
/// This is the same rule git uses.
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

/// Split a path on `/`, dropping empty segments and a trailing `.git`.
fn split_path(path: &str) -> Vec<String> {
    let trimmed = path.trim().trim_matches('/');
    let trimmed = trimmed.strip_suffix(".git").unwrap_or(trimmed);
    trimmed
        .split('/')
        .filter(|s| !s.is_empty())
        .map(|s| s.to_string())
        .collect()
}

/// Break a remote URL into [`ParsedRemote`]. Understands `https`, `http`, `ssh`,
/// `git` and scp-like (`git@host:path`) forms.
pub fn parse_url(url: &str) -> Result<ParsedRemote, ParseError> {
    let url = url.trim();
    if url.is_empty() {
        return Err(ParseError::Empty);
    }

    if let Some(rest) = url.strip_prefix("ssh://") {
        return parse_authority_form(rest, Scheme::Ssh, url);
    }
    if let Some(rest) = url.strip_prefix("https://") {
        return parse_authority_form(rest, Scheme::Https, url);
    }
    if let Some(rest) = url.strip_prefix("http://") {
        return parse_authority_form(rest, Scheme::Http, url);
    }
    if let Some(rest) = url.strip_prefix("git://") {
        return parse_authority_form(rest, Scheme::Git, url);
    }
    if url.contains("://") {
        // An unmodelled scheme still has a parseable authority; treat it as SSH.
        let rest = url.split_once("://").map(|x| x.1).unwrap_or_default();
        return parse_authority_form(rest, Scheme::Ssh, url);
    }
    if is_scp_like(url) {
        return parse_scp_like(url);
    }

    Err(ParseError::NoHost(url.to_string()))
}

/// Parse the `[user@]host[:port]/path` portion that follows `scheme://`.
fn parse_authority_form(
    rest: &str,
    scheme: Scheme,
    original: &str,
) -> Result<ParsedRemote, ParseError> {
    let (authority, path) = match rest.find('/') {
        Some(i) => (&rest[..i], &rest[i + 1..]),
        None => (rest, ""),
    };

    let (user, host_port) = match authority.rfind('@') {
        Some(i) => (Some(authority[..i].to_string()), &authority[i + 1..]),
        None => (None, authority),
    };

    let (host, port) = split_host_port(host_port, original)?;
    if host.is_empty() {
        return Err(ParseError::NoHost(original.to_string()));
    }

    let path = normalise_path(path);
    if path.is_empty() {
        return Err(ParseError::NoPath(original.to_string()));
    }

    Ok(ParsedRemote {
        scheme,
        user,
        host,
        port,
        path,
    })
}

/// Parse `git@github.com:owner/repo(.git)`. scp-like syntax has no port.
fn parse_scp_like(url: &str) -> Result<ParsedRemote, ParseError> {
    let colon = url.find(':').expect("is_scp_like guaranteed a colon");
    let (authority, path) = (&url[..colon], &url[colon + 1..]);

    let (user, host) = match authority.rfind('@') {
        Some(i) => (Some(authority[..i].to_string()), &authority[i + 1..]),
        None => (None, authority),
    };
    if host.is_empty() {
        return Err(ParseError::NoHost(url.to_string()));
    }

    let path = normalise_path(path);
    if path.is_empty() {
        return Err(ParseError::NoPath(url.to_string()));
    }

    Ok(ParsedRemote {
        scheme: Scheme::ScpLike,
        user,
        host: host.to_string(),
        port: None,
        path,
    })
}

fn split_host_port(s: &str, original: &str) -> Result<(String, Option<u16>), ParseError> {
    match s.rfind(':') {
        Some(i) => {
            let (host, port_str) = (&s[..i], &s[i + 1..]);
            if port_str.is_empty() {
                return Ok((host.to_string(), None));
            }
            let port = port_str
                .parse::<u16>()
                .map_err(|_| ParseError::BadPort(original.to_string()))?;
            Ok((host.to_string(), Some(port)))
        }
        None => Ok((s.to_string(), None)),
    }
}

/// Normalise a URL path into clean owner/repo segments. A leading `~` on a
/// segment becomes `_`, because Bitbucket personal repositories use `~user` and
/// a literal `~` path segment is expanded by shells and editors.
fn normalise_path(path: &str) -> String {
    split_path(path)
        .into_iter()
        .map(|seg| match seg.strip_prefix('~') {
            Some(stripped) if !stripped.is_empty() => format!("_{stripped}"),
            _ => seg,
        })
        .collect::<Vec<_>>()
        .join("/")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn id(host: &str, owner: &[&str], repo: &str) -> Identity {
        Identity {
            host: host.to_string(),
            owner: owner.iter().map(|s| s.to_string()).collect(),
            repo: repo.to_string(),
        }
    }

    #[test]
    fn parses_https() {
        assert_eq!(
            Identity::parse("https://github.com/jmsnll/fussy-git.git").unwrap(),
            id("github.com", &["jmsnll"], "fussy-git")
        );
    }

    #[test]
    fn parses_https_without_git_suffix_or_trailing_slash() {
        assert_eq!(
            Identity::parse("https://github.com/jmsnll/fussy-git/").unwrap(),
            id("github.com", &["jmsnll"], "fussy-git")
        );
    }

    #[test]
    fn parses_scp_like() {
        assert_eq!(
            Identity::parse("git@github.com:jmsnll/fussy-git.git").unwrap(),
            id("github.com", &["jmsnll"], "fussy-git")
        );
    }

    #[test]
    fn parses_ssh_with_port() {
        let parsed = parse_url("ssh://git@git.example.com:2222/team/api.git").unwrap();
        assert_eq!(parsed.host, "git.example.com");
        assert_eq!(parsed.port, Some(2222));
        assert_eq!(parsed.path, "team/api");
        assert_eq!(parsed.scheme, Scheme::Ssh);
    }

    #[test]
    fn parses_git_protocol() {
        assert_eq!(
            Identity::parse("git://github.com/jmsnll/fussy-git.git").unwrap(),
            id("github.com", &["jmsnll"], "fussy-git")
        );
    }

    #[test]
    fn gitlab_subgroups_become_multi_segment_owner() {
        let ident = Identity::parse("https://gitlab.com/acme/backend/team/api.git").unwrap();
        assert_eq!(ident, id("gitlab.com", &["acme", "backend", "team"], "api"));
        assert_eq!(ident.owner_path(), "acme/backend/team");
        assert_eq!(ident.project_path(), "acme/backend/team/api");
    }

    #[test]
    fn ssh_host_alias_is_preserved_as_host_here() {
        // Alias resolution happens in the config layer; syntactically the host
        // is whatever was written.
        let parsed = parse_url("git@gh-personal:me/dotfiles.git").unwrap();
        assert_eq!(parsed.host, "gh-personal");
        assert_eq!(parsed.user.as_deref(), Some("git"));
        assert_eq!(parsed.path, "me/dotfiles");
    }

    #[test]
    fn tilde_segments_are_sanitised() {
        let ident = Identity::parse("https://bitbucket.org/~someuser/thing.git").unwrap();
        assert_eq!(ident, id("bitbucket.org", &["_someuser"], "thing"));
    }

    #[test]
    fn rejects_pathless_url() {
        assert_eq!(
            Identity::parse("https://github.com/"),
            Err(ParseError::NoPath("https://github.com/".to_string()))
        );
    }

    #[test]
    fn rejects_owner_without_repo() {
        assert!(matches!(
            Identity::parse("https://github.com/jmsnll"),
            Err(ParseError::NoPath(_))
        ));
    }

    #[test]
    fn rejects_empty() {
        assert_eq!(parse_url("   "), Err(ParseError::Empty));
    }

    #[test]
    fn shorthand_owner_repo_uses_default_host() {
        assert_eq!(
            Identity::parse_shorthand("jmsnll/fussy-git", "github.com").unwrap(),
            id("github.com", &["jmsnll"], "fussy-git")
        );
    }

    #[test]
    fn shorthand_with_explicit_host() {
        assert_eq!(
            Identity::parse_shorthand("gitlab.com/acme/backend/api", "github.com").unwrap(),
            id("gitlab.com", &["acme", "backend"], "api")
        );
    }

    #[test]
    fn shorthand_falls_through_to_url_parsing() {
        assert_eq!(
            Identity::parse_shorthand("git@github.com:jmsnll/fussy-git.git", "github.com").unwrap(),
            id("github.com", &["jmsnll"], "fussy-git")
        );
    }

    #[test]
    fn is_scp_like_discriminates() {
        assert!(is_scp_like("git@github.com:owner/repo"));
        assert!(!is_scp_like("https://github.com/owner/repo"));
        assert!(!is_scp_like("ssh://git@github.com/owner/repo"));
        assert!(!is_scp_like("owner/repo"));
    }
}
