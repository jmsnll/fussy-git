//! Terminal presentation: colour that respects the environment, and helpers for
//! rendering repository paths compactly.
//!
//! Colour is decided per stream by [`init`] and defaults to off, so any code
//! path that does not call it — the library's own tests, a piped invocation —
//! produces plain text. Meaning is always carried by a word; colour only
//! reinforces it, so the output stays readable with `NO_COLOR`, through a pipe,
//! and for a reader who cannot distinguish the hues.

use std::io::IsTerminal;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};

use anstyle::{AnsiColor, Style};

use crate::config::Config;

static STDOUT_COLOR: AtomicBool = AtomicBool::new(false);
static STDERR_COLOR: AtomicBool = AtomicBool::new(false);

/// The `--color` choice.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ColorChoice {
    #[default]
    Auto,
    Always,
    Never,
}

/// Decide whether stdout and stderr get colour, once, at startup.
pub fn init(choice: ColorChoice) {
    STDOUT_COLOR.store(
        decide(choice, std::io::stdout().is_terminal()),
        Ordering::Relaxed,
    );
    STDERR_COLOR.store(
        decide(choice, std::io::stderr().is_terminal()),
        Ordering::Relaxed,
    );
}

fn decide(choice: ColorChoice, is_tty: bool) -> bool {
    match choice {
        ColorChoice::Always => true,
        ColorChoice::Never => false,
        ColorChoice::Auto => {
            if env_flag("NO_COLOR") {
                return false;
            }
            if env_flag("CLICOLOR_FORCE") {
                return true;
            }
            is_tty
        }
    }
}

/// True when an environment variable is present and not an explicit "off".
fn env_flag(key: &str) -> bool {
    match std::env::var_os(key) {
        Some(v) => !v.is_empty() && v != "0",
        None => false,
    }
}

// --- palette ---------------------------------------------------------------

fn fg(color: AnsiColor) -> Style {
    Style::new().fg_color(Some(color.into()))
}

fn paint(enabled: bool, style: Style, text: &str) -> String {
    if enabled {
        format!("{}{text}{}", style.render(), style.render_reset())
    } else {
        text.to_string()
    }
}

fn out() -> bool {
    STDOUT_COLOR.load(Ordering::Relaxed)
}
fn err() -> bool {
    STDERR_COLOR.load(Ordering::Relaxed)
}

/// A section or table heading (bold).
pub fn header(text: &str) -> String {
    paint(out(), Style::new().bold(), text)
}
/// Secondary detail: reasons, path prefixes, "in place" counts.
pub fn dim(text: &str) -> String {
    paint(out(), Style::new().dimmed(), text)
}
/// A good outcome: added, up to date, ahead of upstream.
pub fn ok(text: &str) -> String {
    paint(out(), fg(AnsiColor::Green), text)
}
/// Needs a decision but nothing is broken: skips, behind upstream, a dirty tree.
pub fn warn(text: &str) -> String {
    paint(out(), fg(AnsiColor::Yellow), text)
}
/// A failure or a destructive/blocking condition.
pub fn danger(text: &str) -> String {
    paint(out(), fg(AnsiColor::Red), text)
}
/// A path or an identifier the reader is meant to act on.
pub fn accent(text: &str) -> String {
    paint(out(), fg(AnsiColor::Cyan), text)
}

/// `error:` for the start of a diagnostic line on stderr.
pub fn err_prefix() -> String {
    paint(
        err(),
        Style::new().bold().fg_color(Some(AnsiColor::Red.into())),
        "error:",
    )
}
/// `warning:` for the start of a diagnostic line on stderr.
pub fn warn_prefix() -> String {
    paint(
        err(),
        Style::new().bold().fg_color(Some(AnsiColor::Yellow.into())),
        "warning:",
    )
}
/// A note on stderr that is neither an error nor a warning.
pub fn note(text: &str) -> String {
    paint(err(), Style::new().dimmed(), text)
}

/// Right-pad `painted` (which may contain colour escapes) with spaces to
/// `width` *visible* columns, given that its visible text is `plain_len` wide.
pub fn pad_to(painted: &str, plain_len: usize, width: usize) -> String {
    let mut s = painted.to_string();
    if plain_len < width {
        s.push_str(&" ".repeat(width - plain_len));
    }
    s
}

// --- paths ----------------------------------------------------------------

/// A repository path rendered relative to the most specific managed root that
/// contains it, so output reads `github.com/owner/repo` rather than an absolute
/// path. Falls back to the absolute path (with `$HOME` collapsed to `~`) when
/// the repository is under no root.
pub fn repo_name(cfg: &Config, path: &Path) -> String {
    let shortest = cfg
        .roots
        .iter()
        .filter_map(|root| path.strip_prefix(root).ok())
        .min_by_key(|rel| rel.components().count());
    match shortest {
        Some(rel) => rel.to_string_lossy().into_owned(),
        None => collapse_home(path),
    }
}

fn collapse_home(path: &Path) -> String {
    if let Some(home) = dirs::home_dir() {
        if let Ok(rest) = path.strip_prefix(&home) {
            return format!("~/{}", rest.display());
        }
    }
    path.to_string_lossy().into_owned()
}

/// Shorten `s` to at most `max` characters by removing the middle, keeping more
/// of the tail because the repository name lives there. Returns `s` unchanged
/// when it already fits or `max` is too small to be useful.
pub fn truncate_middle(s: &str, max: usize) -> String {
    let len = s.chars().count();
    if len <= max || max < 7 {
        return s.to_string();
    }
    let keep = max - 3;
    let head = keep / 3;
    let tail = keep - head;
    let chars: Vec<char> = s.chars().collect();
    let head_str: String = chars[..head].iter().collect();
    let tail_str: String = chars[len - tail..].iter().collect();
    format!("{head_str}...{tail_str}")
}

/// The usable terminal width, from `$COLUMNS`, then the tty, then a default.
pub fn term_width() -> usize {
    if let Ok(cols) = std::env::var("COLUMNS") {
        if let Ok(n) = cols.trim().parse::<usize>() {
            if n > 0 {
                return n;
            }
        }
    }
    ratatui::crossterm::terminal::size()
        .ok()
        .map(|(w, _)| w as usize)
        .filter(|&w| w > 0)
        .unwrap_or(100)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn colour_is_off_until_initialised() {
        // Other tests never call init, so both streams stay plain.
        assert_eq!(header("x"), "x");
        assert_eq!(danger("boom"), "boom");
    }

    #[test]
    fn decide_respects_choice() {
        assert!(decide(ColorChoice::Always, false));
        assert!(!decide(ColorChoice::Never, true));
        assert!(decide(ColorChoice::Auto, true));
        assert!(!decide(ColorChoice::Auto, false));
    }

    #[test]
    fn repo_name_is_relative_to_the_deepest_root() {
        let mut cfg = Config::default();
        cfg.roots = vec![
            Path::new("/git").to_path_buf(),
            Path::new("/git/work").to_path_buf(),
        ];
        assert_eq!(
            repo_name(&cfg, Path::new("/git/work/github.com/me/x")),
            "github.com/me/x"
        );
        assert_eq!(
            repo_name(&cfg, Path::new("/git/github.com/me/y")),
            "github.com/me/y"
        );
    }

    #[test]
    fn truncate_middle_keeps_both_ends() {
        assert_eq!(truncate_middle("short", 20), "short");
        let t = truncate_middle("gitlab.com/group/subgroup/team/service", 20);
        assert!(t.chars().count() <= 20, "{t}");
        assert!(t.starts_with("git"), "{t}");
        assert!(t.ends_with("service"), "{t}");
        assert!(t.contains("..."), "{t}");
    }
}
