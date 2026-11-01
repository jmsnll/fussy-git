//! fussy-git command-line interface.
//!
//! Output discipline: stdout carries data that another program might consume
//! (paths, tables, JSON), stderr carries progress and diagnostics. This lets
//! `cd "$(fussy-git get …)"` and `fussy-git list --path | fzf` work without
//! filtering.
//!
//! `--json` (global) switches every command's stdout to a single JSON document.
//! A failure under `--json` is also JSON on stdout — `{"error": {"code",
//! "message", …}}` — so a caller never has to parse prose to find out what went
//! wrong.
//!
//! Exit codes: `0` success, `1` runtime error, `2` usage error (from clap),
//! `3` drift found by a read-only check (`reconcile` dry-run, `doctor`),
//! `4` partial failure across a batch of repositories.

use std::io::{self, IsTerminal, Read, Write};
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::time::Duration;

use anyhow::{Context, Result};
use clap::{Args, Parser, Subcommand, ValueEnum};

use fussy_git::config::{self, Config};
use fussy_git::query::MatchError;
use fussy_git::ui::Format;
use fussy_git::{
    adopt, agent_hook, bulk, doctor, exec, feature, get, index, info, list, reconcile, remove,
    scan, sets, shell, tui, ui, worktree,
};

/// A read-only check that found drift returns this so CI can gate on it.
const EXIT_DRIFT: u8 = 3;
/// At least one repository in a batch operation failed.
const EXIT_PARTIAL: u8 = 4;

#[derive(Parser)]
#[command(
    name = "fussy-git",
    version,
    about = "Keep cloned git repositories organised under one root, by remote URL"
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
    /// When to colourise output.
    #[arg(long, value_enum, default_value_t = ColorArg::Auto, global = true)]
    color: ColorArg,
    /// Write a single JSON document to stdout instead of the human report.
    /// Errors are JSON too: `{"error": {"code", "message", …}}`.
    #[arg(long, global = true)]
    json: bool,
}

#[derive(Clone, Copy, ValueEnum)]
enum ColorArg {
    Auto,
    Always,
    Never,
}

impl From<ColorArg> for ui::ColorChoice {
    fn from(a: ColorArg) -> Self {
        match a {
            ColorArg::Auto => ui::ColorChoice::Auto,
            ColorArg::Always => ui::ColorChoice::Always,
            ColorArg::Never => ui::ColorChoice::Never,
        }
    }
}

#[derive(Subcommand)]
enum Command {
    /// Clone a repository (or every member of an `@set`) into its canonical
    /// path. Repeating the command is a no-op rather than an error.
    Get(GetArgs),
    /// List managed repositories.
    List(ListArgs),
    /// Print the path of the single repository matching a query, for shell `cd`.
    Cd { query: String },
    /// Describe one repository: identity, placement, branch, state, worktrees.
    /// Defaults to the repository containing the current directory.
    #[command(visible_alias = "here")]
    Info {
        /// A query, or a path (`.`, `./x`, `/abs`, `~/x`). Defaults to `.`.
        query: Option<String>,
    },
    /// Show working-tree status across repositories.
    Status(BulkArgs),
    /// Fast-forward pull across repositories.
    Pull(BulkArgs),
    /// Fetch across repositories.
    Fetch(BulkArgs),
    /// Run a command in every selected repository, in parallel.
    Exec(ExecArgs),
    /// Move misplaced repositories to their canonical path. Without `--apply`
    /// this only reports what would change.
    Reconcile(ReconcileArgs),
    /// Bring an existing checkout under management by moving it into place.
    Adopt(AdoptArgs),
    /// Delete one managed repository after checking for unsaved work.
    Remove(RemoveArgs),
    /// Report on the health of the managed tree without changing anything.
    Doctor(DoctorArgs),
    /// List linked worktrees across repositories, whichever tool made them.
    #[command(subcommand)]
    Worktree(WorktreeCmd),
    /// One folder per cross-repository change: a worktree of each repository
    /// on the same branch, for people and agents to work in.
    #[command(subcommand)]
    Feature(FeatureCmd),
    /// Interactive repository browser.
    Browse,
    /// Print shell integration to pass to `eval`.
    ShellInit { shell: ShellArg },
    /// Print the configured root directory or directories.
    Root,
    /// Hooks for coding agents. See the docs for wiring them up.
    #[command(subcommand)]
    Hook(HookCmd),
}

#[derive(Args)]
struct GetArgs {
    /// URL, `owner/repo`, `host/owner/repo`, `alias:owner/repo`, or `@set`.
    target: String,
    /// Check out a specific branch.
    #[arg(short, long)]
    branch: Option<String>,
    /// Shallow clone (`--depth 1`).
    #[arg(long)]
    shallow: bool,
    /// Fast-forward pull when the repository already exists.
    #[arg(short, long)]
    update: bool,
}

#[derive(Args)]
struct ListArgs {
    /// Case-insensitive substring of `host/owner/repo`, or `@set`.
    query: Option<String>,
    #[arg(long)]
    host: Option<String>,
    #[arg(long)]
    owner: Option<String>,
    /// Restrict to repositories with uncommitted changes.
    #[arg(long)]
    dirty: bool,
    /// Restrict to repositories with unpushed commits.
    #[arg(long)]
    unpushed: bool,
    /// Print absolute paths, one per line.
    #[arg(long)]
    path: bool,
    /// Rebuild the index rather than trusting the cache.
    #[arg(long)]
    no_cache: bool,
}

#[derive(Args)]
struct BulkArgs {
    /// Case-insensitive substring filter on the repository path, or `@set`.
    query: Option<String>,
    /// Maximum number of concurrent git processes.
    #[arg(short, long)]
    jobs: Option<usize>,
}

#[derive(Args)]
struct ExecArgs {
    /// Case-insensitive substring filter on the repository path, or `@set`.
    query: Option<String>,
    /// Maximum number of concurrent commands.
    #[arg(short, long)]
    jobs: Option<usize>,
    /// Per-repository timeout, in seconds.
    #[arg(long, default_value_t = 600)]
    timeout: u64,
    /// The command and its arguments, after `--`. Run directly, not through a
    /// shell; use `-- sh -c '…'` for pipes.
    #[arg(last = true, required = true, value_name = "COMMAND")]
    command: Vec<String>,
}

#[derive(Args)]
struct ReconcileArgs {
    /// Perform the moves. The default is a dry-run.
    #[arg(long)]
    apply: bool,
    /// Apply exactly this reviewed plan (from `reconcile --json`; `-` reads
    /// stdin), refusing if the tree has changed since. Implies no prompt.
    #[arg(long, value_name = "FILE", requires = "apply")]
    plan: Option<PathBuf>,
    /// Skip the confirmation prompt.
    #[arg(long)]
    yes: bool,
    /// Move repositories even with uncommitted changes.
    #[arg(long)]
    allow_dirty: bool,
    /// Leave a symlink at each old location so hard-coded paths keep working.
    #[arg(long)]
    leave_symlink: bool,
    /// How to handle two identities that map to one path.
    #[arg(long, value_enum)]
    on_collision: Option<CollisionArg>,
}

#[derive(Args)]
struct AdoptArgs {
    /// Repository to adopt. Defaults to the current directory.
    #[arg(default_value = ".")]
    path: String,
    /// Force the target identity when the repository has no usable remote:
    /// `host/owner/repo`, `alias:owner/repo`, or a URL.
    #[arg(long = "as", value_name = "IDENTITY")]
    as_identity: Option<String>,
    /// Adopt even with uncommitted changes.
    #[arg(long)]
    allow_dirty: bool,
    /// Leave a symlink at the old location.
    #[arg(long)]
    leave_symlink: bool,
}

#[derive(Args)]
struct RemoveArgs {
    /// Case-insensitive substring of `host/owner/repo`.
    query: String,
    /// Remove even when the safety checks report unsaved work.
    #[arg(long)]
    force: bool,
    /// Skip the confirmation prompt.
    #[arg(short, long)]
    yes: bool,
    /// Report what would be removed, and any blockers, without removing it.
    #[arg(long)]
    dry_run: bool,
}

#[derive(Args)]
struct DoctorArgs {
    /// Flag repositories whose last commit is older than this, for example
    /// `90d` or `6mo`.
    #[arg(long, value_name = "DURATION")]
    stale: Option<String>,
}

#[derive(Subcommand)]
enum WorktreeCmd {
    /// List linked worktrees across repositories: Worktrunk's, Claude Code's
    /// or plain `git worktree`'s. Jump to one with `fussy-git cd repo@branch`.
    List {
        /// Case-insensitive substring filter on the repository path, or `@set`.
        query: Option<String>,
    },
    /// Removed in 0.3.0; use Worktrunk (`wt switch -c`) or `git worktree add`.
    #[command(hide = true)]
    Add {
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        _args: Vec<String>,
    },
    /// Removed in 0.3.0; use `wt remove` or `git worktree prune`.
    #[command(hide = true)]
    Prune {
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        _args: Vec<String>,
    },
    /// Removed in 0.3.0; use `wt remove` or `git worktree remove`.
    #[command(hide = true)]
    Remove {
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        _args: Vec<String>,
    },
}

#[derive(Subcommand)]
enum FeatureCmd {
    /// Create feature NAME with a worktree of each repository on one branch,
    /// cloning any that are missing; or add repositories to an existing
    /// feature. Checks every repository before creating anything. Prints the
    /// feature folder.
    New {
        /// Folder name, and the branch name unless `--branch` is given.
        name: String,
        /// Repositories: `get` targets (`owner/repo`, URLs, …) or `@set`s.
        repos: Vec<String>,
        /// Branch to use in every repository, if not NAME.
        #[arg(long)]
        branch: Option<String>,
        /// Start new branches from this revision instead of each
        /// repository's default branch.
        #[arg(long)]
        base: Option<String>,
        /// Check out a local branch that already exists instead of refusing.
        #[arg(long)]
        reuse: bool,
        /// Declare an order: `--depends terraform=tf-modules,dns` (folder
        /// names). Repeatable.
        #[arg(long, value_name = "FOLDER=DEPS")]
        depends: Vec<String>,
    },
    /// List features.
    List,
    /// Print the folder of a feature, or of one repository in it.
    Path {
        /// Defaults to the feature containing the current directory.
        name: Option<String>,
        /// A member folder, e.g. `api`.
        member: Option<String>,
    },
    /// Branch, upstream, ahead/behind, dirty and unpushed for every
    /// repository in a feature, dependencies first.
    Status {
        /// Defaults to the feature containing the current directory.
        name: Option<String>,
    },
    /// Run a command in every repository of a feature, dependencies first.
    Exec {
        /// Defaults to the feature containing the current directory.
        name: Option<String>,
        #[arg(short, long)]
        jobs: Option<usize>,
        /// Per-repository timeout, in seconds.
        #[arg(long, default_value_t = 600)]
        timeout: u64,
        /// The command and its arguments, after `--`.
        #[arg(last = true, required = true, value_name = "COMMAND")]
        command: Vec<String>,
    },
    /// Remove a feature's worktrees (keeping every branch) and archive its
    /// folder. Refuses, changing nothing, while any repository has
    /// uncommitted changes or unpushed commits.
    Done {
        name: String,
        /// Allow unpushed commits (e.g. squash-merged branches). Never
        /// overrides uncommitted changes.
        #[arg(long)]
        force: bool,
    },
}

#[derive(Subcommand)]
enum HookCmd {
    /// Claude Code `PreToolUse` hook: block `git clone` in Bash and suggest
    /// `fussy-git get`. Reads the event JSON on stdin.
    PreToolUse,
}

#[derive(Clone, Copy, ValueEnum)]
enum ShellArg {
    Bash,
    Zsh,
    Fish,
}

#[derive(Clone, Copy, ValueEnum)]
enum CollisionArg {
    Fail,
    Suffix,
    Skip,
}

impl From<CollisionArg> for config::OnCollision {
    fn from(a: CollisionArg) -> Self {
        match a {
            CollisionArg::Fail => config::OnCollision::Fail,
            CollisionArg::Suffix => config::OnCollision::Suffix,
            CollisionArg::Skip => config::OnCollision::Skip,
        }
    }
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    ui::init(cli.color.into());
    let format = if cli.json { Format::Json } else { Format::Text };
    match run(cli.command, format) {
        Ok(code) => code,
        Err(err) => {
            if format.is_json() {
                print_json_error(&err);
            } else {
                eprintln!("{} {err:#}", ui::err_prefix());
            }
            ExitCode::from(1)
        }
    }
}

/// Report `err` as `{"error": {...}}` on stdout. A query that matched several
/// repositories includes the candidates, so the caller can pick one.
fn print_json_error(err: &anyhow::Error) {
    let mut body = serde_json::json!({
        "code": "error",
        "message": format!("{err:#}"),
    });
    if let Some(m) = err.downcast_ref::<MatchError>() {
        body["code"] = m.code().into();
        if let MatchError::Ambiguous { candidates, .. } = m {
            body["candidates"] = serde_json::to_value(candidates).unwrap_or_default();
        }
    }
    let _ = ui::print_json(&serde_json::json!({ "error": body }));
}

fn run(command: Command, format: Format) -> Result<ExitCode> {
    // The hook must answer even when the config is broken: failing to load it
    // would otherwise block every Bash call the agent makes.
    if let Command::Hook(HookCmd::PreToolUse) = command {
        return cmd_pre_tool_use();
    }
    let cfg = Config::load().context("loading configuration")?;
    for w in &cfg.warnings {
        eprintln!("{} {w}", ui::warn_prefix());
    }

    match command {
        Command::Get(a) => cmd_get(&cfg, a, format),
        Command::List(a) => cmd_list(&cfg, a, format),
        Command::Cd { query } => cmd_cd(&cfg, &query, format),
        Command::Info { query } => cmd_info(&cfg, query.as_deref(), format),
        Command::Status(a) => cmd_bulk(&cfg, a, Bulk::Status, format),
        Command::Pull(a) => cmd_bulk(&cfg, a, Bulk::Pull, format),
        Command::Fetch(a) => cmd_bulk(&cfg, a, Bulk::Fetch, format),
        Command::Exec(a) => cmd_exec(&cfg, a, format),
        Command::Reconcile(a) => cmd_reconcile(&cfg, a, format),
        Command::Adopt(a) => cmd_adopt(&cfg, a, format),
        Command::Remove(a) => cmd_remove(&cfg, a, format),
        Command::Doctor(a) => cmd_doctor(&cfg, a, format),
        Command::Worktree(w) => cmd_worktree(&cfg, w, format),
        Command::Feature(f) => cmd_feature(&cfg, f, format),
        Command::Browse => cmd_browse(&cfg),
        Command::ShellInit { shell } => {
            let s = match shell {
                ShellArg::Bash => shell::Shell::Bash,
                ShellArg::Zsh => shell::Shell::Zsh,
                ShellArg::Fish => shell::Shell::Fish,
            };
            print!("{}", shell::init_script(s));
            Ok(ExitCode::SUCCESS)
        }
        Command::Root => {
            if format.is_json() {
                ui::print_json(&serde_json::json!({
                    "root": cfg.root,
                    "roots": cfg.roots,
                }))?;
            } else {
                for root in &cfg.roots {
                    println!("{}", root.display());
                }
            }
            Ok(ExitCode::SUCCESS)
        }
        Command::Hook(HookCmd::PreToolUse) => unreachable!("handled before loading config"),
    }
}

fn cmd_get(cfg: &Config, a: GetArgs, format: Format) -> Result<ExitCode> {
    let opts = get::GetOptions {
        branch: a.branch,
        shallow: a.shallow,
        update: a.update,
    };
    let Some(set) = sets::set_name(&a.target) else {
        let got = get::run(cfg, &a.target, &opts)?;
        if format.is_json() {
            ui::print_json(&got)?;
        } else {
            println!("{}", got.path.display());
        }
        return Ok(ExitCode::SUCCESS);
    };

    // A set: clone every member, carrying on past failures.
    let mut results = Vec::new();
    let mut failed = false;
    for target in sets::targets(cfg, set)? {
        match get::run(cfg, target, &opts) {
            Ok(got) => {
                if !format.is_json() {
                    println!("{}", got.path.display());
                }
                results.push(serde_json::to_value(&got)?);
            }
            Err(e) => {
                failed = true;
                eprintln!("{} {target}: {e:#}", ui::err_prefix());
                results.push(serde_json::json!({
                    "target": target,
                    "error": format!("{e:#}"),
                }));
            }
        }
    }
    if format.is_json() {
        ui::print_json(&results)?;
    }
    Ok(if failed {
        ExitCode::from(EXIT_PARTIAL)
    } else {
        ExitCode::SUCCESS
    })
}

fn cmd_list(cfg: &Config, a: ListArgs, format: Format) -> Result<ExitCode> {
    if a.path && format.is_json() {
        anyhow::bail!("--path and --json cannot be used together");
    }
    if a.no_cache {
        index::refresh(cfg).context("rebuilding index")?;
    }
    let list_format = if format.is_json() {
        list::ListFormat::Json
    } else if a.path {
        list::ListFormat::Paths
    } else {
        list::ListFormat::Table
    };
    let (query, slugs) = match a.query.as_deref().and_then(sets::set_name) {
        Some(set) => {
            let slugs = sets::members(cfg, set)?
                .iter()
                .map(|id| id.to_string())
                .collect();
            (None, Some(slugs))
        }
        None => (a.query, None),
    };
    let filter = list::ListFilter {
        query,
        host: a.host,
        owner: a.owner,
        dirty: a.dirty,
        unpushed: a.unpushed,
        slugs,
    };
    list::run(cfg, &filter, list_format)?;
    Ok(ExitCode::SUCCESS)
}

fn cmd_cd(cfg: &Config, query: &str, format: Format) -> Result<ExitCode> {
    let path = shell::cd_target(cfg, query)?;
    if format.is_json() {
        ui::print_json(&serde_json::json!({ "path": path }))?;
    } else {
        println!("{}", path.display());
    }
    Ok(ExitCode::SUCCESS)
}

fn cmd_info(cfg: &Config, query: Option<&str>, format: Format) -> Result<ExitCode> {
    let info = info::run(cfg, query)?;
    if format.is_json() {
        ui::print_json(&info)?;
    } else {
        print!("{}", info.render(cfg));
    }
    Ok(ExitCode::SUCCESS)
}

#[derive(Clone, Copy)]
enum Bulk {
    Status,
    Pull,
    Fetch,
}

fn cmd_bulk(cfg: &Config, a: BulkArgs, which: Bulk, format: Format) -> Result<ExitCode> {
    let repos = bulk::select(cfg, a.query.as_deref())?;
    if repos.is_empty() {
        return no_repos(format);
    }
    let jobs = a.jobs.unwrap_or(cfg.jobs);
    let code = match which {
        Bulk::Status => bulk::status(cfg, repos, jobs, format)?,
        Bulk::Pull => bulk::pull(cfg, repos, jobs, format)?,
        Bulk::Fetch => bulk::fetch(cfg, repos, jobs, format)?,
    };
    Ok(ExitCode::from(code as u8))
}

fn cmd_exec(cfg: &Config, a: ExecArgs, format: Format) -> Result<ExitCode> {
    let repos = bulk::select(cfg, a.query.as_deref())?;
    if repos.is_empty() {
        return no_repos(format);
    }
    let code = exec::run(
        cfg,
        repos,
        a.command,
        a.jobs.unwrap_or(cfg.jobs),
        Duration::from_secs(a.timeout),
        format,
    )?;
    Ok(ExitCode::from(code as u8))
}

/// A selection that matched nothing: say so, and keep JSON output valid.
fn no_repos(format: Format) -> Result<ExitCode> {
    eprintln!("no repositories match");
    if format.is_json() {
        ui::print_json(&serde_json::json!([]))?;
    }
    Ok(ExitCode::SUCCESS)
}

fn cmd_reconcile(cfg: &Config, a: ReconcileArgs, format: Format) -> Result<ExitCode> {
    let reviewed = a
        .plan
        .as_deref()
        .map(reconcile::PlanDocument::load)
        .transpose()?;
    let plan_opts = reconcile::PlanOptions {
        allow_dirty: reviewed
            .as_ref()
            .map(|d| d.allow_dirty)
            .unwrap_or(a.allow_dirty),
    };
    let discovered = scan::scan(cfg)?;
    let plan = reconcile::plan(cfg, &discovered, &plan_opts)?;

    if !a.apply {
        let drift = plan.has_drift();
        if format.is_json() {
            ui::print_json(&reconcile::PlanDocument::new(plan, &plan_opts))?;
        } else {
            print!("{}", plan.render(cfg));
            if drift {
                println!("\nrun with --apply to make these changes");
            }
        }
        return Ok(if drift {
            ExitCode::from(EXIT_DRIFT)
        } else {
            ExitCode::SUCCESS
        });
    }

    if let Some(doc) = &reviewed {
        reconcile::verify_unchanged(&doc.plan, &plan)?;
    } else if !format.is_json() {
        print!("{}", plan.render(cfg));
    }

    let report = if plan.moves.is_empty() && plan.collisions.is_empty() {
        reconcile::ApplyReport::default()
    } else {
        // A reviewed plan file is the confirmation.
        if reviewed.is_none() && !a.yes {
            if !format.is_json() {
                println!();
            }
            if !confirm("Apply these changes?")? {
                eprintln!("{}", ui::note("aborted"));
                return Ok(ExitCode::SUCCESS);
            }
        }
        if !format.is_json() {
            println!();
        }
        reconcile::apply(
            cfg,
            &plan,
            &reconcile::ApplyOptions {
                on_collision: a.on_collision.map(Into::into).unwrap_or(cfg.on_collision),
                leave_symlink: a.leave_symlink,
                allow_dirty: plan_opts.allow_dirty,
            },
        )?
    };

    if format.is_json() {
        ui::print_json(&report.to_json())?;
    } else if !report.moved.is_empty() || !report.failed.is_empty() || !report.skipped.is_empty() {
        print!("{}", report.render(cfg));
    }
    for w in &report.warnings {
        eprintln!("{} {w}", ui::warn_prefix());
    }
    Ok(if report.failed.is_empty() {
        ExitCode::SUCCESS
    } else {
        ExitCode::from(EXIT_PARTIAL)
    })
}

fn cmd_adopt(cfg: &Config, a: AdoptArgs, format: Format) -> Result<ExitCode> {
    let opts = adopt::AdoptOptions {
        as_identity: a.as_identity,
        allow_dirty: a.allow_dirty,
        leave_symlink: a.leave_symlink,
    };
    let from = std::fs::canonicalize(&a.path).unwrap_or_else(|_| PathBuf::from(&a.path));
    let path = adopt::run(cfg, Path::new(&a.path), &opts)?;
    // The move changed the tree; a stale index would misreport it until the
    // next scan.
    index::refresh(cfg).ok();
    if format.is_json() {
        ui::print_json(&serde_json::json!({
            "from": from,
            "path": path,
            "moved": from != path,
        }))?;
    } else {
        println!("{}", path.display());
    }
    Ok(ExitCode::SUCCESS)
}

fn cmd_remove(cfg: &Config, a: RemoveArgs, format: Format) -> Result<ExitCode> {
    let plan = remove::plan(cfg, &a.query)?;
    let report = |removed: bool| -> Result<()> {
        if format.is_json() {
            let mut v = serde_json::to_value(&plan)?;
            v["removed"] = removed.into();
            ui::print_json(&v)?;
        }
        Ok(())
    };

    if !format.is_json() || a.dry_run {
        eprintln!("remove {}", plan.label());
        for blocker in &plan.blockers {
            eprintln!("  {} {blocker}", ui::warn_prefix());
        }
        for wt in &plan.worktrees {
            eprintln!(
                "  {} linked worktree: {} (blocks removal)",
                ui::warn_prefix(),
                wt.display()
            );
        }
    }
    if a.dry_run {
        report(false)?;
        return Ok(ExitCode::SUCCESS);
    }
    remove::check(&plan, a.force)?;
    // --force overrides the safety checks but still confirms, unless --yes.
    if !a.yes && !confirm("Remove this repository?")? {
        eprintln!("{}", ui::note("aborted"));
        return Ok(ExitCode::SUCCESS);
    }
    remove::execute(cfg, &plan, a.force)?;
    eprintln!("{}", ui::note(&format!("removed {}", plan.label())));
    report(true)?;
    Ok(ExitCode::SUCCESS)
}

fn cmd_doctor(cfg: &Config, a: DoctorArgs, format: Format) -> Result<ExitCode> {
    let stale = a.stale.as_deref().map(doctor::parse_duration).transpose()?;
    let code = doctor::run(cfg, &doctor::DoctorOptions { stale }, format)?;
    Ok(ExitCode::from(code as u8))
}

fn cmd_worktree(cfg: &Config, cmd: WorktreeCmd, format: Format) -> Result<ExitCode> {
    let removed = |what: &str, instead: &str| -> Result<ExitCode> {
        anyhow::bail!(
            "`fussy-git worktree {what}` was removed in 0.3.0: single-repository worktrees \
             are left to tools built for them. Use {instead}."
        )
    };
    match cmd {
        WorktreeCmd::List { query } => {
            let wts = worktree::list(cfg, query.as_deref())?;
            if format.is_json() {
                ui::print_json(&wts)?;
            } else {
                print_worktrees(cfg, &wts);
            }
            Ok(ExitCode::SUCCESS)
        }
        WorktreeCmd::Add { .. } => removed("add", "`wt switch -c <branch>` or `git worktree add`"),
        WorktreeCmd::Prune { .. } => removed("prune", "`wt remove` or `git worktree prune`"),
        WorktreeCmd::Remove { .. } => removed("remove", "`wt remove` or `git worktree remove`"),
    }
}

fn print_worktrees(cfg: &Config, wts: &[worktree::Worktree]) {
    let repo_w = wts
        .iter()
        .map(|w| w.repo.chars().count())
        .max()
        .unwrap_or(0);
    let branch_w = wts
        .iter()
        .map(|w| w.branch.as_deref().unwrap_or("detached").chars().count())
        .max()
        .unwrap_or(0);
    for w in wts {
        let mut state = Vec::new();
        if w.missing {
            state.push(ui::danger("missing"));
        }
        if w.dirty == Some(true) {
            state.push(ui::warn("dirty"));
        }
        if w.locked {
            state.push(ui::dim("locked"));
        }
        let line = format!(
            "{:<repo_w$}  {:<branch_w$}  {}  {}",
            w.repo,
            w.branch.as_deref().unwrap_or("detached"),
            ui::dim(&ui::repo_name(cfg, &w.path)),
            state.join(", ")
        );
        println!("{}", line.trim_end());
    }
}

fn cmd_feature(cfg: &Config, cmd: FeatureCmd, format: Format) -> Result<ExitCode> {
    match cmd {
        FeatureCmd::New {
            name,
            repos,
            branch,
            base,
            reuse,
            depends,
        } => {
            let opts = feature::NewOptions {
                branch,
                base,
                reuse,
                depends: depends
                    .iter()
                    .map(|d| feature::parse_depends(d))
                    .collect::<Result<_>>()?,
            };
            let report = feature::new(cfg, &name, &repos, &opts)?;
            for w in &report.warnings {
                eprintln!("{} {w}", ui::warn_prefix());
            }
            if format.is_json() {
                ui::print_json(&report)?;
            } else {
                for m in &report.members {
                    let state = match m.state {
                        feature::MemberState::Created => ui::ok("created"),
                        feature::MemberState::Existing => ui::dim("existing"),
                        feature::MemberState::Failed => ui::danger("failed"),
                    };
                    eprintln!("  {:<10} {:<20} {}", state, m.dir, ui::dim(&m.identity));
                    if let Some(e) = &m.error {
                        if e.code != "not_planned" {
                            eprintln!("             {}", e.message);
                        }
                    }
                }
            }
            let failed = report.failed();
            if failed == 0 {
                if !format.is_json() {
                    println!("{}", report.path.display());
                }
                Ok(ExitCode::SUCCESS)
            } else if report
                .members
                .iter()
                .any(|m| m.state != feature::MemberState::Failed)
            {
                Ok(ExitCode::from(EXIT_PARTIAL))
            } else {
                if !format.is_json() {
                    eprintln!("nothing was created; fix the conflicts above and run it again");
                }
                Ok(ExitCode::from(1))
            }
        }
        FeatureCmd::List => {
            let features = feature::list(cfg)?;
            if format.is_json() {
                ui::print_json(&features)?;
            } else if features.is_empty() {
                eprintln!(
                    "no features (create one with `fussy-git feature new <name> <@set | repo…>`)"
                );
            } else {
                let w = features.iter().map(|f| f.name.len()).max().unwrap_or(0);
                for f in &features {
                    println!(
                        "{:<w$}  {}  {}",
                        f.name,
                        ui::dim(&format!("branch {}", f.branch)),
                        f.members.join(", ")
                    );
                }
            }
            Ok(ExitCode::SUCCESS)
        }
        FeatureCmd::Path { name, member } => {
            let name = feature::resolve_name(cfg, name.as_deref())?;
            let mut p = feature::path(cfg, &name);
            if !p.join(feature::MANIFEST).is_file() {
                anyhow::bail!("no feature named {name:?} (see `fussy-git feature list`)");
            }
            if let Some(m) = member {
                p = p.join(m);
                if !p.exists() {
                    anyhow::bail!("{} does not exist", p.display());
                }
            }
            if format.is_json() {
                ui::print_json(&serde_json::json!({ "path": p }))?;
            } else {
                println!("{}", p.display());
            }
            Ok(ExitCode::SUCCESS)
        }
        FeatureCmd::Status { name } => {
            let name = feature::resolve_name(cfg, name.as_deref())?;
            let st = feature::status(cfg, &name)?;
            if format.is_json() {
                ui::print_json(&st)?;
            } else {
                print_feature_status(&st);
            }
            Ok(ExitCode::SUCCESS)
        }
        FeatureCmd::Exec {
            name,
            jobs,
            timeout,
            command,
        } => {
            let name = feature::resolve_name(cfg, name.as_deref())?;
            let paths = feature::member_paths(cfg, &name)?;
            if paths.is_empty() {
                return no_repos(format);
            }
            let code = exec::run(
                cfg,
                paths,
                command,
                jobs.unwrap_or(cfg.jobs),
                Duration::from_secs(timeout),
                format,
            )?;
            Ok(ExitCode::from(code as u8))
        }
        FeatureCmd::Done { name, force } => {
            let report = feature::done(cfg, &name, force)?;
            if format.is_json() {
                ui::print_json(&report)?;
            } else {
                for r in report.refused.iter().chain(&report.failed) {
                    eprintln!(
                        "  {} {}",
                        ui::danger(&r.dir),
                        ui::dim(&r.path.display().to_string())
                    );
                    for why in &r.reasons {
                        eprintln!("      {why}");
                    }
                }
                if let Some(a) = &report.archived_to {
                    eprintln!(
                        "{}",
                        ui::note(&format!(
                            "removed {} worktree(s); branches kept; folder archived to {}",
                            report.removed.len(),
                            a.display()
                        ))
                    );
                } else if !report.refused.is_empty() {
                    eprintln!("nothing was changed");
                }
            }
            Ok(if report.refused.is_empty() && report.failed.is_empty() {
                ExitCode::SUCCESS
            } else if report.removed.is_empty() {
                ExitCode::from(1)
            } else {
                ExitCode::from(EXIT_PARTIAL)
            })
        }
    }
}

fn print_feature_status(st: &feature::Status) {
    println!(
        "{}  {}",
        ui::header(&st.name),
        ui::dim(&format!("branch {} · {}", st.branch, st.path.display()))
    );
    let w = st.members.iter().map(|m| m.dir.len()).max().unwrap_or(0);
    for m in &st.members {
        let mut state = Vec::new();
        match m.state {
            feature::Health::Ok => {}
            feature::Health::Missing => state.push(ui::danger("missing")),
            feature::Health::WrongBranch => state.push(ui::warn(&format!(
                "on {}",
                m.branch.as_deref().unwrap_or("detached HEAD")
            ))),
        }
        if m.dirty == Some(true) {
            state.push(ui::warn("dirty"));
        }
        match (m.unpushed, &m.upstream) {
            (Some(n), _) if n > 0 => state.push(ui::warn(&format!("{n} unpushed"))),
            (_, None) if m.state != feature::Health::Missing => state.push(ui::dim("no upstream")),
            _ => {}
        }
        if m.behind > 0 {
            state.push(ui::warn(&format!("{} behind", m.behind)));
        }
        if state.is_empty() {
            state.push(ui::ok("clean, pushed"));
        }
        let deps = if m.depends.is_empty() {
            String::new()
        } else {
            ui::dim(&format!("  (after {})", m.depends.join(", ")))
        };
        println!("  {:<w$}  {}{deps}", m.dir, state.join(", "));
    }
}

fn cmd_pre_tool_use() -> Result<ExitCode> {
    let mut input = String::new();
    io::stdin().read_to_string(&mut input).ok();
    match agent_hook::pre_tool_use(&input) {
        Some(reason) => {
            eprintln!("{reason}");
            Ok(ExitCode::from(2))
        }
        None => Ok(ExitCode::SUCCESS),
    }
}

fn cmd_browse(cfg: &Config) -> Result<ExitCode> {
    // An empty selection means the user quit without choosing; print nothing so
    // the shell `cd` wrapper stays where it is.
    match tui::browse(cfg)? {
        Some(path) => {
            println!("{}", path.display());
            Ok(ExitCode::SUCCESS)
        }
        None => Ok(ExitCode::SUCCESS),
    }
}

/// Ask a yes/no question on stderr and read the answer from stdin. Anything
/// other than an explicit yes is treated as no.
///
/// When stdin is not a terminal there is nobody to answer, so this fails
/// rather than reading: an agent or CI job would otherwise either hang or see a
/// silent "aborted" with exit 0 and believe the change was made.
fn confirm(prompt: &str) -> Result<bool> {
    if !io::stdin().is_terminal() {
        anyhow::bail!("cannot ask {prompt:?}: stdin is not a terminal; pass --yes to proceed");
    }
    eprint!("{prompt} [y/N] ");
    io::stderr().flush().ok();
    let mut line = String::new();
    io::stdin().read_line(&mut line)?;
    Ok(matches!(line.trim().to_lowercase().as_str(), "y" | "yes"))
}
