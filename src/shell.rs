//! `fussy-git shell-init <bash|zsh|fish>` and the `fussy-git cd <query>`
//! resolver.
//!
//! `shell-init` emits a snippet defining an `fg` shell function:
//!
//! * with no arguments it pipes `fussy-git list --path` into `fzf` (falling back
//!   to `fussy-git browse` when `fzf` is not installed) and `cd`s to the choice;
//! * with arguments it runs `cd "$(fussy-git cd "$@")"`.
//!
//! `cd_target` is the unambiguous resolver behind `fussy-git cd`: exactly one
//! match or it errors. Disambiguation is the shell function's job (via `fzf` /
//! `browse`), not `cd`'s.

use std::path::PathBuf;

use anyhow::{bail, Result};

use crate::config::Config;
use crate::scan;

/// A shell that fussy-git can emit an integration snippet for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Shell {
    Bash,
    Zsh,
    Fish,
}

/// Parse a shell name (case-insensitive). Accepts `bash`, `zsh`, `fish`.
pub fn parse_shell(s: &str) -> Result<Shell> {
    match s.trim().to_ascii_lowercase().as_str() {
        "bash" => Ok(Shell::Bash),
        "zsh" => Ok(Shell::Zsh),
        "fish" => Ok(Shell::Fish),
        other => bail!("unsupported shell {other:?} (expected bash, zsh or fish)"),
    }
}

/// The shell integration script for `shell`, ready to pass to `eval` or `source`.
pub fn init_script(shell: Shell) -> String {
    match shell {
        Shell::Bash => posix_script("bash"),
        Shell::Zsh => posix_script("zsh"),
        Shell::Fish => FISH_SCRIPT.to_string(),
    }
}

fn posix_script(shell: &str) -> String {
    format!(
        r#"# fussy-git shell integration ({shell})
# Enable with:  eval "$(fussy-git shell-init {shell})"

fg() {{
    local _fg_dir
    if [ "$#" -eq 0 ]; then
        if command -v fzf >/dev/null 2>&1; then
            _fg_dir="$(fussy-git list --path | fzf)" || return
        else
            _fg_dir="$(fussy-git browse)" || return
        fi
    else
        _fg_dir="$(fussy-git cd "$@")" || return
    fi
    [ -n "$_fg_dir" ] && cd "$_fg_dir"
}}
"#
    )
}

const FISH_SCRIPT: &str = r#"# fussy-git shell integration (fish)
# Enable with:  fussy-git shell-init fish | source

function fg --description 'jump to a fussy-git managed repository'
    set -l _fg_dir
    if test (count $argv) -eq 0
        if type -q fzf
            set _fg_dir (fussy-git list --path | fzf)
        else
            set _fg_dir (fussy-git browse)
        end
    else
        set _fg_dir (fussy-git cd $argv)
    end
    test -n "$_fg_dir"; and cd $_fg_dir
end
"#;

/// Resolve `query` to exactly one managed repository path.
///
/// `query` is matched case-insensitively as a substring of each repo's
/// `host/owner/repo` slug and of its `owner/repo` project path. Zero matches or
/// more than one is an error (the latter lists the candidate slugs).
///
/// Note: this walks the filesystem via [`scan::scan`]; a future revision may
/// serve it from the index module for speed.
pub fn cd_target(cfg: &Config, query: &str) -> Result<PathBuf> {
    let needle = query.trim().to_lowercase();
    if needle.is_empty() {
        bail!("empty query");
    }

    let mut matches: Vec<(String, PathBuf)> = Vec::new();
    for d in scan::scan(cfg)? {
        let (slug, project) = match &d.identity {
            Some(id) => (id.to_string(), id.project_path()),
            None => {
                let rel = d.rel.to_string_lossy().into_owned();
                (rel.clone(), rel)
            }
        };
        let hit = slug.to_lowercase().contains(&needle) || project.to_lowercase().contains(&needle);
        if hit && !matches.iter().any(|(_, p)| p == &d.path) {
            matches.push((slug, d.path));
        }
    }

    matches.sort();
    match matches.len() {
        1 => Ok(matches.pop().expect("length checked").1),
        0 => bail!("no repo matches {query:?}"),
        n => {
            let list = matches
                .iter()
                .map(|(slug, _)| slug.as_str())
                .collect::<Vec<_>>()
                .join("\n");
            bail!("{n} repos match {query:?}:\n{list}")
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::make_repo;

    fn tree() -> (tempfile::TempDir, Config) {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("git");
        std::fs::create_dir_all(&root).unwrap();
        let cfg_path = tmp.path().join(".fussy-git.toml");
        std::fs::write(&cfg_path, format!("root = {root:?}\n")).unwrap();

        make_repo(
            &root.join("github.com/rust-lang/rust"),
            Some("https://github.com/rust-lang/rust.git"),
        );
        make_repo(
            &root.join("github.com/rust-lang/cargo"),
            Some("https://github.com/rust-lang/cargo.git"),
        );
        make_repo(
            &root.join("github.com/jmsnll/fussy-git"),
            Some("git@github.com:jmsnll/fussy-git.git"),
        );

        let cfg = Config::load_from(&cfg_path).unwrap();
        (tmp, cfg)
    }

    #[test]
    fn parse_shell_accepts_known_and_rejects_others() {
        assert_eq!(parse_shell("bash").unwrap(), Shell::Bash);
        assert_eq!(parse_shell("ZSH").unwrap(), Shell::Zsh);
        assert_eq!(parse_shell(" fish ").unwrap(), Shell::Fish);
        assert!(parse_shell("powershell").is_err());
        assert!(parse_shell("").is_err());
    }

    #[test]
    fn init_script_defines_fg_and_references_tool() {
        for shell in [Shell::Bash, Shell::Zsh, Shell::Fish] {
            let script = init_script(shell);
            assert!(!script.is_empty());
            assert!(script.contains("fg"), "{shell:?} missing fg");
            assert!(script.contains("fussy-git"), "{shell:?} missing fussy-git");
            assert!(script.contains("cd"), "{shell:?} missing cd");
        }
        assert!(init_script(Shell::Fish).contains("function fg"));
        assert!(init_script(Shell::Bash).contains("fussy-git cd"));
    }

    #[test]
    fn cd_target_returns_the_unique_match() {
        let (_tmp, cfg) = tree();
        let path = cd_target(&cfg, "jmsnll/fussy-git").unwrap();
        assert!(path.ends_with("github.com/jmsnll/fussy-git"));

        // substring of host/owner/repo, still unique
        let path = cd_target(&cfg, "JMSNLL").unwrap();
        assert!(path.ends_with("github.com/jmsnll/fussy-git"));
    }

    #[test]
    fn cd_target_errors_when_nothing_matches() {
        let (_tmp, cfg) = tree();
        let err = cd_target(&cfg, "nonesuch").unwrap_err().to_string();
        assert!(err.contains("no repo matches"), "{err}");
    }

    #[test]
    fn cd_target_lists_candidates_when_ambiguous() {
        let (_tmp, cfg) = tree();
        let err = cd_target(&cfg, "rust-lang").unwrap_err().to_string();
        assert!(err.contains("github.com/rust-lang/rust"), "{err}");
        assert!(err.contains("github.com/rust-lang/cargo"), "{err}");
        assert!(err.contains("2 repos match"), "{err}");
    }
}
