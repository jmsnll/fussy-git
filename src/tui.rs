//! `fussy-git browse` — a full-screen fuzzy repository picker.
//!
//! All list/filter/cursor arithmetic lives in [`BrowserState`], which knows
//! nothing about a terminal and is unit-tested directly. [`browse`] is the thin
//! ratatui shell around it: it enters raw mode + the alternate screen behind a
//! [`TerminalGuard`] whose `Drop` restores the terminal on *every* exit path,
//! including a panic unwinding through the event loop.

use std::io::{self, Stdout};
use std::path::PathBuf;

use anyhow::Result;
use ratatui::backend::CrosstermBackend;
use ratatui::crossterm::event::{self, Event, KeyCode, KeyEventKind, KeyModifiers};
use ratatui::crossterm::execute;
use ratatui::crossterm::terminal::{
    disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen,
};
use ratatui::layout::{Constraint, Layout};
use ratatui::style::{Modifier, Style};
use ratatui::text::Line;
use ratatui::widgets::{Block, Borders, List, ListItem, ListState, Paragraph};
use ratatui::{Frame, Terminal};

use crate::config;
use crate::scan;

/// One selectable repository.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BrowseItem {
    /// Absolute path to the repository on disk.
    pub path: PathBuf,
    /// `host/owner/repo`, or the path relative to the managed root when the
    /// repo has no parseable remote.
    pub label: String,
}

/// The testable core of the browser: the item list, the current filter query,
/// the indices that survive it and where the cursor sits within them.
#[derive(Debug, Clone)]
pub struct BrowserState {
    items: Vec<BrowseItem>,
    query: String,
    /// Indices into `items` that match `query`, in `items` order.
    filtered: Vec<usize>,
    /// Cursor position *within* `filtered`.
    cursor: usize,
}

impl BrowserState {
    pub fn new(items: Vec<BrowseItem>) -> Self {
        let filtered = (0..items.len()).collect();
        Self {
            items,
            query: String::new(),
            filtered,
            cursor: 0,
        }
    }

    /// Set the filter query and recompute the visible set. Matching is
    /// case-insensitive and succeeds on either a substring hit or an in-order
    /// subsequence of the query's characters in the label. The cursor is
    /// clamped back into range.
    pub fn set_query(&mut self, q: &str) {
        self.query = q.to_string();
        let needle = q.trim().to_lowercase();
        self.filtered = self
            .items
            .iter()
            .enumerate()
            .filter(|(_, it)| fuzzy_match(&it.label, &needle))
            .map(|(i, _)| i)
            .collect();
        self.clamp_cursor();
    }

    pub fn move_up(&mut self) {
        self.cursor = self.cursor.saturating_sub(1);
    }

    pub fn move_down(&mut self) {
        if self.cursor + 1 < self.filtered.len() {
            self.cursor += 1;
        }
    }

    /// The items currently passing the filter, in display order.
    pub fn visible(&self) -> Vec<&BrowseItem> {
        self.filtered.iter().map(|&i| &self.items[i]).collect()
    }

    /// The item under the cursor, or `None` when nothing matches.
    pub fn selected(&self) -> Option<&BrowseItem> {
        self.filtered.get(self.cursor).map(|&i| &self.items[i])
    }

    pub fn query(&self) -> &str {
        &self.query
    }

    /// Cursor offset within the visible list (0 when the list is empty).
    pub fn cursor(&self) -> usize {
        self.cursor
    }

    fn clamp_cursor(&mut self) {
        if self.filtered.is_empty() {
            self.cursor = 0;
        } else {
            self.cursor = self.cursor.min(self.filtered.len() - 1);
        }
    }
}

/// Case-insensitive substring-or-subsequence match. `needle` is expected to be
/// already lowercased; an empty needle matches everything.
fn fuzzy_match(label: &str, needle: &str) -> bool {
    if needle.is_empty() {
        return true;
    }
    let hay = label.to_lowercase();
    if hay.contains(needle) {
        return true;
    }
    let mut chars = hay.chars();
    needle.chars().all(|nc| chars.any(|hc| hc == nc))
}

/// Build the browser's item list from a filesystem scan.
fn build_items(cfg: &config::Config) -> Result<Vec<BrowseItem>> {
    let mut items: Vec<BrowseItem> = scan::scan(cfg)?
        .into_iter()
        .map(|d| {
            let label = d
                .identity
                .as_ref()
                .map(|id| id.to_string())
                .unwrap_or_else(|| d.rel.to_string_lossy().into_owned());
            BrowseItem {
                path: d.path,
                label,
            }
        })
        .collect();
    items.sort_by(|a, b| a.label.cmp(&b.label).then_with(|| a.path.cmp(&b.path)));
    items.dedup_by(|a, b| a.path == b.path);
    Ok(items)
}

/// Run the interactive browser. Returns the chosen repository path, or `None`
/// if the user cancelled. Nothing is written to stdout here — the caller prints
/// the returned path.
pub fn browse(cfg: &config::Config) -> Result<Option<PathBuf>> {
    let items = build_items(cfg)?;
    if items.is_empty() {
        return Ok(None);
    }
    let mut state = BrowserState::new(items);

    // `guard` is declared first so it is dropped last — after `terminal` has
    // released stdout — restoring the terminal on every return and on panic.
    let _guard = TerminalGuard::enter()?;
    let mut terminal = Terminal::new(CrosstermBackend::new(io::stdout()))?;
    event_loop(&mut terminal, &mut state)
}

/// Restores raw mode + the alternate screen when dropped, however that happens.
struct TerminalGuard;

impl TerminalGuard {
    fn enter() -> Result<Self> {
        enable_raw_mode()?;
        execute!(io::stdout(), EnterAlternateScreen)?;
        Ok(Self)
    }
}

impl Drop for TerminalGuard {
    fn drop(&mut self) {
        let _ = disable_raw_mode();
        let _ = execute!(io::stdout(), LeaveAlternateScreen);
    }
}

fn event_loop(
    terminal: &mut Terminal<CrosstermBackend<Stdout>>,
    state: &mut BrowserState,
) -> Result<Option<PathBuf>> {
    loop {
        terminal.draw(|frame| render(frame, state))?;

        let Event::Key(key) = event::read()? else {
            continue;
        };
        if key.kind == KeyEventKind::Release {
            continue;
        }
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        let alt = key.modifiers.contains(KeyModifiers::ALT);

        match key.code {
            KeyCode::Enter => return Ok(state.selected().map(|it| it.path.clone())),
            KeyCode::Esc => return Ok(None),
            KeyCode::Char('c') if ctrl => return Ok(None),
            KeyCode::Char('p') if ctrl => state.move_up(),
            KeyCode::Char('n') if ctrl => state.move_down(),
            KeyCode::Up => state.move_up(),
            KeyCode::Down => state.move_down(),
            KeyCode::Char('q') if state.query().is_empty() => return Ok(None),
            KeyCode::Backspace => {
                let mut q = state.query().to_string();
                q.pop();
                state.set_query(&q);
            }
            KeyCode::Char(c) if !ctrl && !alt => {
                let mut q = state.query().to_string();
                q.push(c);
                state.set_query(&q);
            }
            _ => {}
        }
    }
}

fn render(frame: &mut Frame, state: &BrowserState) {
    let [top, body] =
        Layout::vertical([Constraint::Length(3), Constraint::Min(1)]).areas(frame.area());

    let prompt = Paragraph::new(Line::from(format!("> {}", state.query()))).block(
        Block::default()
            .borders(Borders::ALL)
            .title("filter (Enter: open  Esc: cancel)"),
    );
    frame.render_widget(prompt, top);

    let rows: Vec<ListItem> = state
        .visible()
        .iter()
        .map(|it| ListItem::new(it.label.clone()))
        .collect();
    let count = rows.len();
    let list = List::new(rows)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .title(format!("repositories ({count})")),
        )
        .highlight_style(Style::default().add_modifier(Modifier::REVERSED))
        .highlight_symbol("> ");

    let mut list_state = ListState::default();
    if count > 0 {
        list_state.select(Some(state.cursor()));
    }
    frame.render_stateful_widget(list, body, &mut list_state);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> Vec<BrowseItem> {
        [
            "github.com/rust-lang/rust",
            "github.com/rust-lang/cargo",
            "github.com/jmsnll/fussy-git",
            "gitlab.com/acme/backend/api",
        ]
        .iter()
        .map(|s| BrowseItem {
            path: PathBuf::from("/root").join(s),
            label: s.to_string(),
        })
        .collect()
    }

    fn labels(state: &BrowserState) -> Vec<String> {
        state.visible().iter().map(|it| it.label.clone()).collect()
    }

    #[test]
    fn empty_query_shows_everything() {
        let state = BrowserState::new(sample());
        assert_eq!(state.visible().len(), 4);
        assert_eq!(state.selected().unwrap().label, "github.com/rust-lang/rust");
    }

    #[test]
    fn substring_filter_narrows() {
        let mut state = BrowserState::new(sample());
        state.set_query("rust-lang");
        assert_eq!(
            labels(&state),
            vec!["github.com/rust-lang/rust", "github.com/rust-lang/cargo"]
        );
    }

    #[test]
    fn subsequence_filter_matches() {
        let mut state = BrowserState::new(sample());
        // g,h then f,u,s,s,y appears in order only in the fussy-git label.
        state.set_query("ghfussy");
        assert_eq!(labels(&state), vec!["github.com/jmsnll/fussy-git"]);
    }

    #[test]
    fn cursor_clamps_at_bounds() {
        let mut state = BrowserState::new(sample());
        for _ in 0..10 {
            state.move_down();
        }
        assert_eq!(state.cursor(), 3);
        for _ in 0..10 {
            state.move_up();
        }
        assert_eq!(state.cursor(), 0);
    }

    #[test]
    fn cursor_clamps_after_filter_and_selection_tracks() {
        let mut state = BrowserState::new(sample());
        for _ in 0..3 {
            state.move_down();
        }
        assert_eq!(state.cursor(), 3);

        state.set_query("rust-lang");
        assert_eq!(state.cursor(), 1);
        assert_eq!(
            state.selected().unwrap().label,
            "github.com/rust-lang/cargo"
        );

        state.set_query("no-such-repo-xyz");
        assert!(state.visible().is_empty());
        assert!(state.selected().is_none());

        state.set_query("");
        assert_eq!(state.visible().len(), 4);
        assert_eq!(state.selected().unwrap().label, "github.com/rust-lang/rust");
    }

    #[test]
    fn build_items_falls_back_to_relative_path_without_remote() {
        use crate::config::Config;
        use crate::testutil::make_repo;

        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("git");
        std::fs::create_dir_all(&root).unwrap();
        let cfg_path = tmp.path().join(".fussy-git.toml");
        std::fs::write(&cfg_path, format!("root = {root:?}\n")).unwrap();

        make_repo(
            &root.join("github.com/jmsnll/fussy-git"),
            Some("git@github.com:jmsnll/fussy-git.git"),
        );
        make_repo(&root.join("scratch/local-only"), None);

        let cfg = Config::load_from(&cfg_path).unwrap();
        let items = build_items(&cfg).unwrap();
        let found: Vec<&str> = items.iter().map(|i| i.label.as_str()).collect();
        assert!(found.contains(&"github.com/jmsnll/fussy-git"));
        assert!(found.contains(&"scratch/local-only"));
    }
}
