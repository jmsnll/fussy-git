//! Test-only helpers for building throwaway git repositories that are fully
//! isolated from the developer's global/system git config and hooks.

use std::path::Path;
use std::process::Command;
use std::sync::OnceLock;

use tempfile::TempDir;

/// A controlled global config file, shared by every test git invocation. It
/// replaces the developer's `~/.gitconfig` (hooks path, gpg signing, commit
/// templates, commit blockers) and supplies a committer identity, since a CI
/// runner has neither a global identity nor a system full name for git to fall
/// back on.
fn isolated_global_config() -> &'static Path {
    static DIR: OnceLock<TempDir> = OnceLock::new();
    let dir = DIR.get_or_init(|| {
        let d = tempfile::tempdir().expect("tempdir for isolated gitconfig");
        std::fs::write(
            d.path().join("gitconfig"),
            "[user]\n\tname = fussy-git tests\n\temail = tests@fussy-git.invalid\n\
             [init]\n\tdefaultBranch = main\n\
             [commit]\n\tgpgsign = false\n\
             [core]\n\thooksPath = /dev/null\n",
        )
        .unwrap();
        d
    });
    Box::leak(dir.path().join("gitconfig").into_boxed_path())
}

/// Run `git <args>` in `dir` with a scrubbed environment. Panics on failure.
pub fn git(dir: &Path, args: &[&str]) {
    let status = git_command(dir).args(args).status().expect("spawn git");
    assert!(status.success(), "git {args:?} failed in {}", dir.display());
}

fn git_command(dir: &Path) -> Command {
    let mut cmd = Command::new("git");
    cmd.current_dir(dir)
        .env("GIT_CONFIG_GLOBAL", isolated_global_config())
        .env("GIT_CONFIG_SYSTEM", "/dev/null")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("HOME", "/nonexistent-fussy-git-test-home")
        .env_remove("GIT_DIR")
        .env_remove("GIT_WORK_TREE");
    cmd
}

/// Create an initialised repo at `dir` with one commit on `main`, optionally
/// with an `origin` remote.
pub fn make_repo(dir: &Path, origin: Option<&str>) {
    std::fs::create_dir_all(dir).unwrap();
    git(dir, &["init", "-q", "-b", "main"]);
    git(dir, &["config", "user.email", "test@example.com"]);
    git(dir, &["config", "user.name", "Test"]);
    git(dir, &["config", "commit.gpgsign", "false"]);
    git(dir, &["config", "core.hooksPath", "/dev/null"]);
    if let Some(url) = origin {
        git(dir, &["remote", "add", "origin", url]);
    }
    std::fs::write(dir.join("README.md"), "test\n").unwrap();
    git(dir, &["add", "."]);
    git(dir, &["commit", "-qm", "initial", "--no-verify"]);
}

/// Add a remote to an existing repo.
pub fn add_remote(dir: &Path, name: &str, url: &str) {
    git(dir, &["remote", "add", name, url]);
}

/// Clone `src` (a path or URL) to `dst`, then point `origin` at `origin_url`.
/// Cloning through git rather than a raw directory copy keeps `dst/.git`
/// internally consistent, which a concurrent `git maintenance` run on some
/// platforms can otherwise break mid-copy.
pub fn clone(src: &Path, dst: &Path, origin_url: &str) {
    let parent = dst.parent().unwrap_or(dst);
    std::fs::create_dir_all(parent).unwrap();
    git(
        parent,
        &["clone", "-q", src.to_str().unwrap(), dst.to_str().unwrap()],
    );
    git(dst, &["remote", "set-url", "origin", origin_url]);
}

/// Add a linked worktree at `at` on a new branch.
pub fn add_worktree(repo: &Path, at: &Path, branch: &str) {
    git(
        repo,
        &["worktree", "add", at.to_str().unwrap(), "-b", branch],
    );
}
