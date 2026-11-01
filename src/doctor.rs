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
use serde::Serialize;

use crate::config;
use crate::git;
use crate::reconcile;
use crate::scan::{self, Class};
use crate::ui::{self, Format};
use crate::worktree;

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

/// Everything `doctor` found, grouped by kind. Serialised as-is for `--json`.
#[derive(Debug, Clone, Default, Serialize)]
pub struct Findings {
    /// Repositories examined (linked worktrees excluded).
    pub checked: usize,
    pub misplaced: Vec<Misplaced>,
    pub duplicates: Vec<reconcile::Duplicate>,
    pub collisions: Vec<reconcile::Collision>,
    pub no_remote: Vec<PathBuf>,
    pub unresolved: Vec<Unresolved>,
    pub broken: Vec<PathBuf>,
    pub detached: Vec<Detached>,
    pub stale: Vec<Stale>,
    /// Linked worktrees whose directory is gone, whichever tool made them.
    pub broken_worktrees: Vec<worktree::Worktree>,
}

#[derive(Debug, Clone, Serialize)]
pub struct Misplaced {
    pub from: PathBuf,
    pub to: PathBuf,
}

#[derive(Debug, Clone, Serialize)]
pub struct Unresolved {
    pub path: PathBuf,
    pub reason: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct Detached {
    pub path: PathBuf,
    /// Short `HEAD` commit, empty if it could not be read.
    pub head: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct Stale {
    pub path: PathBuf,
    /// Seconds since the last commit.
    pub age_secs: u64,
}

impl Findings {
    pub fn is_clean(&self) -> bool {
        self.misplaced.is_empty()
            && self.duplicates.is_empty()
            && self.collisions.is_empty()
            && self.no_remote.is_empty()
            && self.unresolved.is_empty()
            && self.broken.is_empty()
            && self.detached.is_empty()
            && self.stale.is_empty()
            && self.broken_worktrees.is_empty()
    }

    /// `0` when the tree is clean, `3` when any issue was found.
    pub fn exit_code(&self) -> i32 {
        if self.is_clean() {
            0
        } else {
            3
        }
    }
}

/// Scan, plan, run the extra checks, print the grouped report (or JSON), and
/// return the process exit code: `0` when the tree is clean, `3` when any issue
/// was found.
pub fn run(cfg: &config::Config, opts: &DoctorOptions, format: Format) -> Result<i32> {
    let findings = collect(cfg, opts)?;
    if format.is_json() {
        ui::print_json(&findings)?;
    } else {
        print!("{}", render(cfg, &findings));
    }
    Ok(findings.exit_code())
}

/// The rendered text and exit code, for tests.
#[cfg(test)]
fn report(cfg: &config::Config, opts: &DoctorOptions) -> Result<(String, i32)> {
    let findings = collect(cfg, opts)?;
    Ok((render(cfg, &findings), findings.exit_code()))
}

/// Gather every finding. Reads only; never mutates anything.
pub fn collect(cfg: &config::Config, opts: &DoctorOptions) -> Result<Findings> {
    let discovered = scan::scan(cfg)?;
    let plan = reconcile::plan(cfg, &discovered, &reconcile::PlanOptions::default())?;
    let mut f = Findings::default();

    for d in &discovered {
        if matches!(d.class, Class::LinkedWorktree) {
            continue;
        }
        f.checked += 1;
        let path = d.path.as_path();

        if is_broken(path) {
            f.broken.push(path.to_path_buf());
            continue;
        }
        if is_detached(path) {
            f.detached.push(Detached {
                path: path.to_path_buf(),
                head: short_head(path).unwrap_or_default(),
            });
        }
        if let Some(max_age) = opts.stale {
            if let Some(age) = staleness(path, max_age) {
                f.stale.push(Stale {
                    path: path.to_path_buf(),
                    age_secs: age,
                });
            }
        }
    }

    // `plan.skips` carries the no-remote / unparseable verdicts (each with its
    // own reason). A broken repo reads as "no remote" to the planner too, so
    // drop anything already reported as broken.
    for s in &plan.skips {
        if f.broken.iter().any(|b| b == &s.path) {
            continue;
        }
        if s.reason.contains("adopt") {
            f.no_remote.push(s.path.clone());
        } else {
            f.unresolved.push(Unresolved {
                path: s.path.clone(),
                reason: s.reason.clone(),
            });
        }
    }

    f.misplaced = plan
        .moves
        .iter()
        .map(|m| Misplaced {
            from: m.from.clone(),
            to: m.to.clone(),
        })
        .collect();
    f.duplicates = plan.duplicates;
    f.collisions = plan.collisions;
    f.broken_worktrees = worktree::list(cfg, None)?
        .into_iter()
        .filter(|w| w.missing)
        .collect();
    Ok(f)
}

/// The grouped, human-readable report: a header, one section per kind of
/// finding, and a trailing summary line.
fn render(cfg: &config::Config, f: &Findings) -> String {
    let name = |p: &Path| ui::repo_name(cfg, p);
    let mut sections = String::new();
    let mut summary: Vec<String> = Vec::new();

    if !f.misplaced.is_empty() {
        let _ = writeln!(
            sections,
            "  {}",
            ui::warn(&format!("misplaced ({})", f.misplaced.len()))
        );
        for m in &f.misplaced {
            let _ = writeln!(sections, "    {}   ->   {}", name(&m.from), name(&m.to));
        }
        summary.push(format!("{} misplaced", f.misplaced.len()));
    }

    if !f.duplicates.is_empty() {
        let _ = writeln!(
            sections,
            "  {}",
            ui::warn(&format!("duplicates ({})", f.duplicates.len()))
        );
        for dup in &f.duplicates {
            let _ = writeln!(sections, "    {}", dup.identity);
            let _ = writeln!(sections, "      keep  {}", name(&dup.keep));
            for o in &dup.others {
                let _ = writeln!(sections, "      copy  {}", name(o));
            }
        }
        summary.push(plural(f.duplicates.len(), "duplicate"));
    }

    if !f.collisions.is_empty() {
        let _ = writeln!(
            sections,
            "  {}",
            ui::danger(&format!("collisions ({})", f.collisions.len()))
        );
        for c in &f.collisions {
            let members = c
                .members
                .iter()
                .map(|m| m.identity.to_string())
                .collect::<Vec<_>>()
                .join(" and ");
            let _ = writeln!(sections, "    {members}");
            let _ = writeln!(sections, "      both resolve to {}", name(&c.target));
        }
        summary.push(plural(f.collisions.len(), "collision"));
    }

    if !f.no_remote.is_empty() {
        let _ = writeln!(
            sections,
            "  {}",
            ui::warn(&format!("no remote ({})", f.no_remote.len()))
        );
        for p in &f.no_remote {
            let _ = writeln!(
                sections,
                "    {}   {}",
                name(p),
                ui::dim("run `fussy-git adopt` or move it out of the root")
            );
        }
        summary.push(format!("{} no-remote", f.no_remote.len()));
    }

    if !f.unresolved.is_empty() {
        let _ = writeln!(
            sections,
            "  {}",
            ui::warn(&format!("unresolved remote ({})", f.unresolved.len()))
        );
        for u in &f.unresolved {
            let _ = writeln!(sections, "    {}   {}", name(&u.path), ui::dim(&u.reason));
        }
        summary.push(plural(f.unresolved.len(), "unresolved"));
    }

    if !f.broken.is_empty() {
        let _ = writeln!(
            sections,
            "  {}",
            ui::danger(&format!("broken ({})", f.broken.len()))
        );
        for p in &f.broken {
            let _ = writeln!(
                sections,
                "    {}   {}",
                name(p),
                ui::dim(".git present but git cannot read the repository")
            );
        }
        summary.push(format!("{} broken", f.broken.len()));
    }

    if !f.detached.is_empty() {
        let _ = writeln!(
            sections,
            "  {}",
            ui::warn(&format!("detached HEAD ({})", f.detached.len()))
        );
        for Detached { path: p, head: sha } in &f.detached {
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
        summary.push(format!("{} detached", f.detached.len()));
    }

    if !f.stale.is_empty() {
        let _ = writeln!(
            sections,
            "  {}",
            ui::warn(&format!("stale ({})", f.stale.len()))
        );
        for Stale {
            path: p,
            age_secs: age,
        } in &f.stale
        {
            let _ = writeln!(
                sections,
                "    {}   {}",
                name(p),
                ui::dim(&format!("last commit {} ago", humanize_age(*age)))
            );
        }
        summary.push(format!("{} stale", f.stale.len()));
    }

    if !f.broken_worktrees.is_empty() {
        let _ = writeln!(
            sections,
            "  {}",
            ui::warn(&format!("broken worktrees ({})", f.broken_worktrees.len()))
        );
        for w in &f.broken_worktrees {
            let _ = writeln!(
                sections,
                "    {}   {}",
                ui::repo_name(cfg, &w.path),
                ui::dim(&format!(
                    "{} of {}: directory missing",
                    w.branch.as_deref().unwrap_or("detached"),
                    w.repo
                ))
            );
        }
        let _ = writeln!(
            sections,
            "    {}",
            ui::dim("clean up with `wt remove` or `git worktree prune` in the repository")
        );
        summary.push(format!("{} broken worktree(s)", f.broken_worktrees.len()));
    }

    let total = f.checked;
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
    } else {
        let _ = writeln!(out);
        out.push_str(&sections);
        let _ = writeln!(out, "\n{}", summary.join(", "));
    }
    out
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
