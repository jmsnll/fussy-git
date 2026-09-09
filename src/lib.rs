//! fussy-git keeps cloned git repositories organised under a configurable root,
//! deriving each repository's path from its remote URL (for example
//! `~/git/github.com/{owner}/{repo}`) and reconciling the filesystem against
//! that layout.

pub mod bulk;
pub mod config;
pub mod fsops;
pub mod get;
pub mod git;
pub mod identity;
pub mod index;
pub mod list;
pub mod ops;
pub mod resolve;
pub mod scan;
pub mod template;
pub mod ui;

#[cfg(test)]
pub mod testutil;
