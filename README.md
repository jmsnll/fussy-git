# fussy-git

Keep your cloned git repositories organised under a single root, with each
repository's path derived from its remote URL:

```
~/git/github.com/jmsnll/fussy-git
~/git/gitlab.com/acme/backend/service
```

`fussy-git` clones new repositories straight into the right place, and it can
**reconcile an existing tree**: sweep the root, read every repository's remote,
and move the misplaced ones to where they belong.

**Documentation: <https://jmsnll.github.io/fussy-git/>**

## How it works

The root is treated as state to be reconciled. A small config file describes how
a path is derived from a remote — `{host}/{owner}/{repo}` by default, overridable
per host — and `fussy-git` makes the filesystem match: new clones land in the
right place, and `reconcile` fixes anything already out of position.

Configuration that git already owns — SSH `Host` aliases, `url.insteadOf`,
credential helpers, commit signing — is read live from git and ssh config, never
duplicated. Every network and authentication operation shells out to the `git`
binary, so that configuration always applies. `fussy-git` never links libgit2.

## Install

```sh
# Homebrew
brew install jmsnll/tap/fussy-git

# crates.io
cargo install fussy-git

# Prebuilt binary (macOS, Linux)
curl --proto '=https' --tlsv1.2 -LsSf https://github.com/jmsnll/fussy-git/releases/latest/download/fussy-git-installer.sh | sh

# Nix
nix run github:jmsnll/fussy-git

# From a checkout
cargo install --path .
```

Requires a `git` binary on `PATH`. Arch Linux `PKGBUILD`s and a nixpkgs
derivation live in [`packaging/`](packaging/).

## Quick start

```sh
# Clone into the canonical path and print it
fussy-git get github.com/jmsnll/fussy-git
fussy-git get git@github.com:jmsnll/fussy-git.git   # any URL form works too

# See what is out of place, then fix it
fussy-git reconcile              # dry-run: prints the plan, changes nothing
fussy-git reconcile --apply      # perform the moves

# Work across every repository at once
fussy-git status
fussy-git pull

# Jump to a repository
eval "$(fussy-git shell-init zsh)"    # adds an `fg` function
fg fussy                              # cd to the repo matching "fussy"
```

## Configuration

`fussy-git` works with no config at all: root `~/git`, layout
`{host}/{owner}/{repo}`, shorthand host `github.com`.

To customise, create `~/.config/fussy-git/config.toml` (or `.fussy-git.toml` in
the current directory or any ancestor, or point `$FUSSY_GIT_CONFIG` at a file):

```toml
root  = "~/git"                       # where new clones land
roots = ["~/git", "~/work/git"]       # additional roots that are searched, not written to
jobs  = 12                            # concurrency for bulk operations
default_host = "github.com"           # host for the bare `owner/repo` shorthand
on_collision = "fail"                 # fail | suffix | skip
ignore = ["**/node_modules/**", "~/git/scratch/**"]

[hosts."github.com"]
alias = ["gh", "github"]              # `fussy-git get gh:jmsnll/fussy-git`
ssh = true                            # prefer git@github.com:owner/repo.git

[hosts."gitlab.internal.example"]
alias = ["work"]
template = "work/{group_path}/{repo}" # {group_path} spans GitLab subgroups
clone_scheme = "ssh"

[hooks]
post_get  = ["git config pull.ff only"]   # run in the repository after `get`
post_move = ["git maintenance run"]        # run in the repository after a reconcile move
```

Each hook command runs with the repository as its working directory, so it is
the place to wire in whatever local setup or editor and shell integrations you
use.

Template placeholders: `{host}`, `{owner}`, `{group_path}` (an alias for
`{owner}` that reads better for nested groups), `{repo}`, `{port}`.

## Commands

| Command | Purpose |
|---|---|
| `get <target>` | Clone into the canonical path. Idempotent. `--branch`, `--shallow`, `--update`. |
| `list [query]` | List managed repositories from the index. `--path`, `--json`, `--dirty`, `--unpushed`, `--host`, `--owner`. |
| `cd <query>` | Print the path of the single matching repository, for shell `cd`. |
| `status [query]` | Parallel working-tree status: branch, ahead/behind, dirty, stashes. |
| `pull [query]` | Parallel `git pull --ff-only` with a summary. |
| `fetch [query]` | Parallel `git fetch --all --prune`. |
| `reconcile` | Report misplaced repositories, duplicates, and collisions. `--apply` performs the moves; `--allow-dirty`, `--leave-symlink`, `--on-collision`. |
| `adopt <path>` | Move an existing checkout to its canonical path. `--as host/owner/repo` for a repository with no usable remote. |
| `remove <query>` | Delete one managed repository after checking for unsaved work. `--force`, `--yes`. |
| `doctor` | Read-only health report: misplaced, duplicate, broken, detached, and (with `--stale 90d`) stale repositories. |
| `browse` | Full-screen fuzzy repository picker. |
| `shell-init <bash\|zsh\|fish>` | Print the `fg` shell integration for `eval`. |
| `root` | Print the configured root or roots. |

### Exit codes

| Code | Meaning |
|---|---|
| `0` | Success |
| `1` | Runtime error |
| `2` | Usage error |
| `3` | Drift found by a read-only check (`reconcile` dry-run, `doctor`) |
| `4` | At least one repository in a batch operation failed |

Data goes to stdout, progress and diagnostics to stderr, so
`cd "$(fussy-git get …)"` and piping `fussy-git list --path` into a fuzzy finder
work without filtering. Colour follows the terminal and `NO_COLOR`; override it
with `--color=always|never`.

## Development

```sh
cargo test
cargo clippy --all-targets -- -D warnings
cargo fmt --check
```

## Status

Feature-complete for v1. A fork workflow for `get` (`--fork`) is not implemented,
and Windows is not yet supported.

## License

MIT
