//! The reconcile engine: compare what `scan` discovered against where every
//! repository *should* live, produce a categorised [`Plan`], and — on request —
//! [`apply`] it.
//!
//! Nothing here ever edits repository contents. A "move" is a filesystem
//! rename (with a copy+verify+delete fallback across devices); duplicates and
//! collisions are only ever reported, never resolved by deletion.

use std::collections::{BTreeMap, HashSet};
use std::fmt::{self, Write as _};
use std::path::{Path, PathBuf};

use anyhow::Result;

use crate::config::{self, OnCollision};
use crate::fsops;
use crate::git;
use crate::identity::Identity;
use crate::preflight;
use crate::scan::{self, Class};
use crate::ui;

/// Width of the left column that holds the change verb (`move`, `collision`, …).
const GUTTER: usize = 11;

const NO_REMOTE_REASON: &str = "no remote — use `fussy-git adopt`";

// --- plan types -------------------------------------------------------------

/// Why a repository needs to move.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MoveReason {
    /// Same path shape, but a segment differs only in ASCII case (typically the
    /// owner), for example `github.com/Foo/bar` becomes `github.com/foo/bar`.
    OwnerCase,
    /// The number of path segments changed, for example a GitLab subgroup that
    /// was flattened or gained a level.
    Subgroup,
    /// Anything else: the repo is simply in the wrong place.
    Relocated,
}

impl fmt::Display for MoveReason {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            MoveReason::OwnerCase => "owner-case fold",
            MoveReason::Subgroup => "subgroup layout",
            MoveReason::Relocated => "relocated",
        })
    }
}

/// A single repository that should be relocated.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Move {
    pub from: PathBuf,
    pub to: PathBuf,
    pub identity: Identity,
    pub reason: MoveReason,
}

/// Several working copies that resolve to the same canonical path.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Duplicate {
    pub identity: Identity,
    /// The copy to keep (already canonical if one is, else the first by path).
    pub keep: PathBuf,
    /// The remaining copies, sorted by path. Never touched by [`apply`].
    pub others: Vec<PathBuf>,
    /// True when every copy has the same `HEAD` commit.
    pub identical_head: bool,
}

/// Two or more *distinct* identities whose canonical paths collide (typically
/// case-insensitively, on a case-folding filesystem).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Collision {
    /// A representative canonical path for the colliding set.
    pub target: PathBuf,
    /// Each colliding working copy and the identity it resolves to.
    pub members: Vec<(PathBuf, Identity)>,
}

/// A repository the plan deliberately leaves alone, with the reason why.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Skip {
    pub path: PathBuf,
    pub reason: String,
}

/// The full categorised result of planning a reconcile.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Plan {
    pub moves: Vec<Move>,
    pub duplicates: Vec<Duplicate>,
    pub collisions: Vec<Collision>,
    pub skips: Vec<Skip>,
    /// Count of repositories already sitting at their canonical path.
    pub ok: usize,
}

impl Plan {
    /// True if anything is out of place (moves, duplicates or collisions).
    /// Skips alone are not drift.
    pub fn has_drift(&self) -> bool {
        !self.moves.is_empty() || !self.duplicates.is_empty() || !self.collisions.is_empty()
    }

    /// True if the plan contains nothing at all.
    pub fn is_empty(&self) -> bool {
        self.moves.is_empty()
            && self.duplicates.is_empty()
            && self.collisions.is_empty()
            && self.skips.is_empty()
            && self.ok == 0
    }
}

impl Plan {
    /// A scannable, optionally coloured rendering of the plan: a one-line
    /// summary, then one indented entry per change with the change verb in a
    /// fixed left column. Every path is shown relative to its managed root.
    pub fn render(&self, cfg: &config::Config) -> String {
        let name = |p: &Path| ui::repo_name(cfg, p);
        let mut out = String::new();

        let _ = writeln!(
            out,
            "{}  {}",
            ui::header("reconcile"),
            ui::dim(&self.summary())
        );

        if self.moves.is_empty()
            && self.duplicates.is_empty()
            && self.collisions.is_empty()
            && self.skips.is_empty()
        {
            return out;
        }
        let _ = writeln!(out);

        for m in &self.moves {
            let _ = writeln!(out, "  {}{}", verb("move", ui::accent), name(&m.from));
            let _ = writeln!(
                out,
                "  {}-> {}   {}",
                " ".repeat(GUTTER),
                name(&m.to),
                ui::dim(&m.reason.to_string())
            );
        }

        for d in &self.duplicates {
            let _ = writeln!(out, "  {}{}", verb("duplicate", ui::warn), d.identity);
            let _ = writeln!(out, "  {}keep  {}", " ".repeat(GUTTER), name(&d.keep));
            for o in &d.others {
                let _ = writeln!(out, "  {}copy  {}", " ".repeat(GUTTER), name(o));
            }
            let heads = if d.identical_head {
                "same HEAD"
            } else {
                "HEADs differ"
            };
            let _ = writeln!(out, "  {}{}", " ".repeat(GUTTER), ui::dim(heads));
        }

        for c in &self.collisions {
            let members = c
                .members
                .iter()
                .map(|(_, id)| id.to_string())
                .collect::<Vec<_>>()
                .join(" and ");
            let _ = writeln!(out, "  {}{}", verb("collision", ui::danger), members);
            let _ = writeln!(
                out,
                "  {}both resolve to {}",
                " ".repeat(GUTTER),
                name(&c.target)
            );
            let _ = writeln!(
                out,
                "  {}{}",
                " ".repeat(GUTTER),
                ui::dim("choose with --on-collision=suffix|skip")
            );
        }

        for s in &self.skips {
            let _ = writeln!(
                out,
                "  {}{}   {}",
                verb("skip", ui::dim),
                name(&s.path),
                ui::dim(&s.reason)
            );
        }

        out
    }

    /// One-line summary of the plan's counts, listing only what is non-zero.
    pub fn summary(&self) -> String {
        let mut parts = Vec::new();
        if !self.moves.is_empty() {
            parts.push(format!("{} to move", self.moves.len()));
        }
        if !self.duplicates.is_empty() {
            parts.push(plural(self.duplicates.len(), "duplicate"));
        }
        if !self.collisions.is_empty() {
            parts.push(plural(self.collisions.len(), "collision"));
        }
        if !self.skips.is_empty() {
            parts.push(format!("{} skipped", self.skips.len()));
        }
        if self.ok > 0 {
            parts.push(format!("{} in place", self.ok));
        }
        if parts.is_empty() {
            "nothing to do".to_string()
        } else {
            parts.join(", ")
        }
    }
}

/// The change verb for the left column, coloured and padded to [`GUTTER`].
fn verb(word: &str, paint: fn(&str) -> String) -> String {
    ui::pad_to(&paint(word), word.chars().count(), GUTTER)
}

fn plural(n: usize, word: &str) -> String {
    if n == 1 {
        format!("1 {word}")
    } else {
        format!("{n} {word}s")
    }
}

/// Options for [`plan`].
#[derive(Debug, Clone, Default)]
pub struct PlanOptions {
    /// Treat a dirty working tree as movable rather than skipping it.
    pub allow_dirty: bool,
}

// --- planning --------------------------------------------------------------

struct Eligible<'a> {
    d: &'a scan::Discovered,
    identity: &'a Identity,
    canonical: PathBuf,
}

/// Build a [`Plan`] from a scan result.
pub fn plan(
    cfg: &config::Config,
    discovered: &[scan::Discovered],
    opts: &PlanOptions,
) -> Result<Plan> {
    let mut plan = Plan::default();
    let mut eligibles: Vec<Eligible> = Vec::new();

    for d in discovered {
        match &d.class {
            Class::LinkedWorktree => continue,
            Class::NoRemote => {
                plan.skips.push(Skip {
                    path: d.path.clone(),
                    reason: NO_REMOTE_REASON.to_string(),
                });
                continue;
            }
            Class::UnparseableRemote { reason, .. } => {
                plan.skips.push(Skip {
                    path: d.path.clone(),
                    reason: reason.clone(),
                });
                continue;
            }
            Class::Ok | Class::Misplaced { .. } => {}
        }

        let Some(identity) = d.identity.as_ref() else {
            plan.skips.push(Skip {
                path: d.path.clone(),
                reason: "remote did not resolve to an identity".to_string(),
            });
            continue;
        };

        // Recompute the canonical path from the template rather than trusting
        // the relative path scan stashed on the class.
        let rel = cfg.template_for(&identity.host).render(identity, None);
        let canonical = cfg.root.join(&rel);
        eligibles.push(Eligible {
            d,
            identity,
            canonical,
        });
    }

    // Group by exact canonical path.
    let mut groups: BTreeMap<PathBuf, Vec<usize>> = BTreeMap::new();
    for (i, e) in eligibles.iter().enumerate() {
        groups.entry(e.canonical.clone()).or_default().push(i);
    }

    // Bucket distinct canonical paths that fold together case-insensitively.
    let mut ci: BTreeMap<String, Vec<PathBuf>> = BTreeMap::new();
    for key in groups.keys() {
        ci.entry(key.to_string_lossy().to_lowercase())
            .or_default()
            .push(key.clone());
    }
    let colliding: HashSet<PathBuf> = ci
        .values()
        .filter(|paths| paths.len() > 1)
        .flatten()
        .cloned()
        .collect();

    for paths in ci.values() {
        if paths.len() <= 1 {
            continue;
        }
        let mut members: Vec<(PathBuf, Identity)> = Vec::new();
        for key in paths {
            for &idx in &groups[key] {
                members.push((
                    eligibles[idx].d.path.clone(),
                    eligibles[idx].identity.clone(),
                ));
            }
        }
        members.sort_by(|a, b| a.0.cmp(&b.0));
        let target = paths.iter().min().cloned().expect("bucket has >1 path");
        plan.collisions.push(Collision { target, members });
    }

    for (key, idxs) in &groups {
        if colliding.contains(key) {
            continue;
        }

        if idxs.len() == 1 {
            let e = &eligibles[idxs[0]];
            if e.d.path == *key {
                plan.ok += 1;
                continue;
            }
            let to_rel = key.strip_prefix(&cfg.root).unwrap_or(key);
            let reason = move_reason(&e.d.rel, to_rel);
            let safety = preflight::inspect(&e.d.path)?;
            if safety.movable(opts.allow_dirty) {
                plan.moves.push(Move {
                    from: e.d.path.clone(),
                    to: key.clone(),
                    identity: e.identity.clone(),
                    reason,
                });
            } else {
                plan.skips.push(Skip {
                    path: e.d.path.clone(),
                    reason: immovable_reason(&safety, opts.allow_dirty),
                });
            }
            continue;
        }

        // More than one copy resolves here: a duplicate.
        let keep = idxs
            .iter()
            .copied()
            .find(|&i| eligibles[i].d.path == *key)
            .or_else(|| {
                idxs.iter()
                    .copied()
                    .min_by(|&a, &b| eligibles[a].d.path.cmp(&eligibles[b].d.path))
            })
            .expect("group is non-empty");
        let keep_path = eligibles[keep].d.path.clone();
        let mut others: Vec<PathBuf> = idxs
            .iter()
            .copied()
            .filter(|&i| i != keep)
            .map(|i| eligibles[i].d.path.clone())
            .collect();
        others.sort();

        let identical_head = {
            let mut all: Vec<&Path> = vec![keep_path.as_path()];
            all.extend(others.iter().map(|p| p.as_path()));
            heads_identical(&all)
        };

        plan.duplicates.push(Duplicate {
            identity: eligibles[keep].identity.clone(),
            keep: keep_path,
            others,
            identical_head,
        });
    }

    plan.moves.sort_by(|a, b| a.from.cmp(&b.from));
    plan.duplicates.sort_by(|a, b| a.keep.cmp(&b.keep));
    plan.collisions.sort_by(|a, b| a.target.cmp(&b.target));
    plan.skips.sort_by(|a, b| a.path.cmp(&b.path));

    Ok(plan)
}

fn move_reason(from_rel: &Path, to_rel: &Path) -> MoveReason {
    let from: Vec<String> = from_rel
        .iter()
        .map(|s| s.to_string_lossy().into_owned())
        .collect();
    let to: Vec<String> = to_rel
        .iter()
        .map(|s| s.to_string_lossy().into_owned())
        .collect();

    if from.len() == to.len() {
        let folded_equal = from.join("/").to_lowercase() == to.join("/").to_lowercase();
        return if folded_equal && from != to {
            MoveReason::OwnerCase
        } else {
            MoveReason::Relocated
        };
    }

    // Different depth is only a "subgroup" change when it is specifically the
    // owner path that grew or shrank: same host, same repo name, at least the
    // `host/owner/repo` shape on both sides.
    let same_ends =
        from.len() >= 3 && to.len() >= 3 && from.first() == to.first() && from.last() == to.last();
    if same_ends {
        MoveReason::Subgroup
    } else {
        MoveReason::Relocated
    }
}

fn immovable_reason(safety: &preflight::Safety, allow_dirty: bool) -> String {
    let mut parts = safety.blockers();
    if !allow_dirty && safety.dirty {
        parts.push("uncommitted changes (retry with --allow-dirty)".to_string());
    }
    if parts.is_empty() {
        "not safe to move".to_string()
    } else {
        parts.join("; ")
    }
}

fn head_rev(repo: &Path) -> Option<String> {
    git::stdout(Some(repo), ["rev-parse", "HEAD"])
        .ok()
        .filter(|s| !s.is_empty())
}

fn heads_identical(paths: &[&Path]) -> bool {
    let mut revs = paths.iter().map(|p| head_rev(p));
    let first = match revs.next() {
        Some(Some(r)) => r,
        _ => return false,
    };
    revs.all(|r| r.as_deref() == Some(first.as_str()))
}

// --- applying -------------------------------------------------------------

/// Options for [`apply`].
#[derive(Debug, Clone, Default)]
pub struct ApplyOptions {
    pub on_collision: OnCollision,
    /// Leave a symlink at the old path pointing at the new one.
    pub leave_symlink: bool,
    pub allow_dirty: bool,
}

/// What [`apply`] actually did.
#[derive(Debug, Clone, Default)]
pub struct ApplyReport {
    pub moved: Vec<(PathBuf, PathBuf)>,
    pub failed: Vec<(PathBuf, String)>,
    pub skipped: Vec<Skip>,
    /// Non-fatal notes (hook failures, a symlink that could not be created).
    pub warnings: Vec<String>,
}

impl ApplyReport {
    /// A scannable, optionally coloured rendering of what `apply` did, mirroring
    /// [`Plan::render`]'s layout. Paths are relative to their managed root.
    pub fn render(&self, cfg: &config::Config) -> String {
        let name = |p: &Path| ui::repo_name(cfg, p);
        let mut out = String::new();

        let mut summary = Vec::new();
        summary.push(format!("{} moved", self.moved.len()));
        if !self.failed.is_empty() {
            summary.push(ui::danger(&format!("{} failed", self.failed.len())));
        }
        if !self.skipped.is_empty() {
            summary.push(format!("{} skipped", self.skipped.len()));
        }
        let _ = writeln!(out, "{}  {}", ui::header("reconcile"), summary.join(", "));

        if self.moved.is_empty() && self.failed.is_empty() && self.skipped.is_empty() {
            return out;
        }
        let _ = writeln!(out);

        for (from, to) in &self.moved {
            let _ = writeln!(
                out,
                "  {}{}  ->  {}",
                verb("moved", ui::ok),
                name(from),
                name(to)
            );
        }
        for (path, why) in &self.failed {
            let _ = writeln!(
                out,
                "  {}{}   {}",
                verb("failed", ui::danger),
                name(path),
                ui::dim(why)
            );
        }
        for s in &self.skipped {
            let _ = writeln!(
                out,
                "  {}{}   {}",
                verb("skip", ui::dim),
                name(&s.path),
                ui::dim(&s.reason)
            );
        }

        out
    }
}

/// Carry out `plan`. Moves are applied; duplicates and collisions are recorded
/// as skipped unless `on_collision == Suffix`, which disambiguates the
/// colliding copies with a `-2`, `-3`… suffix.
pub fn apply(cfg: &config::Config, plan: &Plan, opts: &ApplyOptions) -> Result<ApplyReport> {
    let mut report = ApplyReport::default();

    for mv in &plan.moves {
        apply_move(cfg, mv, opts, &mut report);
    }

    for dup in &plan.duplicates {
        report.skipped.push(Skip {
            path: dup.keep.clone(),
            reason: format!(
                "duplicate of {} — {} other copy(ies) left untouched",
                dup.identity,
                dup.others.len()
            ),
        });
    }

    for col in &plan.collisions {
        match opts.on_collision {
            OnCollision::Suffix => apply_collision_suffix(col, &mut report),
            OnCollision::Fail | OnCollision::Skip => {
                report.skipped.push(Skip {
                    path: col.target.clone(),
                    reason: "collision — rerun with on_collision = suffix to disambiguate"
                        .to_string(),
                });
            }
        }
    }

    Ok(report)
}

fn apply_move(cfg: &config::Config, mv: &Move, opts: &ApplyOptions, report: &mut ApplyReport) {
    let from = mv.from.as_path();
    let to = mv.to.as_path();

    match preflight::inspect(from) {
        Ok(safety) if safety.movable(opts.allow_dirty) => {}
        Ok(safety) => {
            report.skipped.push(Skip {
                path: from.to_path_buf(),
                reason: immovable_reason(&safety, opts.allow_dirty),
            });
            return;
        }
        Err(e) => {
            report
                .failed
                .push((from.to_path_buf(), format!("preflight failed: {e}")));
            return;
        }
    }

    if to.exists() {
        report
            .failed
            .push((from.to_path_buf(), "target exists".to_string()));
        return;
    }

    if let Some(parent) = to.parent() {
        if let Err(e) = std::fs::create_dir_all(parent) {
            report.failed.push((
                from.to_path_buf(),
                format!("creating {}: {e}", parent.display()),
            ));
            return;
        }
    }

    if let Err(e) = fsops::move_dir(from, to) {
        report
            .failed
            .push((from.to_path_buf(), format!("move failed: {e}")));
        return;
    }
    report.moved.push((from.to_path_buf(), to.to_path_buf()));

    if opts.leave_symlink {
        if let Err(e) = std::os::unix::fs::symlink(to, from) {
            report
                .warnings
                .push(format!("no symlink left at {}: {e}", from.display()));
        }
    }

    if let Some(root) = fsops::managed_root(cfg, from) {
        fsops::prune_empty_dirs(from.parent(), root);
    }

    report.warnings.extend(run_post_move_hooks(cfg, to));
}

fn apply_collision_suffix(col: &Collision, report: &mut ApplyReport) {
    let Some(parent) = col.target.parent() else {
        report.failed.push((
            col.target.clone(),
            "collision target has no parent".to_string(),
        ));
        return;
    };
    let base = col
        .target
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| "repo".to_string());

    if let Err(e) = std::fs::create_dir_all(parent) {
        report.failed.push((
            col.target.clone(),
            format!("creating {}: {e}", parent.display()),
        ));
        return;
    }

    let mut n = 2u32;
    for (path, _identity) in &col.members {
        if *path == col.target {
            continue; // the canonical occupant, if any, stays put
        }
        let dest = loop {
            let cand = parent.join(format!("{base}-{n}"));
            n += 1;
            if !cand.exists() {
                break cand;
            }
        };
        match fsops::move_dir(path, &dest) {
            Ok(()) => report.moved.push((path.clone(), dest)),
            Err(e) => report
                .failed
                .push((path.clone(), format!("collision move failed: {e}"))),
        }
    }
}

/// Run `cfg.hooks.post_move` in `dir`. Failures are collected as warnings, not
/// propagated.
fn run_post_move_hooks(cfg: &config::Config, dir: &Path) -> Vec<String> {
    let mut warnings = Vec::new();
    for cmd in &cfg.hooks.post_move {
        let status = std::process::Command::new("sh")
            .arg("-c")
            .arg(cmd)
            .current_dir(dir)
            .status();
        match status {
            Ok(s) if s.success() => {}
            Ok(s) => warnings.push(format!("post_move hook `{cmd}` exited with {s}")),
            Err(e) => warnings.push(format!("post_move hook `{cmd}` failed to start: {e}")),
        }
    }
    warnings
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::{add_worktree, make_repo};
    use std::fs;

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

    fn plan_default(f: &Fx) -> Plan {
        let discovered = scan::scan(&f.cfg).unwrap();
        plan(&f.cfg, &discovered, &PlanOptions::default()).unwrap()
    }

    #[test]
    fn reports_ok_and_a_relocated_move() {
        let f = fx();
        make_repo(
            &f.root.join("github.com/me/right"),
            Some("git@github.com:me/right.git"),
        );
        // Same depth, different name — a plain relocation.
        make_repo(
            &f.root.join("github.com/me/oldname"),
            Some("git@github.com:me/newname.git"),
        );

        let p = plan_default(&f);
        assert_eq!(p.ok, 1);
        assert_eq!(p.moves.len(), 1);
        assert_eq!(p.moves[0].from, f.root.join("github.com/me/oldname"));
        assert_eq!(p.moves[0].to, f.root.join("github.com/me/newname"));
        assert_eq!(p.moves[0].reason, MoveReason::Relocated);
        assert!(p.has_drift());
        assert!(!p.is_empty());
    }

    #[test]
    fn detects_owner_case_fold() {
        let f = fx();
        make_repo(
            &f.root.join("github.com/Me/thing"),
            Some("git@github.com:me/thing.git"),
        );
        let p = plan_default(&f);
        assert_eq!(p.moves.len(), 1);
        assert_eq!(p.moves[0].reason, MoveReason::OwnerCase);
    }

    #[test]
    fn detects_subgroup_layout_change() {
        let f = fx();
        make_repo(
            &f.root.join("gitlab.com/acme/api"),
            Some("https://gitlab.com/acme/backend/api.git"),
        );
        let p = plan_default(&f);
        assert_eq!(p.moves.len(), 1);
        assert_eq!(p.moves[0].reason, MoveReason::Subgroup);
        assert_eq!(p.moves[0].to, f.root.join("gitlab.com/acme/backend/api"));
    }

    #[test]
    fn groups_duplicates_with_identical_head() {
        let f = fx();
        let a = f.root.join("copy-a");
        make_repo(&a, Some("git@github.com:me/dup.git"));
        let b = f.root.join("copy-b");
        crate::testutil::clone(&a, &b, "git@github.com:me/dup.git");

        let p = plan_default(&f);
        assert_eq!(p.duplicates.len(), 1);
        assert_eq!(p.duplicates[0].keep, a);
        assert_eq!(p.duplicates[0].others, vec![b]);
        assert!(p.duplicates[0].identical_head);
        assert!(p.moves.is_empty());
    }

    #[test]
    fn skips_no_remote_and_unparseable_remote() {
        let f = fx();
        make_repo(&f.root.join("local-only"), None);
        make_repo(&f.root.join("weird"), Some("not-a-url"));

        let p = plan_default(&f);
        assert_eq!(p.skips.len(), 2);
        assert!(p.skips.iter().any(|s| s.reason.contains("adopt")));
        assert!(!p.has_drift());
    }

    #[test]
    fn skips_dirty_repo_unless_allow_dirty() {
        let f = fx();
        let repo = f.root.join("stray");
        make_repo(&repo, Some("git@github.com:me/stray.git"));
        fs::write(repo.join("dirt.txt"), "x").unwrap();

        let discovered = scan::scan(&f.cfg).unwrap();
        let p = plan(&f.cfg, &discovered, &PlanOptions::default()).unwrap();
        assert_eq!(p.moves.len(), 0);
        assert_eq!(p.skips.len(), 1);

        let p2 = plan(&f.cfg, &discovered, &PlanOptions { allow_dirty: true }).unwrap();
        assert_eq!(p2.moves.len(), 1);
    }

    #[test]
    fn detects_case_insensitive_collision() {
        let f = fx();
        make_repo(&f.root.join("x"), Some("git@github.com:me/repo.git"));
        make_repo(&f.root.join("y"), Some("git@github.com:ME/repo.git"));

        let p = plan_default(&f);
        assert_eq!(p.collisions.len(), 1);
        assert_eq!(p.collisions[0].members.len(), 2);
        assert!(p.moves.is_empty());
        assert!(p.has_drift());
    }

    #[test]
    fn linked_worktrees_are_not_in_the_plan() {
        let f = fx();
        let main = f.root.join("github.com/me/proj");
        make_repo(&main, Some("git@github.com:me/proj.git"));
        add_worktree(&main, &f.root.join("wt/proj-x"), "feature");

        let p = plan_default(&f);
        assert_eq!(p.ok, 1);
        assert!(p.moves.is_empty());
        assert!(p.skips.is_empty());
    }

    #[test]
    fn apply_moves_repo_to_canonical_and_prunes_empty_parents() {
        let f = fx();
        make_repo(
            &f.root.join("deep/nested/stray"),
            Some("git@github.com:me/stray.git"),
        );
        let p = plan_default(&f);
        let report = apply(&f.cfg, &p, &ApplyOptions::default()).unwrap();

        assert_eq!(report.moved.len(), 1);
        assert!(report.failed.is_empty());
        assert!(f.root.join("github.com/me/stray/.git").exists());
        assert!(!f.root.join("deep/nested/stray").exists());
        assert!(!f.root.join("deep/nested").exists());
        assert!(!f.root.join("deep").exists());
    }

    #[test]
    fn apply_leave_symlink_leaves_a_working_link() {
        let f = fx();
        make_repo(&f.root.join("stray"), Some("git@github.com:me/stray.git"));
        let p = plan_default(&f);
        let opts = ApplyOptions {
            leave_symlink: true,
            ..Default::default()
        };
        apply(&f.cfg, &p, &opts).unwrap();

        let link = f.root.join("stray");
        assert!(link.symlink_metadata().unwrap().file_type().is_symlink());
        assert_eq!(
            fs::read_link(&link).unwrap(),
            f.root.join("github.com/me/stray")
        );
        assert!(
            link.join(".git").exists(),
            "link resolves to the moved repo"
        );
    }

    #[test]
    fn apply_reports_target_exists_as_failed_not_a_panic() {
        let f = fx();
        make_repo(&f.root.join("stray"), Some("git@github.com:me/stray.git"));
        fs::create_dir_all(f.root.join("github.com/me/stray")).unwrap();
        fs::write(f.root.join("github.com/me/stray/keep"), "x").unwrap();

        let p = plan_default(&f);
        let report = apply(&f.cfg, &p, &ApplyOptions::default()).unwrap();
        assert!(report.moved.is_empty());
        assert_eq!(report.failed.len(), 1);
        assert!(report.failed[0].1.contains("target exists"));
    }

    #[test]
    fn apply_rechecks_preflight_and_skips_a_freshly_dirty_repo() {
        let f = fx();
        let repo = f.root.join("stray");
        make_repo(&repo, Some("git@github.com:me/stray.git"));
        let discovered = scan::scan(&f.cfg).unwrap();
        let p = plan(&f.cfg, &discovered, &PlanOptions { allow_dirty: true }).unwrap();
        assert_eq!(p.moves.len(), 1);

        fs::write(repo.join("dirt.txt"), "x").unwrap();
        let report = apply(&f.cfg, &p, &ApplyOptions::default()).unwrap();
        assert!(report.moved.is_empty());
        assert_eq!(report.skipped.len(), 1);
        assert!(repo.join(".git").exists());
    }

    #[test]
    fn apply_collision_suffix_disambiguates_members() {
        let f = fx();
        make_repo(&f.root.join("x"), Some("git@github.com:me/repo.git"));
        make_repo(&f.root.join("y"), Some("git@github.com:ME/repo.git"));

        let p = plan_default(&f);
        let opts = ApplyOptions {
            on_collision: OnCollision::Suffix,
            ..Default::default()
        };
        let report = apply(&f.cfg, &p, &opts).unwrap();

        assert_eq!(report.moved.len(), 2);
        assert!(report.failed.is_empty());
        let moved_dests: Vec<_> = report.moved.iter().map(|(_, d)| d.clone()).collect();
        assert!(moved_dests.iter().all(|d| d.join(".git").exists()));
    }

    #[test]
    fn apply_collision_fail_records_a_skip() {
        let f = fx();
        make_repo(&f.root.join("x"), Some("git@github.com:me/repo.git"));
        make_repo(&f.root.join("y"), Some("git@github.com:ME/repo.git"));

        let p = plan_default(&f);
        let report = apply(&f.cfg, &p, &ApplyOptions::default()).unwrap();
        assert!(report.moved.is_empty());
        assert_eq!(report.skipped.len(), 1);
    }

    #[test]
    fn plan_display_is_scannable() {
        let f = fx();
        make_repo(
            &f.root.join("github.com/me/right"),
            Some("git@github.com:me/right.git"),
        );
        make_repo(&f.root.join("stray"), Some("git@github.com:me/stray.git"));
        make_repo(&f.root.join("local-only"), None);
        let p = plan_default(&f);
        let text = p.render(&f.cfg);
        assert!(text.contains("move"), "{text}");
        assert!(text.contains("skip"), "{text}");
        assert!(text.contains("github.com/me/stray"), "{text}");
        assert!(text.contains("1 to move"), "{text}");
        assert!(text.contains("1 in place"), "{text}");
    }
}
