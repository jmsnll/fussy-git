//! The sync engine: compare a [`Manifest`](crate::manifest) against what
//! [`scan`](crate::scan) found on disk, produce a categorised [`Plan`], and — on
//! request — [`apply`] it by cloning what is missing.
//!
//! Mirrors [`reconcile`](crate::reconcile)'s split: [`plan`] is pure and touches
//! nothing; [`apply`] clones. Where `reconcile` corrects *layout* drift, `sync`
//! corrects *set* drift — which repositories should be here at all.
//!
//! What `sync` deliberately does **not** do:
//!
//! * Move a manifested repo that is sitting in the wrong place — that is
//!   reported as `misplaced → run reconcile`, never relocated here.
//! * Remove `extra` repositories. Pruning is a later, opt-in addition; v1 only
//!   reports them.

use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use anyhow::Result;

use crate::config;
use crate::get::{self, CloneOutcome, GetOptions};
use crate::identity::Identity;
use crate::index;
use crate::manifest::Manifest;
use crate::ops::{self, Done};
use crate::resolve;
use crate::scan::{self, Class};
use crate::ui;

/// Width of the left column that holds the verdict (`missing`, `extra`, …).
const GUTTER: usize = 11;

/// Per-repo timeout for a clone during `--apply`, matching `bulk`'s network
/// timeout.
const CLONE_TIMEOUT: Duration = Duration::from_secs(300);

// --- plan types ------------------------------------------------------------

/// A manifest entry with no matching repository on disk.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Missing {
    pub identity: Identity,
    /// The exact URL to clone from, when the manifest entry was itself a URL.
    /// `None` means "derive it from the host rule", as `get` would.
    pub url: Option<String>,
}

/// A manifested repository that exists but is not at its canonical path.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Misplaced {
    pub identity: Identity,
    pub from: PathBuf,
}

/// A repository on disk that the manifest does not mention.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Extra {
    pub path: PathBuf,
    pub identity: Identity,
}

/// A manifest entry that could not be resolved to an identity.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Unresolvable {
    pub target: String,
    pub reason: String,
}

/// The full categorised result of planning a sync.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Plan {
    pub missing: Vec<Missing>,
    pub misplaced: Vec<Misplaced>,
    pub extra: Vec<Extra>,
    pub unresolvable: Vec<Unresolvable>,
    /// Count of manifested repositories already sitting at their canonical path.
    pub ok: usize,
}

impl Plan {
    /// True when the tree does not match the manifest.
    pub fn has_drift(&self) -> bool {
        !self.missing.is_empty()
            || !self.misplaced.is_empty()
            || !self.extra.is_empty()
            || !self.unresolvable.is_empty()
    }

    /// One-line summary of the plan's counts, listing only what is non-zero.
    pub fn summary(&self) -> String {
        let mut parts = Vec::new();
        if !self.missing.is_empty() {
            parts.push(format!("{} missing", self.missing.len()));
        }
        if !self.misplaced.is_empty() {
            parts.push(format!("{} misplaced", self.misplaced.len()));
        }
        if !self.extra.is_empty() {
            parts.push(format!("{} extra", self.extra.len()));
        }
        if !self.unresolvable.is_empty() {
            parts.push(plural(self.unresolvable.len(), "error"));
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

    /// A scannable, optionally coloured rendering: a one-line summary, then one
    /// indented entry per finding with the verdict in a fixed left column.
    pub fn render(&self, cfg: &config::Config) -> String {
        let name = |p: &Path| ui::repo_name(cfg, p);
        let mut out = String::new();

        let _ = writeln!(out, "{}  {}", ui::header("sync"), ui::dim(&self.summary()));

        if !self.has_drift() {
            return out;
        }
        let _ = writeln!(out);

        for m in &self.missing {
            let _ = writeln!(out, "  {}{}", verb("missing", ui::accent), m.identity);
        }
        for m in &self.misplaced {
            let _ = writeln!(
                out,
                "  {}{}   {}",
                verb("misplaced", ui::warn),
                name(&m.from),
                ui::dim("run `fussy-git reconcile`")
            );
        }
        for e in &self.extra {
            let _ = writeln!(
                out,
                "  {}{}   {}",
                verb("extra", ui::warn),
                name(&e.path),
                ui::dim("not in manifest")
            );
        }
        for u in &self.unresolvable {
            let _ = writeln!(
                out,
                "  {}{}   {}",
                verb("error", ui::danger),
                u.target,
                ui::dim(&u.reason)
            );
        }

        out
    }
}

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

// --- planning -------------------------------------------------------------

/// Compare `manifest` against `discovered` and categorise every difference.
pub fn plan(
    cfg: &config::Config,
    manifest: &Manifest,
    discovered: &[scan::Discovered],
) -> Result<Plan> {
    let mut plan = Plan::default();

    // On-disk repositories, keyed by canonical identity (case-folded so a
    // case-insensitive filesystem does not read as drift).
    let mut disk: BTreeMap<String, Vec<&scan::Discovered>> = BTreeMap::new();
    for d in discovered {
        if matches!(d.class, Class::LinkedWorktree) {
            continue;
        }
        let Some(id) = d.identity.as_ref() else {
            // No remote / unparseable remote: `reconcile` and `doctor` own these.
            continue;
        };
        disk.entry(canonical_key(cfg, id)).or_default().push(d);
    }

    // Desired repositories from the manifest. De-duplicated by canonical
    // identity; a later entry wins.
    let mut desired: BTreeMap<String, (Identity, Option<String>)> = BTreeMap::new();
    for entry in &manifest.entries {
        match resolve::identity_from_target(cfg, &entry.target) {
            Ok(id) => {
                let url = looks_like_url(&entry.target).then(|| entry.target.trim().to_string());
                desired.insert(canonical_key(cfg, &id), (id, url));
            }
            Err(e) => plan.unresolvable.push(Unresolvable {
                target: entry.target.clone(),
                reason: e.to_string(),
            }),
        }
    }

    for (key, (id, url)) in &desired {
        match disk.get(key) {
            None => plan.missing.push(Missing {
                identity: id.clone(),
                url: url.clone(),
            }),
            Some(hits) if hits.iter().any(|d| matches!(d.class, Class::Ok)) => plan.ok += 1,
            Some(hits) => plan.misplaced.push(Misplaced {
                identity: id.clone(),
                from: hits[0].path.clone(),
            }),
        }
    }

    for (key, hits) in &disk {
        if desired.contains_key(key) {
            continue;
        }
        for d in hits {
            plan.extra.push(Extra {
                path: d.path.clone(),
                identity: d.identity.clone().expect("only Some identities are keyed"),
            });
        }
    }

    plan.missing.sort_by_key(|m| m.identity.to_string());
    plan.misplaced.sort_by_key(|m| m.identity.to_string());
    plan.extra.sort_by(|a, b| a.path.cmp(&b.path));
    plan.unresolvable.sort_by(|a, b| a.target.cmp(&b.target));

    Ok(plan)
}

/// The case-folded canonical path a repository with this identity should occupy,
/// relative to a root. Reuses the same template render `scan` uses for its
/// `Misplaced` verdict, so the two agree.
fn canonical_key(cfg: &config::Config, id: &Identity) -> String {
    cfg.template_for(&id.host).render(id, None).to_lowercase()
}

/// Distinguishes a URL or scp-like target from a bare `owner/repo` shorthand.
fn looks_like_url(s: &str) -> bool {
    let s = s.trim();
    if s.contains("://") {
        return true;
    }
    match (s.find(':'), s.find('/')) {
        (Some(colon), Some(slash)) => colon < slash,
        (Some(_), None) => true,
        _ => false,
    }
}

// --- applying ------------------------------------------------------------

/// Options for [`apply`].
#[derive(Debug, Clone)]
pub struct ApplyOptions {
    /// Maximum number of concurrent clones.
    pub jobs: usize,
}

/// One unit of clone work handed to the worker pool.
#[derive(Clone)]
struct CloneJob {
    identity: Identity,
    url: Option<String>,
}

/// What [`apply`] actually did.
#[derive(Debug, Clone, Default)]
pub struct ApplyReport {
    pub cloned: Vec<(Identity, PathBuf)>,
    /// Entries that were already present when the clone ran (a race, or the tree
    /// changed between plan and apply).
    pub already_present: Vec<(Identity, PathBuf)>,
    pub failed: Vec<(Identity, String)>,
    /// Non-fatal notes: `post_get` hook failures.
    pub warnings: Vec<String>,
}

impl ApplyReport {
    /// A scannable rendering mirroring [`Plan::render`]'s layout.
    pub fn render(&self, cfg: &config::Config) -> String {
        let name = |p: &Path| ui::repo_name(cfg, p);
        let mut out = String::new();

        let mut summary = vec![format!("{} cloned", self.cloned.len())];
        if !self.already_present.is_empty() {
            summary.push(format!("{} already present", self.already_present.len()));
        }
        if !self.failed.is_empty() {
            summary.push(ui::danger(&format!("{} failed", self.failed.len())));
        }
        let _ = writeln!(out, "{}  {}", ui::header("sync"), summary.join(", "));

        if self.cloned.is_empty() && self.failed.is_empty() {
            return out;
        }
        let _ = writeln!(out);

        for (_, path) in &self.cloned {
            let _ = writeln!(out, "  {}{}", verb("cloned", ui::ok), name(path));
        }
        for (id, why) in &self.failed {
            let _ = writeln!(
                out,
                "  {}{}   {}",
                verb("failed", ui::danger),
                id,
                ui::dim(why)
            );
        }

        out
    }
}

/// Clone every `Missing` entry in `plan` into its canonical path, in parallel.
/// A failed clone does not stop the others.
pub fn apply(cfg: &config::Config, plan: &Plan, opts: &ApplyOptions) -> Result<ApplyReport> {
    let jobs: Vec<CloneJob> = plan
        .missing
        .iter()
        .map(|m| CloneJob {
            identity: m.identity.clone(),
            url: m.url.clone(),
        })
        .collect();

    // The worker closure must be `'static`; hand it an owned config.
    let cfg_owned = Arc::new(cfg.clone());
    let outcomes = ops::for_each(
        jobs,
        opts.jobs.max(1),
        CLONE_TIMEOUT,
        move |job: &CloneJob| {
            get::clone_into(
                &cfg_owned,
                &job.identity,
                job.url.as_deref(),
                &GetOptions::default(),
            )
        },
    );

    let mut report = ApplyReport::default();
    let mut any_cloned = false;
    for Done { item, result } in outcomes {
        match result {
            Ok(CloneOutcome::Cloned(path)) => {
                any_cloned = true;
                report.warnings.extend(
                    get::post_get_hooks(cfg, &path)
                        .into_iter()
                        .map(|w| format!("{}: {w}", item.identity)),
                );
                report.cloned.push((item.identity, path));
            }
            Ok(CloneOutcome::AlreadyPresent(path)) => {
                report.already_present.push((item.identity, path));
            }
            Err(e) => report.failed.push((item.identity, e)),
        }
    }

    if any_cloned {
        // A stale index would misreport the tree until the next scan.
        if let Err(e) = index::refresh(cfg) {
            report
                .warnings
                .push(format!("could not refresh the index: {e:#}"));
        }
    }

    Ok(report)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::manifest::Entry;
    use crate::testutil::{self, make_repo};
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

    fn manifest(targets: &[&str]) -> Manifest {
        Manifest {
            entries: targets
                .iter()
                .map(|t| Entry {
                    target: t.to_string(),
                })
                .collect(),
            source: None,
        }
    }

    fn plan_for(f: &Fx, m: &Manifest) -> Plan {
        let discovered = scan::scan(&f.cfg).unwrap();
        plan(&f.cfg, m, &discovered).unwrap()
    }

    #[test]
    fn classifies_ok_missing_and_extra() {
        let f = fx();
        make_repo(
            &f.root.join("github.com/jmsnll/here"),
            Some("git@github.com:jmsnll/here.git"),
        );
        make_repo(
            &f.root.join("github.com/jmsnll/stray"),
            Some("git@github.com:jmsnll/stray.git"),
        );

        let p = plan_for(&f, &manifest(&["jmsnll/here", "rust-lang/rust"]));

        assert_eq!(p.ok, 1);
        assert_eq!(p.missing.len(), 1);
        assert_eq!(
            p.missing[0].identity.to_string(),
            "github.com/rust-lang/rust"
        );
        assert_eq!(p.extra.len(), 1);
        assert_eq!(p.extra[0].path, f.root.join("github.com/jmsnll/stray"));
        assert!(p.has_drift());
    }

    #[test]
    fn a_manifested_repo_in_the_wrong_place_is_misplaced_not_missing() {
        let f = fx();
        make_repo(
            &f.root.join("wrong-place"),
            Some("git@github.com:jmsnll/thing.git"),
        );

        let p = plan_for(&f, &manifest(&["jmsnll/thing"]));
        assert!(p.missing.is_empty());
        assert_eq!(p.misplaced.len(), 1);
        assert_eq!(p.misplaced[0].from, f.root.join("wrong-place"));
        assert!(p.has_drift());
    }

    #[test]
    fn a_matching_tree_has_no_drift() {
        let f = fx();
        make_repo(
            &f.root.join("github.com/jmsnll/here"),
            Some("git@github.com:jmsnll/here.git"),
        );
        let p = plan_for(&f, &manifest(&["jmsnll/here"]));
        assert!(!p.has_drift());
        assert_eq!(p.ok, 1);
        assert!(p.render(&f.cfg).contains("1 in place"));
    }

    #[test]
    fn an_unresolvable_entry_is_reported_and_counts_as_drift() {
        let f = fx();
        let p = plan_for(&f, &manifest(&["http://"]));
        assert_eq!(p.unresolvable.len(), 1);
        assert_eq!(p.unresolvable[0].target, "http://");
        assert!(p.has_drift());
    }

    #[test]
    fn duplicate_manifest_entries_are_de_duplicated() {
        let f = fx();
        let p = plan_for(
            &f,
            &manifest(&[
                "jmsnll/thing",
                "github.com/jmsnll/thing",
                "https://github.com/jmsnll/thing.git",
            ]),
        );
        assert_eq!(p.missing.len(), 1);
    }

    #[test]
    fn unparseable_remote_on_disk_is_not_reported_as_extra() {
        let f = fx();
        make_repo(&f.root.join("weird"), Some("not-a-url"));
        let p = plan_for(&f, &manifest(&[]));
        assert!(p.extra.is_empty());
        assert!(!p.has_drift());
    }

    #[test]
    fn apply_clones_the_missing_repositories() {
        let f = fx();

        // The bare upstream lives under a plainly-named directory: its path
        // becomes an on-disk directory tree under the root once cloned, and
        // `scan` skips any component that starts with a dot.
        let remote_dir = tempfile::Builder::new()
            .prefix("fussy-git-remote-")
            .tempdir()
            .unwrap();
        let bare = remote_dir.path().join("remotes/widget.git");
        fs::create_dir_all(&bare).unwrap();
        testutil::git(&bare, &["init", "-q", "--bare", "-b", "main"]);
        let seed = remote_dir.path().join("seed");
        make_repo(&seed, None);
        let url = format!("file://example.com{}", bare.display());
        testutil::git(&seed, &["remote", "add", "origin", &url]);
        testutil::git(&seed, &["push", "-q", "origin", "main"]);

        let m = manifest(&[&url]);
        let p = plan_for(&f, &m);
        assert_eq!(p.missing.len(), 1);

        let report = apply(&f.cfg, &p, &ApplyOptions { jobs: 2 }).unwrap();
        assert_eq!(report.cloned.len(), 1, "{:?}", report.failed);
        assert!(report.failed.is_empty());

        let cloned = &report.cloned[0].1;
        assert!(cloned.starts_with(f.root.join("example.com")));
        assert!(crate::git::is_repo_root(cloned));

        // Re-planning now sees it in place.
        let p2 = plan_for(&f, &m);
        assert!(!p2.has_drift(), "{}", p2.render(&f.cfg));
    }

    #[test]
    fn apply_reports_a_failed_clone_without_stopping_the_batch() {
        let f = fx();
        let report = apply(
            &f.cfg,
            &Plan {
                missing: vec![Missing {
                    identity: Identity::parse("https://example.invalid/no/such.git").unwrap(),
                    url: Some("https://example.invalid/no/such.git".to_string()),
                }],
                ..Default::default()
            },
            &ApplyOptions { jobs: 1 },
        )
        .unwrap();
        assert_eq!(report.cloned.len(), 0);
        assert_eq!(report.failed.len(), 1);
        assert!(report.render(&f.cfg).contains("failed"));
    }
}
