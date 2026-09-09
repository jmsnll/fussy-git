//! Path templates: turning an [`Identity`] into the relative path where its
//! repository belongs, and — the other direction — reading an identity back out
//! of a path so the reconciler can tell "correct", "misplaced" and "foreign"
//! apart.
//!
//! A template is a `/`-separated string of literal segments and placeholders:
//!
//! | placeholder     | value                                             |
//! |-----------------|---------------------------------------------------|
//! | `{host}`        | `github.com`                                       |
//! | `{owner}`       | owner path — `jmsnll`, or `acme/backend` for a subgroup |
//! | `{group_path}`  | alias for `{owner}`                                |
//! | `{repo}`        | `fussy-git`                                        |
//! | `{port}`        | remote port, or empty when the remote has none     |
//!
//! `{owner}` / `{group_path}` may span multiple path segments; every other
//! placeholder is a single segment.

use std::fmt;

use regex::Regex;
use thiserror::Error;

use crate::identity::Identity;

pub const DEFAULT_TEMPLATE: &str = "{host}/{owner}/{repo}";

#[derive(Debug, Error, PartialEq, Eq)]
pub enum TemplateError {
    #[error("template is empty")]
    Empty,
    #[error("unknown placeholder {{{0}}} in template")]
    UnknownPlaceholder(String),
    #[error("unbalanced braces in template {0:?}")]
    UnbalancedBraces(String),
    #[error("template {0:?} must contain {{repo}}")]
    MissingRepo(String),
    #[error("template has no {{owner}}/{{group_path}}, cannot place a repo uniquely")]
    MissingOwner,
}

/// A parsed, validated path template.
#[derive(Debug, Clone)]
pub struct Template {
    raw: String,
    parts: Vec<Part>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum Part {
    Literal(String),
    Host,
    Owner,
    Repo,
    Port,
}

/// The identity parts a template recovered from a path. Every field is
/// `Option` because a template need not mention every placeholder; a
/// host-specific template often omits `{host}`, for example.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Captured {
    pub host: Option<String>,
    pub owner: Option<Vec<String>>,
    pub repo: Option<String>,
    pub port: Option<u16>,
}

impl Template {
    pub fn parse(raw: &str) -> Result<Self, TemplateError> {
        let raw = raw.trim().trim_matches('/');
        if raw.is_empty() {
            return Err(TemplateError::Empty);
        }

        let mut parts = Vec::new();
        let mut literal = String::new();
        let mut chars = raw.chars().peekable();

        while let Some(c) = chars.next() {
            match c {
                '{' => {
                    if !literal.is_empty() {
                        parts.push(Part::Literal(std::mem::take(&mut literal)));
                    }
                    let mut name = String::new();
                    let mut closed = false;
                    for nc in chars.by_ref() {
                        if nc == '}' {
                            closed = true;
                            break;
                        }
                        if nc == '{' {
                            return Err(TemplateError::UnbalancedBraces(raw.to_string()));
                        }
                        name.push(nc);
                    }
                    if !closed {
                        return Err(TemplateError::UnbalancedBraces(raw.to_string()));
                    }
                    parts.push(match name.as_str() {
                        "host" => Part::Host,
                        "owner" | "group_path" => Part::Owner,
                        "repo" => Part::Repo,
                        "port" => Part::Port,
                        other => {
                            return Err(TemplateError::UnknownPlaceholder(other.to_string()));
                        }
                    });
                }
                '}' => return Err(TemplateError::UnbalancedBraces(raw.to_string())),
                _ => literal.push(c),
            }
        }
        if !literal.is_empty() {
            parts.push(Part::Literal(literal));
        }

        if !parts.contains(&Part::Repo) {
            return Err(TemplateError::MissingRepo(raw.to_string()));
        }
        if !parts.contains(&Part::Owner) {
            return Err(TemplateError::MissingOwner);
        }

        Ok(Template {
            raw: raw.to_string(),
            parts,
        })
    }

    pub fn as_str(&self) -> &str {
        &self.raw
    }

    /// Render the canonical relative path for `identity`.
    pub fn render(&self, identity: &Identity, port: Option<u16>) -> String {
        let mut out = String::new();
        for part in &self.parts {
            let piece = match part {
                Part::Literal(s) => s.clone(),
                Part::Host => identity.host.clone(),
                Part::Owner => identity.owner_path(),
                Part::Repo => identity.repo.clone(),
                Part::Port => port.map(|p| p.to_string()).unwrap_or_default(),
            };
            out.push_str(&piece);
        }
        // An unset `{port}` renders as empty and leaves a doubled slash behind.
        collapse_slashes(&out)
    }

    /// Attempt to read an identity back out of a relative path.
    pub fn capture(&self, path: &str) -> Option<Captured> {
        let re = self.to_regex();
        let path = path.trim().trim_matches('/');
        let caps = re.captures(path)?;

        let mut captured = Captured::default();
        if let Some(m) = caps.name("host") {
            captured.host = Some(m.as_str().to_string());
        }
        if let Some(m) = caps.name("owner") {
            captured.owner = Some(
                m.as_str()
                    .split('/')
                    .filter(|s| !s.is_empty())
                    .map(|s| s.to_string())
                    .collect(),
            );
        }
        if let Some(m) = caps.name("repo") {
            captured.repo = Some(m.as_str().to_string());
        }
        if let Some(m) = caps.name("port") {
            captured.port = m.as_str().parse().ok();
        }
        Some(captured)
    }

    /// Compile the template into an anchored regex for [`capture`]. `{owner}`
    /// matches greedily across `/` so a GitLab group path is captured whole,
    /// while `{repo}` stays within one segment so the split lands correctly.
    ///
    /// [`capture`]: Template::capture
    fn to_regex(&self) -> Regex {
        let mut pattern = String::from("^");
        for part in &self.parts {
            match part {
                Part::Literal(s) => pattern.push_str(&regex::escape(s)),
                Part::Host => pattern.push_str("(?P<host>[^/]+)"),
                Part::Owner => pattern.push_str("(?P<owner>.+)"),
                Part::Repo => pattern.push_str("(?P<repo>[^/]+)"),
                Part::Port => pattern.push_str("(?P<port>[0-9]+)"),
            }
        }
        pattern.push('$');
        Regex::new(&pattern).expect("template regex is well-formed by construction")
    }
}

impl fmt::Display for Template {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.raw)
    }
}

fn collapse_slashes(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut prev_slash = false;
    for c in s.chars() {
        if c == '/' {
            if !prev_slash {
                out.push(c);
            }
            prev_slash = true;
        } else {
            out.push(c);
            prev_slash = false;
        }
    }
    out.trim_matches('/').to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ident(host: &str, owner: &[&str], repo: &str) -> Identity {
        Identity {
            host: host.to_string(),
            owner: owner.iter().map(|s| s.to_string()).collect(),
            repo: repo.to_string(),
        }
    }

    #[test]
    fn default_template_renders() {
        let t = Template::parse(DEFAULT_TEMPLATE).unwrap();
        assert_eq!(
            t.render(&ident("github.com", &["jmsnll"], "fussy-git"), None),
            "github.com/jmsnll/fussy-git"
        );
    }

    #[test]
    fn subgroup_render() {
        let t = Template::parse("{host}/{owner}/{repo}").unwrap();
        assert_eq!(
            t.render(&ident("gitlab.com", &["acme", "backend"], "api"), None),
            "gitlab.com/acme/backend/api"
        );
    }

    #[test]
    fn group_path_alias() {
        let t = Template::parse("acme/{group_path}/{repo}").unwrap();
        assert_eq!(
            t.render(&ident("gitlab.internal", &["team", "sub"], "svc"), None),
            "acme/team/sub/svc"
        );
    }

    #[test]
    fn port_placeholder_empty_collapses() {
        let t = Template::parse("{host}/{port}/{owner}/{repo}").unwrap();
        assert_eq!(t.render(&ident("h", &["o"], "r"), None), "h/o/r");
        assert_eq!(t.render(&ident("h", &["o"], "r"), Some(2222)), "h/2222/o/r");
    }

    #[test]
    fn round_trips_default() {
        let t = Template::parse(DEFAULT_TEMPLATE).unwrap();
        let id = ident("github.com", &["jmsnll"], "fussy-git");
        let path = t.render(&id, None);
        let cap = t.capture(&path).unwrap();
        assert_eq!(cap.host.as_deref(), Some("github.com"));
        assert_eq!(cap.owner, Some(vec!["jmsnll".to_string()]));
        assert_eq!(cap.repo.as_deref(), Some("fussy-git"));
    }

    #[test]
    fn round_trips_subgroup() {
        let t = Template::parse(DEFAULT_TEMPLATE).unwrap();
        let id = ident("gitlab.com", &["acme", "backend", "team"], "api");
        let path = t.render(&id, None);
        let cap = t.capture(&path).unwrap();
        assert_eq!(cap.host.as_deref(), Some("gitlab.com"));
        assert_eq!(
            cap.owner,
            Some(vec![
                "acme".to_string(),
                "backend".to_string(),
                "team".to_string()
            ])
        );
        assert_eq!(cap.repo.as_deref(), Some("api"));
    }

    #[test]
    fn capture_without_host_placeholder() {
        let t = Template::parse("acme/{group_path}/{repo}").unwrap();
        let cap = t.capture("acme/team/sub/svc").unwrap();
        assert_eq!(cap.host, None);
        assert_eq!(cap.owner, Some(vec!["team".to_string(), "sub".to_string()]));
        assert_eq!(cap.repo.as_deref(), Some("svc"));
    }

    #[test]
    fn capture_rejects_non_matching_literal() {
        let t = Template::parse("acme/{group_path}/{repo}").unwrap();
        assert!(t.capture("other/team/svc").is_none());
    }

    #[test]
    fn rejects_unknown_placeholder() {
        assert_eq!(
            Template::parse("{host}/{team}/{repo}").unwrap_err(),
            TemplateError::UnknownPlaceholder("team".to_string())
        );
    }

    #[test]
    fn rejects_missing_repo() {
        assert!(matches!(
            Template::parse("{host}/{owner}"),
            Err(TemplateError::MissingRepo(_))
        ));
    }

    #[test]
    fn rejects_missing_owner() {
        assert_eq!(
            Template::parse("{host}/{repo}").unwrap_err(),
            TemplateError::MissingOwner
        );
    }

    #[test]
    fn rejects_unbalanced() {
        assert!(matches!(
            Template::parse("{host/{repo}"),
            Err(TemplateError::UnbalancedBraces(_))
        ));
    }
}
