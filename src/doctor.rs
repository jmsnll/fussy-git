//! `fussy-git doctor` — a read-only health report over the managed tree.
//!
//! The "where should this repository live" verdicts (misplaced / duplicate /
//! collision / no-remote / unparseable) are reused straight from the reconcile
//! planner; on top of that `doctor` runs a few cheap per-repo checks: a broken
//! gitdir, a detached `HEAD`, and — when asked — staleness by last-commit time.
//!
//! Nothing here ever mutates the filesystem or a repository. `doctor` only
//! reads.

use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use anyhow::{anyhow, bail, Result};

use crate::config;
use crate::git;
use crate::reconcile;
use crate::scan::{self, Class};
use crate::ui;

/// Options for [`run`].
#[derive(Debug, Clone, Default)]
pub struct DoctorOptions {
    /// When set, flag repositories whose last commit is older than this.
    pub stale: Option<Duration>,
}

/// Parse a short duration like `90d`, `2w`, `6mo`, `1y`, `12h`.
///
/// Grammar: one or more ASCII digits followed by a unit suffix. Units are
/// `h` (hour), `d` (day), `w` (week), `mo` (30 days) and `y` (365 days).
/// Anything else — a missing unit, a missing number, an unknown suffix, a
/// decimal point — is rejected.
pub fn parse_duration(s: &str) -> Result<Duration> {
    let s = s.trim();
    if s.is_empty() {
        bail!("empty duration (try `90d`, `2w`, `6mo`)");
    }
    let split = s
        .find(|c: char| !c.is_ascii_digit())
        .ok_or_else(|| anyhow!("duration {s:?} has no unit (try `90d`, `2w`, `6mo`)"))?;
    let (num, unit) = s.split_at(split);
    if num.is_empty() {
        bail!("duration {s:?} has no leading number");
    }
    let n: u64 = num
        .parse()
        .map_err(|_| anyhow!("duration {s:?} has an out-of-range number"))?;
    let secs_per: u64 = match unit {
        "h" => 3_600,
        "d" => 86_400,
        "w" => 604_800,
        "mo" => 2_592_000,
        "y" => 31_536_000,
        other => bail!("unknown duration unit {other:?} (use h, d, w, mo or y)"),
    };
    Ok(Duration::from_secs(n.saturating_mul(secs_per)))
}

/// Scan, plan, run the extra checks, print the grouped report, and return the
/// process exit code: `0` when the tree is clean, `3` when any issue was found.
pub fn run(cfg: &config::Config, opts: &DoctorOptions) -> Result<i32> {
    let (text, code) = report(cfg, opts)?;
    print!("{text}");
    Ok(code)
}

/// Build the full report text (sections plus the trailing summary line) and the
/// exit code. Split out from [`run`] so tests can inspect the output.
fn report(cfg: &config::Config, opts: &DoctorOptions) -> Result<(String, i32)> {
    let discovered = scan::scan(cfg)?;
    let plan = reconcile::plan(cfg, &discovered, &reconcile::PlanOptions::default())?;

    let mut broken: Vec<PathBuf> = Vec::new();
    let mut detached: Vec<(PathBuf, String)> = Vec::new();
    let mut stale: Vec<(PathBuf, u64)> = Vec::new();
    let mut total = 0usize;

    for d in &discovered {
        if matches!(d.class, Class::LinkedWorktree) {
            continue;
        }
        total += 1;
        let path = d.path.as_path();

        if is_broken(path) {
            broken.push(path.to_path_buf());
            continue;
        }
        if is_detached(path) {
            detached.push((path.to_path_buf(), short_head(path).unwrap_or_default()));
        }
        if let Some(max_age) = opts.stale {
            if let Some(age) = staleness(path, max_age) {
                stale.push((path.to_path_buf(), age));
            }
        }
    }

    // `plan.skips` carries the no-remote / unparseable verdicts (each with its
    // own reason). A broken repo reads as "no remote" to the planner too, so
    // drop anything already reported as broken.
    let mut no_remote: Vec<PathBuf> = Vec::new();
    let mut unresolved: Vec<(PathBuf, String)> = Vec::new();
    for s in &plan.skips {
        if broken.iter().any(|b| b == &s.path) {
            continue;
        }
        if s.reason.contains("adopt") {
            no_remote.push(s.path.clone());
        } else {
            unresolved.push((s.path.clone(), s.reason.clone()));
        }
    }

    let name = |p: &Path| ui::repo_name(cfg, p);
    let mut sections = String::new();
    let mut summary: Vec<String> = Vec::new();

    if !plan.moves.is_empty() {
        let _ = writeln!(
            sections,
            "  {}",
            ui::warn(&format!("misplaced ({})", plan.moves.len()))
        );
        for m in &plan.moves {
            let _ = writeln!(sections, "    {}   ->   {}", name(&m.from), name(&m.to));
        }
        summary.push(format!("{} misplaced", plan.moves.len()));
    }

    if !plan.duplicates.is_empty() {
        let _ = writeln!(
            sections,
            "  {}",
            ui::warn(&format!("duplicates ({})", plan.duplicates.len()))
        );
        for dup in &plan.duplicates {
            let _ = writeln!(sections, "    {}", dup.identity);
            let _ = writeln!(sections, "      keep  {}", name(&dup.keep));
            for o in &dup.others {
                let _ = writeln!(sections, "      copy  {}", name(o));
            }
        }
        summary.push(plural(plan.duplicates.len(), "duplicate"));
    }

    if !plan.collisions.is_empty() {
        let _ = writeln!(
            sections,
            "  {}",
            ui::danger(&format!("collisions ({})", plan.collisions.len()))
        );
        for c in &plan.collisions {
            let members = c
                .members
                .iter()
                .map(|(_, id)| id.to_string())
                .collect::<Vec<_>>()
                .join(" and ");
            let _ = writeln!(sections, "    {members}");
            let _ = writeln!(sections, "      both resolve to {}", name(&c.target));
        }
        summary.push(plural(plan.collisions.len(), "collision"));
    }

    if !no_remote.is_empty() {
        let _ = writeln!(
            sections,
            "  {}",
            ui::warn(&format!("no remote ({})", no_remote.len()))
        );
        for p in &no_remote {
            let _ = writeln!(
                sections,
                "    {}   {}",
                name(p),
                ui::dim("run `fussy-git adopt` or move it out of the root")
            );
        }
        summary.push(format!("{} no-remote", no_remote.len()));
    }

    if !unresolved.is_empty() {
        let _ = writeln!(
            sections,
            "  {}",
            ui::warn(&format!("unresolved remote ({})", unresolved.len()))
        );
        for (p, why) in &unresolved {
            let _ = writeln!(sections, "    {}   {}", name(p), ui::dim(why));
        }
        summary.push(plural(unresolved.len(), "unresolved"));
    }

    if !broken.is_empty() {
        let _ = writeln!(
            sections,
            "  {}",
            ui::danger(&format!("broken ({})", broken.len()))
        );
        for p in &broken {
            let _ = writeln!(
                sections,
                "    {}   {}",
                name(p),
                ui::dim(".git present but git cannot read the repository")
            );
        }
        summary.push(format!("{} broken", broken.len()));
    }

    if !detached.is_empty() {
        let _ = writeln!(
            sections,
            "  {}",
            ui::warn(&format!("detached HEAD ({})", detached.len()))
        );
        for (p, sha) in &detached {
            if sha.is_empty() {
                let _ = writeln!(sections, "    {}", name(p));
            } else {
                let _ = writeln!(
                    sections,
                    "    {}   {}",
                    name(p),
                    ui::dim(&format!("HEAD at {sha}"))
                );
            }
        }
        summary.push(format!("{} detached", detached.len()));
    }

    if !stale.is_empty() {
        let _ = writeln!(
            sections,
            "  {}",
            ui::warn(&format!("stale ({})", stale.len()))
        );
        for (p, age) in &stale {
            let _ = writeln!(
                sections,
                "    {}   {}",
                name(p),
                ui::dim(&format!("last commit {} ago", humanize_age(*age)))
            );
        }
        summary.push(format!("{} stale", stale.len()));
    }

    let mut out = String::new();
    let _ = writeln!(
        out,
        "{}  checked {total} {}",
        ui::header("doctor"),
        if total == 1 {
            "repository"
        } else {
            "repositories"
        }
    );

    if summary.is_empty() {
        let _ = writeln!(out, "\nno issues");
        Ok((out, 0))
    } else {
        let _ = writeln!(out);
        out.push_str(&sections);
        let _ = writeln!(out, "\n{}", summary.join(", "));
        Ok((out, 3))
    }
}

fn plural(n: usize, word: &str) -> String {
    if n == 1 {
        format!("1 {word}")
    } else {
        format!("{n} {word}s")
    }
}

/// `.git` is present but `git` refuses to treat the directory as a repository.
fn is_broken(path: &Path) -> bool {
    path.join(".git").exists() && git::run(Some(path), ["rev-parse", "--git-dir"]).is_err()
}

/// The repo has at least one commit but `HEAD` is not on a branch.
fn is_detached(path: &Path) -> bool {
    if git::run(Some(path), ["rev-parse", "--verify", "--quiet", "HEAD"]).is_err() {
        return false; // no commits yet — empty, not detached
    }
    matches!(git::current_branch(path), Ok(None) | Err(_))
}

fn short_head(path: &Path) -> Option<String> {
    git::stdout(Some(path), ["rev-parse", "--short", "HEAD"])
        .ok()
        .filter(|s| !s.is_empty())
}

/// Age in seconds of the last commit, if it is older than `max_age`.
fn staleness(path: &Path, max_age: Duration) -> Option<u64> {
    let committed: i64 = git::stdout(Some(path), ["log", "-1", "--format=%ct"])
        .ok()?
        .parse()
        .ok()?;
    let now = SystemTime::now().duration_since(UNIX_EPOCH).ok()?.as_secs() as i64;
    let age = now.checked_sub(committed)?;
    if age < 0 {
        return None;
    }
    let age = age as u64;
    (age > max_age.as_secs()).then_some(age)
}

/// A compact, hand-rolled "3d" / "5mo" / "2y1mo" rendering of an age in seconds.
fn humanize_age(secs: u64) -> String {
    let days = secs / 86_400;
    if days >= 365 {
        let years = days / 365;
        let months = (days % 365) / 30;
        if months > 0 {
            format!("{years}y{months}mo")
        } else {
            format!("{years}y")
        }
    } else if days >= 30 {
        format!("{}mo", days / 30)
    } else if days >= 1 {
        format!("{days}d")
    } else {
        format!("{}h", (secs / 3_600).max(1))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::{git, make_repo};
    use std::fs;
    use std::process::Command;

    struct Fx {
        _tmp: tempfile::TempDir,
        root: PathBuf,
        cfg: config::Config,
    }

    fn fx() -> Fx {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("git");
        fs::create_dir_all(&root).unwrap();
        let cfg_path = tmp.path().join(".fussy-git.toml");
        fs::write(&cfg_path, format!("root = {root:?}\n")).unwrap();
        let cfg = config::Config::load_from(&cfg_path).unwrap();
        Fx {
            _tmp: tmp,
            root,
            cfg,
        }
    }

    /// Rewrite the author and committer dates on `HEAD`. `testutil::git` cannot
    /// inject environment variables, so this shells out directly with the same
    /// config scrubbing.
    fn backdate_head(dir: &Path, date: &str) {
        let ok = Command::new("git")
            .current_dir(dir)
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_SYSTEM", "/dev/null")
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_TERMINAL_PROMPT", "0")
            .env("HOME", "/nonexistent-fussy-git-test-home")
            .env("GIT_AUTHOR_DATE", date)
            .env("GIT_COMMITTER_DATE", date)
            .args(["commit", "-q", "--amend", "--no-edit"])
            .status()
            .expect("spawn git");
        assert!(ok.success(), "backdating commit in {}", dir.display());
    }

    #[test]
    fn flags_misplaced_no_remote_and_broken() {
        let f = fx();
        make_repo(
            &f.root.join("wrong-place"),
            Some("git@github.com:me/right.git"),
        );
        make_repo(&f.root.join("local-only"), None);
        let borked = f.root.join("github.com/me/borked");
        make_repo(&borked, Some("git@github.com:me/borked.git"));
        fs::remove_dir_all(borked.join(".git/refs")).unwrap();

        let (text, code) = report(&f.cfg, &DoctorOptions::default()).unwrap();
        assert_eq!(code, 3, "{text}");
        assert!(text.contains("misplaced (1)"), "{text}");
        assert!(text.contains("no remote (1)"), "{text}");
        assert!(text.contains("broken (1)"), "{text}");
        // The broken repo must not also be counted as no-remote.
        assert!(!text.contains("no remote (2)"), "{text}");
        assert!(text.contains("1 misplaced"), "{text}");
    }

    #[test]
    fn clean_tree_reports_nothing() {
        let f = fx();
        make_repo(
            &f.root.join("github.com/me/tidy"),
            Some("git@github.com:me/tidy.git"),
        );
        let (text, code) = report(&f.cfg, &DoctorOptions::default()).unwrap();
        assert_eq!(code, 0, "{text}");
        assert!(text.contains("checked 1 repository"), "{text}");
        assert!(text.contains("no issues"), "{text}");
    }

    #[test]
    fn stale_flags_only_the_backdated_repo() {
        let f = fx();
        make_repo(
            &f.root.join("github.com/me/fresh"),
            Some("git@github.com:me/fresh.git"),
        );
        let ancient = f.root.join("github.com/me/ancient");
        make_repo(&ancient, Some("git@github.com:me/ancient.git"));
        backdate_head(&ancient, "2020-01-01 00:00:00 +0000");

        let opts = DoctorOptions {
            stale: Some(parse_duration("90d").unwrap()),
        };
        let (text, code) = report(&f.cfg, &opts).unwrap();
        assert_eq!(code, 3, "{text}");
        assert!(text.contains("stale (1)"), "{text}");
        assert!(text.contains("ancient"), "{text}");
        assert!(!text.contains("fresh"), "{text}");

        // Without --stale the same tree is clean.
        let (_, code) = report(&f.cfg, &DoctorOptions::default()).unwrap();
        assert_eq!(code, 0);
    }

    #[test]
    fn detached_head_is_flagged() {
        let f = fx();
        let r = f.root.join("github.com/me/det");
        make_repo(&r, Some("git@github.com:me/det.git"));
        git(&r, &["checkout", "--detach", "HEAD"]);

        let (text, code) = report(&f.cfg, &DoctorOptions::default()).unwrap();
        assert_eq!(code, 3, "{text}");
        assert!(text.contains("detached HEAD (1)"), "{text}");
    }

    #[test]
    fn parse_duration_accepts_the_documented_forms() {
        assert_eq!(
            parse_duration("90d").unwrap(),
            Duration::from_secs(90 * 86_400)
        );
        assert_eq!(
            parse_duration("2w").unwrap(),
            Duration::from_secs(14 * 86_400)
        );
        assert_eq!(
            parse_duration("6mo").unwrap(),
            Duration::from_secs(6 * 2_592_000)
        );
        assert_eq!(
            parse_duration("1y").unwrap(),
            Duration::from_secs(31_536_000)
        );
        assert_eq!(
            parse_duration("12h").unwrap(),
            Duration::from_secs(12 * 3_600)
        );
        assert_eq!(
            parse_duration("  7d  ").unwrap(),
            Duration::from_secs(7 * 86_400)
        );
    }

    #[test]
    fn parse_duration_rejects_junk() {
        for bad in ["abc", "5x", "", "   ", "10", "d10", "1.5d", "-3d", "1 d"] {
            assert!(parse_duration(bad).is_err(), "{bad:?} should be rejected");
        }
    }

    #[test]
    fn humanize_age_is_compact() {
        assert_eq!(humanize_age(3 * 86_400), "3d");
        assert_eq!(humanize_age(90 * 86_400), "3mo");
        assert_eq!(humanize_age(400 * 86_400), "1y1mo");
        assert_eq!(humanize_age(365 * 86_400), "1y");
        assert_eq!(humanize_age(7_200), "2h");
    }
}
