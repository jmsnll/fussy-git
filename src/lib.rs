//! fussy-git keeps cloned git repositories organised under a configurable root,
//! deriving each repository's path from its remote URL (for example
//! `~/git/github.com/{owner}/{repo}`) and reconciling the filesystem against
//! that layout.

pub mod adopt;
pub mod agent_hook;
pub mod bulk;
pub mod config;
pub mod doctor;
pub mod exec;
pub mod feature;
pub mod fsops;
pub mod get;
pub mod git;
pub mod hooks;
pub mod identity;
pub mod index;
pub mod info;
pub mod list;
pub mod ops;
pub mod preflight;
pub mod query;
pub mod reconcile;
pub mod remove;
pub mod resolve;
pub mod scan;
pub mod sets;
pub mod shell;
pub mod template;
pub mod tui;
pub mod ui;
pub mod worktree;

#[cfg(test)]
pub mod testutil;
