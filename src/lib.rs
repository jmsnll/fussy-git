//! fussy-git keeps cloned git repositories organised under a configurable root,
//! deriving each repository's path from its remote URL (for example
//! `~/git/github.com/{owner}/{repo}`) and reconciling the filesystem against
//! that layout.

pub mod adopt;
pub mod bulk;
pub mod config;
pub mod doctor;
pub mod fsops;
pub mod get;
pub mod git;
pub mod identity;
pub mod index;
pub mod list;
pub mod manifest;
pub mod ops;
pub mod preflight;
pub mod reconcile;
pub mod remove;
pub mod resolve;
pub mod scan;
pub mod shell;
pub mod sync;
pub mod template;
pub mod tui;
pub mod ui;

#[cfg(test)]
pub mod testutil;
