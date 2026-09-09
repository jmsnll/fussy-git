//! fussy-git command-line interface.
//!
//! Output discipline: stdout carries data that another program might consume
//! (paths, tables, JSON), stderr carries progress and diagnostics. This lets
//! `cd "$(fussy-git get …)"` and `fussy-git list --path | fzf` work without
//! filtering.
//!
//! Exit codes: `0` success, `1` runtime error, `2` usage error (from clap),
//! `3` drift found by a read-only check (`reconcile` dry-run, `doctor`),
//! `4` partial failure across a batch of repositories.

use std::io::{self, Write};
use std::process::ExitCode;

use anyhow::{anyhow, Context, Result};
use clap::{Args, Parser, Subcommand, ValueEnum};

use fussy_git::config::{self, Config};
use fussy_git::{
    adopt, bulk, doctor, get, index, list, manifest, reconcile, remove, scan, shell, sync, tui, ui,
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
    /// Clone a repository into its canonical path. Repeating the command is a
    /// no-op rather than an error.
    Get(GetArgs),
    /// List managed repositories.
    List(ListArgs),
    /// Print the path of the single repository matching a query, for shell `cd`.
    Cd { query: String },
    /// Show working-tree status across repositories.
    Status(BulkArgs),
    /// Fast-forward pull across repositories.
    Pull(BulkArgs),
    /// Fetch across repositories.
    Fetch(BulkArgs),
    /// Move misplaced repositories to their canonical path. Without `--apply`
    /// this only reports what would change.
    Reconcile(ReconcileArgs),
    /// Bring an existing checkout under management by moving it into place.
    Adopt(AdoptArgs),
    /// Delete one managed repository after checking for unsaved work.
    Remove(RemoveArgs),
    /// Report on the health of the managed tree without changing anything.
    Doctor(DoctorArgs),
    /// Make the tree match a repository manifest: clone what is missing. Without
    /// `--apply` this only reports what would change.
    Sync(SyncArgs),
    /// Write a manifest for the current tree to stdout.
    Dump,
    /// Interactive repository browser.
    Browse,
    /// Print shell integration to pass to `eval`.
    ShellInit { shell: ShellArg },
    /// Print the configured root directory or directories.
    Root,
}

#[derive(Args)]
struct GetArgs {
    /// URL, `owner/repo`, `host/owner/repo`, or `alias:owner/repo`.
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
    /// Case-insensitive substring of `host/owner/repo`.
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
    #[arg(long, conflicts_with = "json")]
    path: bool,
    /// Print JSON.
    #[arg(long)]
    json: bool,
    /// Rebuild the index rather than trusting the cache.
    #[arg(long)]
    no_cache: bool,
}

#[derive(Args)]
struct BulkArgs {
    /// Case-insensitive substring filter on the repository path.
    query: Option<String>,
    /// Maximum number of concurrent git processes.
    #[arg(short, long)]
    jobs: Option<usize>,
}

#[derive(Args)]
struct ReconcileArgs {
    /// Perform the moves. The default is a dry-run.
    #[arg(long)]
    apply: bool,
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
}

#[derive(Args)]
struct SyncArgs {
    /// Clone every missing repository. The default is a dry-run.
    #[arg(long)]
    apply: bool,
    /// Path to the manifest, overriding discovery.
    #[arg(long, value_name = "PATH")]
    manifest: Option<std::path::PathBuf>,
    /// Maximum number of concurrent clones.
    #[arg(short, long)]
    jobs: Option<usize>,
}

#[derive(Args)]
struct DoctorArgs {
    /// Flag repositories whose last commit is older than this, for example
    /// `90d` or `6mo`.
    #[arg(long, value_name = "DURATION")]
    stale: Option<String>,
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
    match run() {
        Ok(code) => code,
        Err(err) => {
            eprintln!("{} {err:#}", ui::err_prefix());
            ExitCode::from(1)
        }
    }
}

fn run() -> Result<ExitCode> {
    let cli = Cli::parse();
    ui::init(cli.color.into());
    let cfg = Config::load().context("loading configuration")?;

    match cli.command {
        Command::Get(a) => cmd_get(&cfg, a),
        Command::List(a) => cmd_list(&cfg, a),
        Command::Cd { query } => cmd_cd(&cfg, &query),
        Command::Status(a) => cmd_bulk(&cfg, a, Bulk::Status),
        Command::Pull(a) => cmd_bulk(&cfg, a, Bulk::Pull),
        Command::Fetch(a) => cmd_bulk(&cfg, a, Bulk::Fetch),
        Command::Reconcile(a) => cmd_reconcile(&cfg, a),
        Command::Adopt(a) => cmd_adopt(&cfg, a),
        Command::Remove(a) => cmd_remove(&cfg, a),
        Command::Doctor(a) => cmd_doctor(&cfg, a),
        Command::Sync(a) => cmd_sync(&cfg, a),
        Command::Dump => cmd_dump(&cfg),
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
            for root in &cfg.roots {
                println!("{}", root.display());
            }
            Ok(ExitCode::SUCCESS)
        }
    }
}

fn cmd_get(cfg: &Config, a: GetArgs) -> Result<ExitCode> {
    let opts = get::GetOptions {
        branch: a.branch,
        shallow: a.shallow,
        update: a.update,
    };
    let path = get::run(cfg, &a.target, &opts)?;
    println!("{}", path.display());
    Ok(ExitCode::SUCCESS)
}

fn cmd_list(cfg: &Config, a: ListArgs) -> Result<ExitCode> {
    if a.no_cache {
        index::refresh(cfg).context("rebuilding index")?;
    }
    let format = if a.json {
        list::ListFormat::Json
    } else if a.path {
        list::ListFormat::Paths
    } else {
        list::ListFormat::Table
    };
    let filter = list::ListFilter {
        query: a.query,
        host: a.host,
        owner: a.owner,
        dirty: a.dirty,
        unpushed: a.unpushed,
    };
    list::run(cfg, &filter, format)?;
    Ok(ExitCode::SUCCESS)
}

fn cmd_cd(cfg: &Config, query: &str) -> Result<ExitCode> {
    let path = shell::cd_target(cfg, query)?;
    println!("{}", path.display());
    Ok(ExitCode::SUCCESS)
}

#[derive(Clone, Copy)]
enum Bulk {
    Status,
    Pull,
    Fetch,
}

fn cmd_bulk(cfg: &Config, a: BulkArgs, which: Bulk) -> Result<ExitCode> {
    let mut repos = bulk::all_repo_paths(cfg)?;
    if let Some(q) = &a.query {
        let needle = q.to_lowercase();
        repos.retain(|p| p.to_string_lossy().to_lowercase().contains(&needle));
    }
    if repos.is_empty() {
        eprintln!("no repositories match");
        return Ok(ExitCode::SUCCESS);
    }
    let jobs = a.jobs.unwrap_or(cfg.jobs);
    let code = match which {
        Bulk::Status => bulk::status(cfg, repos, jobs)?,
        Bulk::Pull => bulk::pull(cfg, repos, jobs)?,
        Bulk::Fetch => bulk::fetch(cfg, repos, jobs)?,
    };
    Ok(ExitCode::from(code as u8))
}

fn cmd_reconcile(cfg: &Config, a: ReconcileArgs) -> Result<ExitCode> {
    let discovered = scan::scan(cfg)?;
    let plan = reconcile::plan(
        cfg,
        &discovered,
        &reconcile::PlanOptions {
            allow_dirty: a.allow_dirty,
        },
    )?;

    print!("{}", plan.render(cfg));

    if !a.apply {
        if plan.has_drift() {
            println!("\nrun with --apply to make these changes");
        }
        return Ok(if plan.has_drift() {
            ExitCode::from(EXIT_DRIFT)
        } else {
            ExitCode::SUCCESS
        });
    }

    if plan.moves.is_empty() && plan.collisions.is_empty() {
        return Ok(ExitCode::SUCCESS);
    }

    if !a.yes {
        println!();
        if !confirm("Apply these changes?")? {
            eprintln!("{}", ui::note("aborted"));
            return Ok(ExitCode::SUCCESS);
        }
    }
    println!();

    let report = reconcile::apply(
        cfg,
        &plan,
        &reconcile::ApplyOptions {
            on_collision: a.on_collision.map(Into::into).unwrap_or(cfg.on_collision),
            leave_symlink: a.leave_symlink,
            allow_dirty: a.allow_dirty,
        },
    )?;
    print!("{}", report.render(cfg));
    for w in &report.warnings {
        eprintln!("{} {w}", ui::warn_prefix());
    }
    Ok(if report.failed.is_empty() {
        ExitCode::SUCCESS
    } else {
        ExitCode::from(EXIT_PARTIAL)
    })
}

fn cmd_adopt(cfg: &Config, a: AdoptArgs) -> Result<ExitCode> {
    let opts = adopt::AdoptOptions {
        as_identity: a.as_identity,
        allow_dirty: a.allow_dirty,
        leave_symlink: a.leave_symlink,
    };
    let path = adopt::run(cfg, std::path::Path::new(&a.path), &opts)?;
    println!("{}", path.display());
    // The move changed the tree; a stale index would misreport it until the
    // next scan.
    index::refresh(cfg).ok();
    Ok(ExitCode::SUCCESS)
}

fn cmd_remove(cfg: &Config, a: RemoveArgs) -> Result<ExitCode> {
    let plan = remove::plan(cfg, &a.query)?;
    eprintln!("remove {}", plan.label());
    for blocker in &plan.blockers {
        eprintln!("  {} {blocker}", ui::warn_prefix());
    }
    // --force overrides the safety checks but still confirms, unless --yes.
    if !a.yes && !confirm("Remove this repository?")? {
        eprintln!("{}", ui::note("aborted"));
        return Ok(ExitCode::SUCCESS);
    }
    remove::execute(cfg, &plan, a.force)?;
    eprintln!("{}", ui::note(&format!("removed {}", plan.label())));
    Ok(ExitCode::SUCCESS)
}

fn cmd_doctor(cfg: &Config, a: DoctorArgs) -> Result<ExitCode> {
    let stale = a.stale.as_deref().map(doctor::parse_duration).transpose()?;
    let code = doctor::run(cfg, &doctor::DoctorOptions { stale })?;
    Ok(ExitCode::from(code as u8))
}

fn cmd_sync(cfg: &Config, a: SyncArgs) -> Result<ExitCode> {
    let name = manifest::PROJECT_MANIFEST_NAME;
    let m = manifest::Manifest::discover(a.manifest.as_deref())?.ok_or_else(|| {
        anyhow!("no manifest found — create {name} or run `fussy-git dump > {name}`")
    })?;

    let discovered = scan::scan(cfg)?;
    let plan = sync::plan(cfg, &m, &discovered)?;
    print!("{}", plan.render(cfg));

    if !a.apply {
        if !plan.has_drift() {
            return Ok(ExitCode::SUCCESS);
        }
        if !plan.missing.is_empty() {
            println!("\nrun with --apply to clone the missing repos");
        }
        return Ok(ExitCode::from(EXIT_DRIFT));
    }

    if plan.missing.is_empty() {
        return Ok(ExitCode::SUCCESS);
    }
    println!();

    let report = sync::apply(
        cfg,
        &plan,
        &sync::ApplyOptions {
            jobs: a.jobs.unwrap_or(cfg.jobs),
        },
    )?;
    print!("{}", report.render(cfg));
    for w in &report.warnings {
        eprintln!("{} {w}", ui::warn_prefix());
    }
    Ok(if report.failed.is_empty() {
        ExitCode::SUCCESS
    } else {
        ExitCode::from(EXIT_PARTIAL)
    })
}

fn cmd_dump(cfg: &Config) -> Result<ExitCode> {
    print!("{}", manifest::dump_tree(cfg)?);
    Ok(ExitCode::SUCCESS)
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
fn confirm(prompt: &str) -> Result<bool> {
    eprint!("{prompt} [y/N] ");
    io::stderr().flush().ok();
    let mut line = String::new();
    io::stdin().read_line(&mut line)?;
    Ok(matches!(line.trim().to_lowercase().as_str(), "y" | "yes"))
}
