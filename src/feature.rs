//! `fussy-git feature` — one folder per cross-repository change.
//!
//! A feature that spans several repositories (an app, its puppet code, the
//! terraform that deploys it, the terraform modules that uses) gets one
//! folder, with a linked worktree of each repository on the same branch:
//!
//! ```text
//! <features_root>/sso/
//!   .fussy-feature.toml   manifest: members, branch, bases, dependencies
//!   CLAUDE.md             generated from the manifest; facts only
//!   NOTES.md              yours; never rewritten
//!   api/  puppet/  terraform/  tf-modules/   worktrees, branch `sso`
//! ```
//!
//! That folder is the whole feature's context for a person or an agent, and
//! the main clones stay untouched. It is not isolation: every worktree still
//! writes commits into its main clone's `.git`.
//!
//! The manifest records intent (which repositories, which branch, what each
//! depends on); git is the truth for state (what is checked out, dirty,
//! pushed). [`new`] checks every repository before creating anything, and is
//! convergent: running it again resumes or adds members. [`done`] refuses
//! while any member has uncommitted work, or unpushed commits unless forced,
//! and keeps every branch.

use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use anyhow::{anyhow, bail, Context, Result};
use serde::{Deserialize, Serialize};

use crate::config::Config;
use crate::query::real;
use crate::{get, git, index, ops, resolve, sets};

/// The manifest file in every feature folder.
pub const MANIFEST: &str = ".fussy-feature.toml";
const MANIFEST_VERSION: u32 = 1;
/// Regenerated from the manifest by `new`.
const GENERATED: &str = "CLAUDE.md";
/// Created once, never rewritten.
const NOTES: &str = "NOTES.md";
/// Finished features are moved here.
const ARCHIVE: &str = ".archive";
const FETCH_TIMEOUT: Duration = Duration::from_secs(300);

// --- manifest ---------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Manifest {
    pub version: u32,
    pub name: String,
    pub branch: String,
    pub created_unix: u64,
    #[serde(default)]
    pub members: Vec<Member>,
}

/// One repository in a feature.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Member {
    /// `host/owner/repo`.
    pub identity: String,
    /// The folder inside the feature.
    pub dir: String,
    /// The main clone the worktree belongs to.
    pub repo_path: PathBuf,
    /// The revision a new branch was started from.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub base: Option<String>,
    /// Folders (`dir`s) of members this one consumes; they come first when
    /// pushing and opening pull requests.
    #[serde(default)]
    pub depends: Vec<String>,
}

/// The folder for feature `name`.
pub fn path(cfg: &Config, name: &str) -> PathBuf {
    cfg.features_root.join(name)
}

fn load(cfg: &Config, name: &str) -> Result<Manifest> {
    let file = path(cfg, name).join(MANIFEST);
    let text = std::fs::read_to_string(&file).map_err(|e| {
        if e.kind() == std::io::ErrorKind::NotFound {
            anyhow!("no feature named {name:?} (see `fussy-git feature list`)")
        } else {
            anyhow!("reading {}: {e}", file.display())
        }
    })?;
    let m: Manifest =
        toml::from_str(&text).with_context(|| format!("parsing {}", file.display()))?;
    if m.version != MANIFEST_VERSION {
        bail!(
            "{} is manifest version {}, but this fussy-git reads version {MANIFEST_VERSION}",
            file.display(),
            m.version
        );
    }
    Ok(m)
}

fn save(cfg: &Config, m: &Manifest) -> Result<()> {
    let dir = path(cfg, &m.name);
    std::fs::create_dir_all(&dir).with_context(|| format!("creating {}", dir.display()))?;
    let text = format!(
        "# fussy-git feature manifest. Edit `depends` freely; run `fussy-git feature new {}` to add repositories.\n{}",
        m.name,
        toml::to_string(m)?
    );
    std::fs::write(dir.join(MANIFEST), text)?;
    Ok(())
}

/// The feature containing the current directory, if any.
pub fn current(cfg: &Config) -> Option<String> {
    let cwd = real(&std::env::current_dir().ok()?);
    let rel = cwd.strip_prefix(real(&cfg.features_root)).ok()?;
    let name = rel.components().next()?.as_os_str().to_str()?.to_string();
    path(cfg, &name).join(MANIFEST).is_file().then_some(name)
}

/// `name`, or the feature containing the current directory.
pub fn resolve_name(cfg: &Config, name: Option<&str>) -> Result<String> {
    match name {
        Some(n) => Ok(n.to_string()),
        None => current(cfg).ok_or_else(|| {
            anyhow!("not inside a feature folder; name the feature (see `fussy-git feature list`)")
        }),
    }
}

/// Feature names become folder names and default branch names.
fn validate_name(name: &str) -> Result<()> {
    let ok = !name.is_empty()
        && name.len() <= 100
        && name
            .chars()
            .next()
            .is_some_and(|c| c.is_ascii_lowercase() || c.is_ascii_digit())
        && name
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || "_-.".contains(c));
    if !ok {
        bail!(
            "invalid feature name {name:?}: use lowercase letters, digits, `_`, `-` and `.`, \
             starting with a letter or digit (e.g. `plat_123_sso`)"
        );
    }
    Ok(())
}

fn validate_branch(branch: &str) -> Result<()> {
    git::run(None, ["check-ref-format", "--branch", branch])
        .map(|_| ())
        .map_err(|_| anyhow!("{branch:?} is not a valid git branch name"))
}

/// Puppet r10k environments (one per control-repo branch) accept only
/// `[a-z0-9_]`; other names are rewritten or rejected.
fn r10k_safe(branch: &str) -> bool {
    !branch.is_empty()
        && branch
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_')
}

/// Order members so that dependencies come first. Unknown dependencies are
/// ignored; a cycle falls back to manifest order for the members involved.
fn dependency_order(members: &[Member]) -> Vec<usize> {
    let mut done: Vec<usize> = Vec::new();
    let mut remaining: Vec<usize> = (0..members.len()).collect();
    while !remaining.is_empty() {
        let placed: Vec<&str> = done.iter().map(|&i| members[i].dir.as_str()).collect();
        let ready = remaining.iter().position(|&i| {
            members[i]
                .depends
                .iter()
                .all(|d| placed.contains(&d.as_str()) || !members.iter().any(|m| &m.dir == d))
        });
        let next = ready.unwrap_or(0);
        done.push(remaining.remove(next));
    }
    done
}

// --- new ---------------------------------------------------------------------

/// Options for [`new`].
#[derive(Debug, Clone, Default)]
pub struct NewOptions {
    /// The branch, when it should differ from the feature name.
    pub branch: Option<String>,
    /// Start new branches from this revision instead of each repository's
    /// default branch.
    pub base: Option<String>,
    /// Check out a local branch that already exists, rather than refusing.
    pub reuse: bool,
    /// `(dir, [dirs it depends on])`, merged into the manifest.
    pub depends: Vec<(String, Vec<String>)>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum MemberState {
    /// The worktree was made by this run.
    Created,
    /// The worktree was already there.
    Existing,
    /// Nothing was made for this member; see `error`.
    Failed,
}

#[derive(Debug, Clone, Serialize)]
pub struct MemberError {
    /// Stable: `clone_failed`, `path_exists`, `branch_checked_out_elsewhere`,
    /// `branch_exists`, `bad_base`, `worktree_failed`, `not_planned`.
    pub code: &'static str,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub other_path: Option<PathBuf>,
}

#[derive(Debug, Clone, Serialize)]
pub struct MemberReport {
    pub identity: String,
    pub dir: String,
    pub path: PathBuf,
    pub repo_path: Option<PathBuf>,
    pub state: MemberState,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub base: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<MemberError>,
}

#[derive(Debug, Clone, Serialize)]
pub struct NewReport {
    pub name: String,
    pub path: PathBuf,
    pub branch: String,
    pub members: Vec<MemberReport>,
    pub warnings: Vec<String>,
}

impl NewReport {
    pub fn failed(&self) -> usize {
        self.members
            .iter()
            .filter(|m| m.state == MemberState::Failed)
            .count()
    }
}

/// How a member's worktree will be made.
enum Plan {
    /// Already a worktree on the feature branch.
    Existing,
    /// Check out the existing local branch.
    Local,
    /// Create the branch tracking `origin/<branch>`.
    Track,
    /// Create the branch from `base`, with no upstream.
    New(String),
}

/// Create feature `name` with a worktree of each repository in `targets`
/// (`get` targets or `@set`s), or add them to the feature if it exists.
pub fn new(cfg: &Config, name: &str, targets: &[String], opts: &NewOptions) -> Result<NewReport> {
    validate_name(name)?;
    let folder = path(cfg, name);
    let existing = folder
        .join(MANIFEST)
        .is_file()
        .then(|| load(cfg, name))
        .transpose()?;
    let branch = match (&existing, &opts.branch) {
        (Some(m), Some(b)) if &m.branch != b => bail!(
            "feature {name:?} already uses branch {:?}; it cannot change",
            m.branch
        ),
        (Some(m), _) => m.branch.clone(),
        (None, Some(b)) => b.clone(),
        (None, None) => name.to_string(),
    };
    validate_branch(&branch)?;
    if existing.is_none() && targets.is_empty() {
        bail!(
            "name the repositories for the feature: `fussy-git feature new {name} <@set | repo…>`"
        );
    }

    let mut manifest = existing.unwrap_or_else(|| Manifest {
        version: MANIFEST_VERSION,
        name: name.to_string(),
        branch: branch.clone(),
        created_unix: SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0),
        members: Vec::new(),
    });
    let mut warnings = Vec::new();
    let mut failures: Vec<MemberReport> = Vec::new();

    // Expand sets, then find (or clone) each repository.
    let mut expanded: Vec<String> = Vec::new();
    for t in targets {
        match sets::set_name(t) {
            Some(set) => expanded.extend(sets::targets(cfg, set)?.iter().cloned()),
            None => expanded.push(t.clone()),
        }
    }
    let index = index::load_or_build(cfg)?;
    for target in &expanded {
        let id = resolve::identity_from_target(cfg, target)
            .with_context(|| format!("resolving {target:?}"))?;
        let slug = id.to_string();
        if manifest.members.iter().any(|m| m.identity == slug) {
            continue;
        }
        let repo_path = match index
            .entries()
            .iter()
            .find(|e| e.slug().as_deref() == Some(&slug))
        {
            Some(e) => e.path.clone(),
            None => match get::run(cfg, target, &get::GetOptions::default()) {
                Ok(got) => got.path,
                Err(e) => {
                    failures.push(MemberReport {
                        identity: slug,
                        dir: id.repo.clone(),
                        path: PathBuf::new(),
                        repo_path: None,
                        state: MemberState::Failed,
                        base: None,
                        error: Some(MemberError {
                            code: "clone_failed",
                            message: format!("{e:#}"),
                            other_path: None,
                        }),
                    });
                    continue;
                }
            },
        };
        let dir = unique_dir(&manifest.members, &id.repo, &id.owner_path());
        manifest.members.push(Member {
            identity: slug,
            dir,
            repo_path,
            base: None,
            depends: Vec::new(),
        });
    }

    for (dir, deps) in &opts.depends {
        let m = manifest
            .members
            .iter_mut()
            .find(|m| &m.dir == dir)
            .ok_or_else(|| anyhow!("--depends: no member folder {dir:?} in {name:?}"))?;
        for d in deps {
            if !m.depends.contains(d) {
                m.depends.push(d.clone());
            }
        }
    }
    for m in &manifest.members {
        for d in &m.depends {
            if !manifest.members.iter().any(|o| &o.dir == d) {
                warnings.push(format!("{}: depends on unknown member {d:?}", m.dir));
            }
        }
    }
    if !r10k_safe(&branch)
        && manifest
            .members
            .iter()
            .any(|m| m.identity.contains("puppet") || m.identity.contains("control"))
    {
        warnings.push(format!(
            "branch {branch:?} is not a valid Puppet r10k environment name (only a-z, 0-9 and _); \
             r10k will rename or reject it"
        ));
    }

    // Fetch, so new branches start from the remote's current default branch.
    let repos: Vec<PathBuf> = manifest
        .members
        .iter()
        .map(|m| m.repo_path.clone())
        .collect();
    for o in ops::for_each_repo(repos, cfg.jobs, FETCH_TIMEOUT, |repo| {
        git::run(Some(repo), ["fetch", "origin", "--quiet"]).map(|_| ())
    }) {
        if let Err(e) = o.result {
            warnings.push(format!(
                "could not fetch {}: {}",
                o.repo.display(),
                e.lines().next().unwrap_or_default()
            ));
        }
    }

    // Check every member before creating anything.
    let mut plans: Vec<(usize, Plan)> = Vec::new();
    for (i, m) in manifest.members.iter().enumerate() {
        match plan_member(&folder, m, &branch, opts) {
            Ok(p) => plans.push((i, p)),
            Err(err) => failures.push(MemberReport {
                identity: m.identity.clone(),
                dir: m.dir.clone(),
                path: folder.join(&m.dir),
                repo_path: Some(m.repo_path.clone()),
                state: MemberState::Failed,
                base: None,
                error: Some(err),
            }),
        }
    }
    if !failures.is_empty() {
        // Report the planned members as not made, so the caller sees every
        // repository and can fix the conflicts before anything changes.
        let mut members = failures;
        for (i, _) in &plans {
            let m = &manifest.members[*i];
            members.push(MemberReport {
                identity: m.identity.clone(),
                dir: m.dir.clone(),
                path: folder.join(&m.dir),
                repo_path: Some(m.repo_path.clone()),
                state: MemberState::Failed,
                base: None,
                error: Some(MemberError {
                    code: "not_planned",
                    message: "skipped: another member has a conflict; nothing was created"
                        .to_string(),
                    other_path: None,
                }),
            });
        }
        return Ok(NewReport {
            name: name.to_string(),
            path: folder,
            branch,
            members,
            warnings,
        });
    }

    // Record intent first, so an interrupted run can be resumed.
    for (i, p) in &plans {
        if let Plan::New(base) = p {
            manifest.members[*i].base = Some(base.clone());
        }
    }
    save(cfg, &manifest)?;

    let mut members = Vec::new();
    for (i, p) in plans {
        let m = &manifest.members[i];
        let wt = folder.join(&m.dir);
        let wt_arg = wt.to_string_lossy().into_owned();
        let made = match &p {
            Plan::Existing => Ok(MemberState::Existing),
            Plan::Local => git::run(Some(&m.repo_path), ["worktree", "add", &wt_arg, &branch])
                .map(|_| MemberState::Created),
            Plan::Track => {
                let upstream = format!("origin/{branch}");
                git::run(
                    Some(&m.repo_path),
                    [
                        "worktree", "add", "--track", "-b", &branch, &wt_arg, &upstream,
                    ],
                )
                .map(|_| MemberState::Created)
            }
            Plan::New(base) => git::run(
                Some(&m.repo_path),
                [
                    "worktree",
                    "add",
                    "--no-track",
                    "-b",
                    &branch,
                    &wt_arg,
                    base,
                ],
            )
            .map(|_| MemberState::Created),
        };
        let (state, error) = match made {
            Ok(state) => {
                let reason = format!("fussy-git feature {name}");
                // Already locked on a resumed run; that is fine.
                let _ = git::run(
                    Some(&m.repo_path),
                    ["worktree", "lock", "--reason", &reason, &wt_arg],
                );
                (state, None)
            }
            Err(e) => (
                MemberState::Failed,
                Some(MemberError {
                    code: "worktree_failed",
                    message: format!("{e:#}"),
                    other_path: None,
                }),
            ),
        };
        members.push(MemberReport {
            identity: m.identity.clone(),
            dir: m.dir.clone(),
            path: wt,
            repo_path: Some(m.repo_path.clone()),
            state,
            base: m.base.clone(),
            error,
        });
    }

    std::fs::write(folder.join(GENERATED), generated_claude_md(&manifest))?;
    let notes = folder.join(NOTES);
    if !notes.exists() {
        std::fs::write(
            &notes,
            format!("# {name}\n\nNotes for this feature. fussy-git never rewrites this file.\n"),
        )?;
    }

    Ok(NewReport {
        name: name.to_string(),
        path: folder,
        branch,
        members,
        warnings,
    })
}

/// A folder name for a new member: the repository name, or `owner-repo`
/// when that is taken (compared case-insensitively, for macOS).
fn unique_dir(members: &[Member], repo: &str, owner: &str) -> String {
    let taken = |d: &str| members.iter().any(|m| m.dir.eq_ignore_ascii_case(d));
    if !taken(repo) {
        return repo.to_string();
    }
    let qualified = format!("{}-{repo}", owner.replace('/', "-"));
    let mut candidate = qualified.clone();
    let mut n = 2;
    while taken(&candidate) {
        candidate = format!("{qualified}-{n}");
        n += 1;
    }
    candidate
}

fn plan_member(
    folder: &Path,
    m: &Member,
    branch: &str,
    opts: &NewOptions,
) -> Result<Plan, MemberError> {
    let err = |code, message: String, other_path| MemberError {
        code,
        message,
        other_path,
    };
    let wt = folder.join(&m.dir);
    if wt.exists() {
        let on_branch = git::toplevel(&wt).is_ok_and(|t| real(&t) == real(&wt))
            && git::current_branch(&wt).ok().flatten().as_deref() == Some(branch);
        return if on_branch {
            Ok(Plan::Existing)
        } else {
            Err(err(
                "path_exists",
                format!(
                    "{} exists but is not a worktree on {branch:?}",
                    wt.display()
                ),
                None,
            ))
        };
    }
    let worktrees =
        git::worktrees(&m.repo_path).map_err(|e| err("worktree_failed", format!("{e:#}"), None))?;
    if let Some(w) = worktrees
        .iter()
        .find(|w| w.branch.as_deref() == Some(branch))
    {
        return Err(err(
            "branch_checked_out_elsewhere",
            format!(
                "{branch:?} is already checked out at {}; switch that checkout to another branch first",
                w.path.display()
            ),
            Some(w.path.clone()),
        ));
    }
    if git::ref_exists(&m.repo_path, &format!("refs/heads/{branch}")) {
        // A branch this feature already recorded is its own; anything else
        // is someone's existing work, used only when asked.
        return if opts.reuse || m.base.is_some() {
            Ok(Plan::Local)
        } else {
            Err(err(
                "branch_exists",
                format!("a local branch {branch:?} already exists; pass --reuse to check it out"),
                None,
            ))
        };
    }
    if git::ref_exists(&m.repo_path, &format!("refs/remotes/origin/{branch}")) {
        return Ok(Plan::Track);
    }
    let base = opts
        .base
        .clone()
        .or_else(|| git::default_branch(&m.repo_path))
        .unwrap_or_else(|| "HEAD".to_string());
    if git::run(
        Some(&m.repo_path),
        [
            "rev-parse",
            "--verify",
            "--quiet",
            &format!("{base}^{{commit}}"),
        ],
    )
    .is_err()
    {
        return Err(err(
            "bad_base",
            format!("base {base:?} does not exist in this repository"),
            None,
        ));
    }
    Ok(Plan::New(base))
}

/// The generated CLAUDE.md: facts from the manifest only, never live state,
/// which would go stale.
fn generated_claude_md(m: &Manifest) -> String {
    let order = dependency_order(&m.members);
    let mut s = format!(
        "<!-- Generated by fussy-git from {MANIFEST}; rewritten by `fussy-git feature new`. \
         Put your own notes in {NOTES}. -->\n\
         # Feature: {name}\n\n\
         Every folder below is a git worktree on branch `{branch}` of the repository listed. \
         Commit, push and open a pull request in each repository separately.\n\n\
         | Folder | Repository | Started from | Depends on |\n\
         | --- | --- | --- | --- |\n",
        name = m.name,
        branch = m.branch,
    );
    for &i in &order {
        let mem = &m.members[i];
        s.push_str(&format!(
            "| `{}` | {} | {} | {} |\n",
            mem.dir,
            mem.identity,
            mem.base.as_deref().unwrap_or("existing branch"),
            if mem.depends.is_empty() {
                "".to_string()
            } else {
                mem.depends.join(", ")
            }
        ));
    }
    let ordered: Vec<&str> = order.iter().map(|&i| m.members[i].dir.as_str()).collect();
    s.push_str(&format!(
        "\nPush and open pull requests in this order (dependencies first): {}.\n\
         Link the pull requests to each other in their descriptions.\n\n\
         - Live state of every repository: `fussy-git feature status --json` (run from this folder).\n\
         - Run a command in every repository: `fussy-git feature exec -- <command>`.\n\
         - Each repository's own CLAUDE.md still applies inside its folder.\n\
         - Do not switch branches or create worktrees in these folders, and do not edit the main clones.\n\
         - When the work is merged: `fussy-git feature done {}` (refuses while anything is uncommitted or unpushed).\n",
        ordered.join(", "),
        m.name
    ));
    s
}

// --- list / status -----------------------------------------------------------

#[derive(Debug, Clone, Serialize)]
pub struct Summary {
    pub name: String,
    pub path: PathBuf,
    pub branch: String,
    pub members: Vec<String>,
}

/// Every feature under `features_root`.
pub fn list(cfg: &Config) -> Result<Vec<Summary>> {
    let mut out = Vec::new();
    let Ok(rd) = std::fs::read_dir(&cfg.features_root) else {
        return Ok(out);
    };
    for entry in rd.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        if name.starts_with('.') || !entry.path().join(MANIFEST).is_file() {
            continue;
        }
        let m = load(cfg, &name)?;
        out.push(Summary {
            path: entry.path(),
            branch: m.branch,
            members: m.members.into_iter().map(|mem| mem.dir).collect(),
            name,
        });
    }
    out.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(out)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Health {
    Ok,
    /// The folder is gone (deleted by hand, or creation failed).
    Missing,
    /// The folder exists but has another branch checked out.
    WrongBranch,
}

#[derive(Debug, Clone, Serialize)]
pub struct MemberStatus {
    pub identity: String,
    pub dir: String,
    pub path: PathBuf,
    pub repo_path: PathBuf,
    pub state: Health,
    pub branch: Option<String>,
    pub upstream: Option<String>,
    pub ahead: u32,
    pub behind: u32,
    pub dirty: Option<bool>,
    /// Commits not on any `origin` branch. With no upstream yet, this counts
    /// the whole branch's own work.
    pub unpushed: Option<u32>,
    pub depends: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct Status {
    pub name: String,
    pub path: PathBuf,
    pub branch: String,
    /// Members in dependency order.
    pub members: Vec<MemberStatus>,
}

/// Live state of every member of feature `name`, dependencies first.
pub fn status(cfg: &Config, name: &str) -> Result<Status> {
    let m = load(cfg, name)?;
    let folder = path(cfg, name);
    let members = dependency_order(&m.members)
        .into_iter()
        .map(|i| member_status(&folder, &m.members[i], &m.branch))
        .collect();
    Ok(Status {
        name: m.name,
        path: folder,
        branch: m.branch,
        members,
    })
}

fn member_status(folder: &Path, mem: &Member, branch: &str) -> MemberStatus {
    let wt = folder.join(&mem.dir);
    let mut st = MemberStatus {
        identity: mem.identity.clone(),
        dir: mem.dir.clone(),
        path: wt.clone(),
        repo_path: mem.repo_path.clone(),
        state: Health::Missing,
        branch: None,
        upstream: None,
        ahead: 0,
        behind: 0,
        dirty: None,
        unpushed: None,
        depends: mem.depends.clone(),
    };
    if !wt.join(".git").exists() {
        return st;
    }
    st.branch = git::current_branch(&wt).ok().flatten();
    st.state = if st.branch.as_deref() == Some(branch) {
        Health::Ok
    } else {
        Health::WrongBranch
    };
    st.upstream = git::stdout(
        Some(&wt),
        [
            "rev-parse",
            "--abbrev-ref",
            "--symbolic-full-name",
            "@{upstream}",
        ],
    )
    .ok()
    .filter(|s| !s.is_empty());
    if let Ok(Some((a, b))) = git::ahead_behind(&wt) {
        st.ahead = a;
        st.behind = b;
    }
    st.dirty = git::is_dirty(&wt).ok();
    st.unpushed = git::stdout(
        Some(&wt),
        ["rev-list", "--count", "HEAD", "--not", "--remotes=origin"],
    )
    .ok()
    .and_then(|s| s.parse().ok());
    st
}

/// Worktree paths of the members that exist, dependencies first, for `exec`.
pub fn member_paths(cfg: &Config, name: &str) -> Result<Vec<PathBuf>> {
    Ok(status(cfg, name)?
        .members
        .into_iter()
        .filter(|m| m.state != Health::Missing)
        .map(|m| m.path)
        .collect())
}

// --- done --------------------------------------------------------------------

#[derive(Debug, Clone, Serialize)]
pub struct Refusal {
    pub dir: String,
    pub path: PathBuf,
    pub reasons: Vec<String>,
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct DoneReport {
    pub name: String,
    /// Members whose worktree was removed.
    pub removed: Vec<PathBuf>,
    /// Members that would lose work; when non-empty nothing was changed.
    pub refused: Vec<Refusal>,
    /// Members git could not remove.
    pub failed: Vec<Refusal>,
    /// Where the folder (manifest, CLAUDE.md, NOTES.md) was moved.
    pub archived_to: Option<PathBuf>,
}

/// Finish feature `name`: remove every member's worktree (keeping the
/// branches) and archive the folder. Refuses, changing nothing, while any
/// member has uncommitted changes, or unpushed commits unless `force`.
pub fn done(cfg: &Config, name: &str, force: bool) -> Result<DoneReport> {
    let st = status(cfg, name)?;
    let mut report = DoneReport {
        name: name.to_string(),
        ..Default::default()
    };
    for m in &st.members {
        let mut reasons = Vec::new();
        if m.dirty == Some(true) {
            reasons.push(
                "uncommitted changes (commit or stash them; --force does not override this)"
                    .to_string(),
            );
        }
        if !force && m.unpushed.unwrap_or(0) > 0 {
            reasons.push(format!(
                "{} unpushed commit(s) (push them, or pass --force if they were squash-merged)",
                m.unpushed.unwrap_or(0)
            ));
        }
        if m.state == Health::WrongBranch {
            reasons.push(format!(
                "has {} checked out instead of {}",
                m.branch.as_deref().unwrap_or("a detached HEAD"),
                st.branch
            ));
        }
        if !reasons.is_empty() {
            report.refused.push(Refusal {
                dir: m.dir.clone(),
                path: m.path.clone(),
                reasons,
            });
        }
    }
    if !report.refused.is_empty() {
        return Ok(report);
    }

    for m in &st.members {
        let wt_arg = m.path.to_string_lossy().into_owned();
        let result = if m.state == Health::Missing {
            git::run(Some(&m.repo_path), ["worktree", "prune"]).map(|_| ())
        } else {
            let _ = git::run(Some(&m.repo_path), ["worktree", "unlock", &wt_arg]);
            git::run(Some(&m.repo_path), ["worktree", "remove", &wt_arg]).map(|_| ())
        };
        match result {
            Ok(()) => report.removed.push(m.path.clone()),
            Err(e) => report.failed.push(Refusal {
                dir: m.dir.clone(),
                path: m.path.clone(),
                reasons: vec![format!("{e:#}")],
            }),
        }
    }
    if report.failed.is_empty() {
        let archive = cfg.features_root.join(ARCHIVE);
        std::fs::create_dir_all(&archive)?;
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        let dest = archive.join(format!("{name}-{stamp}"));
        std::fs::rename(path(cfg, name), &dest)
            .with_context(|| format!("archiving {}", path(cfg, name).display()))?;
        report.archived_to = Some(dest);
    }
    Ok(report)
}

/// For `worktree list`: the feature a worktree path belongs to, if any.
pub fn feature_of(cfg: &Config, worktree: &Path) -> Option<String> {
    let rel = real(worktree)
        .strip_prefix(real(&cfg.features_root))
        .ok()?
        .to_path_buf();
    let mut parts = rel.components();
    let name = parts.next()?.as_os_str().to_str()?.to_string();
    parts.next()?;
    (!name.starts_with('.')).then_some(name)
}

/// `"dir=dep1,dep2"` from the command line.
pub fn parse_depends(spec: &str) -> Result<(String, Vec<String>)> {
    let (dir, deps) = spec
        .split_once('=')
        .ok_or_else(|| anyhow!("--depends expects `folder=dep1,dep2`, got {spec:?}"))?;
    let deps: Vec<String> = deps
        .split(',')
        .map(str::trim)
        .filter(|d| !d.is_empty())
        .map(str::to_string)
        .collect();
    if dir.trim().is_empty() || deps.is_empty() {
        bail!("--depends expects `folder=dep1,dep2`, got {spec:?}");
    }
    Ok((dir.trim().to_string(), deps))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::{self, make_repo};

    /// A bare remote seeded with one commit, and its `file://example.com…`
    /// URL, which git can clone and fussy-git can turn into an identity.
    fn remote(tmp: &Path, name: &str) -> String {
        let bare = tmp.join(format!("remotes/{name}.git"));
        std::fs::create_dir_all(&bare).unwrap();
        testutil::git(&bare, &["init", "-q", "--bare", "-b", "main"]);
        let seed = tmp.join(format!("seed-{name}"));
        make_repo(&seed, None);
        let url = format!("file://example.com{}", bare.display());
        testutil::git(&seed, &["remote", "add", "origin", &url]);
        testutil::git(&seed, &["push", "-q", "origin", "main"]);
        url
    }

    fn cfg(tmp: &Path) -> Config {
        let root = real(tmp).join("git");
        std::fs::create_dir_all(&root).unwrap();
        let mut c = Config::default();
        c.root = root.clone();
        c.roots = vec![root.clone()];
        c.features_root = root.join(".features");
        c
    }

    fn commit(dir: &Path, file: &str) {
        std::fs::write(dir.join(file), file).unwrap();
        testutil::git(dir, &["add", "."]);
        testutil::git(dir, &["commit", "-qm", file, "--no-verify"]);
    }

    #[test]
    fn new_status_and_done_round_trip() {
        let tmp = tempfile::tempdir().unwrap();
        let tmp = real(tmp.path());
        let cfg = cfg(&tmp);
        let api = remote(&tmp, "api");
        let mods = remote(&tmp, "tf-modules");

        let opts = NewOptions {
            depends: vec![],
            ..Default::default()
        };
        let report = new(&cfg, "sso", &[api.clone(), mods.clone()], &opts).unwrap();
        assert_eq!(report.failed(), 0, "{:?}", report.members);
        let folder = path(&cfg, "sso");
        assert!(folder.join("api/README.md").is_file());
        assert!(folder.join("tf-modules/README.md").is_file());
        assert!(folder.join(GENERATED).is_file() && folder.join(NOTES).is_file());
        assert_eq!(report.members[0].base.as_deref(), Some("origin/main"));

        // Rerunning is convergent; --depends is merged into the manifest.
        let again = new(
            &cfg,
            "sso",
            &[],
            &NewOptions {
                depends: vec![("api".into(), vec!["tf-modules".into()])],
                ..Default::default()
            },
        )
        .unwrap();
        assert!(again
            .members
            .iter()
            .all(|m| m.state == MemberState::Existing));

        let st = status(&cfg, "sso").unwrap();
        let order: Vec<&str> = st.members.iter().map(|m| m.dir.as_str()).collect();
        assert_eq!(order, vec!["tf-modules", "api"], "dependencies first");
        assert!(st
            .members
            .iter()
            .all(|m| m.state == Health::Ok && m.unpushed == Some(0)));
        let md = std::fs::read_to_string(folder.join(GENERATED)).unwrap();
        assert!(md.contains("dependencies first): tf-modules, api"), "{md}");

        // Unpushed work blocks `done`; --force allows it; dirt never does.
        commit(&folder.join("api"), "change.txt");
        let refused = done(&cfg, "sso", false).unwrap();
        assert_eq!(refused.refused.len(), 1);
        assert!(folder.join("api").exists(), "nothing removed on refusal");
        std::fs::write(folder.join("tf-modules/wip.txt"), "wip").unwrap();
        let dirty = done(&cfg, "sso", true).unwrap();
        assert_eq!(dirty.refused.len(), 1);
        assert!(dirty.refused[0].reasons[0].contains("uncommitted"));
        std::fs::remove_file(folder.join("tf-modules/wip.txt")).unwrap();

        let finished = done(&cfg, "sso", true).unwrap();
        assert!(finished.refused.is_empty() && finished.failed.is_empty());
        assert!(!folder.exists());
        let archived = finished.archived_to.unwrap();
        assert!(archived.join(MANIFEST).is_file() && archived.join(NOTES).is_file());
        // Branches are kept in the main clones.
        let api_repo = &st
            .members
            .iter()
            .find(|m| m.dir == "api")
            .unwrap()
            .repo_path;
        assert!(git::ref_exists(api_repo, "refs/heads/sso"));
        assert!(
            list(&cfg).unwrap().is_empty(),
            "archived features are not listed"
        );
    }

    #[test]
    fn conflicts_are_reported_before_anything_is_created() {
        let tmp = tempfile::tempdir().unwrap();
        let tmp = real(tmp.path());
        let cfg = cfg(&tmp);
        let api = remote(&tmp, "api");
        let web = remote(&tmp, "web");
        // `web` already has the branch checked out in its main clone.
        let web_repo = get::run(&cfg, &web, &get::GetOptions::default())
            .unwrap()
            .path;
        testutil::git(&web_repo, &["switch", "-q", "-c", "sso"]);

        let report = new(&cfg, "sso", &[api, web], &NewOptions::default()).unwrap();
        let codes: Vec<&str> = report
            .members
            .iter()
            .map(|m| m.error.as_ref().unwrap().code)
            .collect();
        assert!(codes.contains(&"branch_checked_out_elsewhere"), "{codes:?}");
        assert!(codes.contains(&"not_planned"), "{codes:?}");
        assert!(
            !path(&cfg, "sso").join("api").exists(),
            "nothing was created"
        );
    }

    #[test]
    fn names_dirs_and_order() {
        assert!(validate_name("plat_123_sso").is_ok());
        assert!(validate_name("Feat/x").is_err());
        assert!(r10k_safe("plat_123_sso") && !r10k_safe("feat-sso"));
        let m = |dir: &str, deps: &[&str]| Member {
            identity: format!("h/o/{dir}"),
            dir: dir.into(),
            repo_path: PathBuf::new(),
            base: None,
            depends: deps.iter().map(|d| d.to_string()).collect(),
        };
        let members = vec![m("app", &["live"]), m("live", &["mods"]), m("mods", &[])];
        let order: Vec<&str> = dependency_order(&members)
            .into_iter()
            .map(|i| members[i].dir.as_str())
            .collect();
        assert_eq!(order, vec!["mods", "live", "app"]);
        assert_eq!(unique_dir(&members, "app", "other"), "other-app");
        assert_eq!(unique_dir(&members, "new", "acme"), "new");
        assert_eq!(
            parse_depends("live=mods, app").unwrap(),
            (
                "live".to_string(),
                vec!["mods".to_string(), "app".to_string()]
            )
        );
    }
}
