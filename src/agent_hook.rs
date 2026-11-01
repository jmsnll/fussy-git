//! `fussy-git hook pre-tool-use` — a Claude Code `PreToolUse` hook that steers
//! `git clone` towards `fussy-git get`.
//!
//! Left alone, coding agents clone whatever they need into the current
//! directory or `/tmp`, which scatters checkouts across the disk — exactly the
//! mess `reconcile` exists to clean up. This hook reads the tool call Claude
//! Code is about to make (JSON on stdin), and when it is a Bash command that
//! runs `git clone`, blocks it and suggests the equivalent `fussy-git get`.
//!
//! Claude Code treats exit code `2` as "block, and show stderr to the model",
//! so the suggestion reaches the agent as feedback and it retries with the
//! right command. Anything that is not a clone is allowed (exit `0`, no
//! output). A command containing `FUSSY_GIT_ALLOW_CLONE=1` is always allowed,
//! for the rare clone that really should go somewhere else.

use serde::Deserialize;

/// Environment assignment that opts a single command out of the hook.
pub const ALLOW_CLONE: &str = "FUSSY_GIT_ALLOW_CLONE=1";

#[derive(Deserialize)]
struct HookInput {
    #[serde(default)]
    tool_name: String,
    #[serde(default)]
    tool_input: ToolInput,
}

#[derive(Deserialize, Default)]
struct ToolInput {
    #[serde(default)]
    command: String,
}

/// Decide on one `PreToolUse` event. Returns the message to block with, or
/// `None` to allow. Input that is not a Bash tool call, or not valid JSON, is
/// allowed: a hook must never wedge the agent.
pub fn pre_tool_use(input: &str) -> Option<String> {
    let event: HookInput = serde_json::from_str(input).ok()?;
    if event.tool_name != "Bash" {
        return None;
    }
    let cmd = event.tool_input.command;
    if cmd.contains(ALLOW_CLONE) {
        return None;
    }
    let clone = find_clone(&cmd)?;
    let mut suggestion = String::from("fussy-git get");
    if let Some(b) = &clone.branch {
        suggestion.push_str(&format!(" --branch {b}"));
    }
    if clone.shallow {
        suggestion.push_str(" --shallow");
    }
    suggestion.push(' ');
    suggestion.push_str(clone.url.as_deref().unwrap_or("<url>"));

    Some(format!(
        "Blocked `git clone`: repositories on this machine are managed by fussy-git.\n\
         Run `{suggestion}` instead. It clones into the canonical path \
         (<root>/<host>/<owner>/<repo>), reuses an existing checkout if there is one, \
         and prints the path on stdout (add --json for structured output), so you can \
         `cd \"$(…)\"` into it. If this clone genuinely must go elsewhere, prefix the \
         command with {ALLOW_CLONE}."
    ))
}

/// The parts of a `git clone` invocation the suggestion needs.
#[derive(Debug, Default, PartialEq, Eq)]
struct Clone {
    url: Option<String>,
    branch: Option<String>,
    shallow: bool,
}

/// git global options that take a separate value (`git -C dir clone …`).
const GIT_OPTS_WITH_VALUE: &[&str] = &["-C", "-c", "--git-dir", "--work-tree", "--namespace"];

/// `git clone` options that take a separate value.
const CLONE_OPTS_WITH_VALUE: &[&str] = &[
    "-b",
    "--branch",
    "-o",
    "--origin",
    "-u",
    "--upload-pack",
    "-c",
    "--config",
    "-j",
    "--jobs",
    "--depth",
    "--shallow-since",
    "--shallow-exclude",
    "--reference",
    "--reference-if-able",
    "--separate-git-dir",
    "--template",
    "--filter",
    "--server-option",
    "--bundle-uri",
];

/// Find the first `git … clone …` in a shell command line.
fn find_clone(cmd: &str) -> Option<Clone> {
    for segment in segments(cmd) {
        let words: Vec<&str> = segment.split_whitespace().collect();
        // Allow leading `VAR=value` assignments and `command`/`exec` wrappers.
        let mut i = 0;
        while i < words.len()
            && (words[i].contains('=') && !words[i].starts_with('-')
                || matches!(words[i], "command" | "exec" | "sudo"))
        {
            i += 1;
        }
        if words.get(i) != Some(&"git") {
            continue;
        }
        i += 1;
        while i < words.len() && words[i].starts_with('-') {
            i += if GIT_OPTS_WITH_VALUE.contains(&words[i]) {
                2
            } else {
                1
            };
        }
        if words.get(i) != Some(&"clone") {
            continue;
        }
        return Some(parse_clone_args(&words[i + 1..]));
    }
    None
}

fn parse_clone_args(args: &[&str]) -> Clone {
    let mut clone = Clone::default();
    let mut i = 0;
    while i < args.len() {
        let a = args[i];
        if a == "--" {
            clone.url = args.get(i + 1).map(|s| unquote(s));
            break;
        }
        if let Some(rest) = a.strip_prefix("--branch=") {
            clone.branch = Some(unquote(rest));
        } else if a.starts_with("--depth") {
            clone.shallow = true;
            if a == "--depth" {
                i += 1;
            }
        } else if a == "-b" || a == "--branch" {
            clone.branch = args.get(i + 1).map(|s| unquote(s));
            i += 1;
        } else if CLONE_OPTS_WITH_VALUE.contains(&a) {
            i += 1;
        } else if !a.starts_with('-') {
            clone.url = Some(unquote(a));
            break;
        }
        i += 1;
    }
    clone
}

/// Split a command line into simple commands on `;`, `&`, `|`, newlines and
/// parentheses. Not a shell parser — quoting is ignored — but good enough to
/// find a `git clone` in the kind of one-liners agents write.
fn segments(cmd: &str) -> impl Iterator<Item = &str> {
    cmd.split(['\n', ';', '&', '|', '(', ')'])
        .map(str::trim)
        .filter(|s| !s.is_empty())
}

fn unquote(s: &str) -> String {
    s.trim_matches(|c| c == '"' || c == '\'').to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bash(cmd: &str) -> String {
        serde_json::json!({
            "hook_event_name": "PreToolUse",
            "tool_name": "Bash",
            "tool_input": { "command": cmd },
        })
        .to_string()
    }

    #[test]
    fn blocks_a_plain_clone_and_suggests_get() {
        let msg = pre_tool_use(&bash("git clone https://github.com/acme/api.git")).unwrap();
        assert!(
            msg.contains("`fussy-git get https://github.com/acme/api.git`"),
            "{msg}"
        );
    }

    #[test]
    fn carries_branch_and_depth_into_the_suggestion() {
        let msg = pre_tool_use(&bash(
            "cd /tmp && git clone --depth 1 -b release git@github.com:acme/api.git api",
        ))
        .unwrap();
        assert!(
            msg.contains("`fussy-git get --branch release --shallow git@github.com:acme/api.git`"),
            "{msg}"
        );
    }

    #[test]
    fn sees_through_git_global_options_and_env() {
        let c = find_clone("GIT_TRACE=1 git -C /tmp -c x=y clone repo-url").unwrap();
        assert_eq!(c.url.as_deref(), Some("repo-url"));
    }

    #[test]
    fn allows_everything_else() {
        assert!(pre_tool_use(&bash("git status")).is_none());
        assert!(pre_tool_use(&bash("echo git clone is great")).is_none());
        assert!(pre_tool_use(&bash("fussy-git get acme/api")).is_none());
        assert!(pre_tool_use(&bash("git log --grep clone")).is_none());
        assert!(pre_tool_use(&bash(&format!("{ALLOW_CLONE} git clone x"))).is_none());
        let not_bash = serde_json::json!({"tool_name": "Read", "tool_input": {}}).to_string();
        assert!(pre_tool_use(&not_bash).is_none());
        assert!(pre_tool_use("not json").is_none());
    }
}
