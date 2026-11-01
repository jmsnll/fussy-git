//! Running the user's lifecycle hooks (`[hooks]` in the config).
//!
//! Each hook is a shell command run with the repository as its working
//! directory. Its stdout is sent to fussy-git's **stderr**: stdout is reserved
//! for data (`cd "$(fussy-git get …)"`, `--json`), and a chatty hook must not
//! corrupt it. A failing hook is a warning, never an error — the clone, move or
//! worktree it follows has already happened.

use std::path::Path;
use std::process::{Command, Stdio};

/// Run every command in `cmds` in `dir`, returning one warning per hook that
/// failed. `event` names the hook in the warning, for example `post_get`.
pub fn run(event: &str, cmds: &[String], dir: &Path) -> Vec<String> {
    let mut warnings = Vec::new();
    for cmd in cmds {
        let status = Command::new("sh")
            .arg("-c")
            .arg(cmd)
            .current_dir(dir)
            .stdin(Stdio::null())
            .stdout(Stdio::from(std::io::stderr()))
            .status();
        match status {
            Ok(s) if s.success() => {}
            Ok(s) => warnings.push(format!("{event} hook `{cmd}` exited with {s}")),
            Err(e) => warnings.push(format!("{event} hook `{cmd}` failed to start: {e}")),
        }
    }
    warnings
}
