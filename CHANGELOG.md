# Changelog

All notable changes to this project are documented here. The format is based on
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and this project
adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Added

- `feature new|list|path|status|exec|done`: one folder per cross-repository
  change, under `features_root` (default `<root>/.features`), with a worktree of
  each repository on the same branch.
  - `new` clones any missing repositories, fetches, checks every repository
    before creating anything (each problem gets a stable error code), locks
    each worktree, and is resumable. Running it again extends the feature.
  - A `.fussy-feature.toml` manifest records the members, branch, bases and
    `--depends` order. A generated `CLAUDE.md` holds facts only; `NOTES.md` is
    never rewritten.
  - `status` lists members dependencies-first, with dirty and unpushed state.
    `exec` runs a command across the members.
  - `done` refuses on uncommitted changes, and on unpushed commits unless you
    pass `--force`. It keeps the branches and archives the folder.
  - A warning when the branch isn't a valid Puppet r10k environment name.
- `worktree list` shows the `feature` each worktree belongs to.
- `features_root` config key.

### Changed

- `doctor` and `remove` point to `feature new` and `feature done` for feature
  worktrees.
- The plugin skill covers feature workflows: dependency order, pinned
  cross-repository references, and leaving `done`'s refusal in place.

## [0.3.0] - 2026-11-01

fussy-git decides where repositories live and works across many of them. It
now leaves single-repository worktrees to tools built for them, such as
[Worktrunk](https://worktrunk.dev) and Claude Code's `--worktree`.

### Added

- `cd repo@branch`, and therefore `fg repo@branch`, jumps to the checkout of
  that branch: the main clone or a linked worktree made by any tool. `/` and `-`
  are interchangeable, matching Worktrunk's directory names.
- `doctor` reports broken worktrees, meaning worktrees whose directory was
  deleted.
- A "Using with Worktrunk" guide.
- Config warnings: keys that are accepted but do nothing are reported on
  stderr.

### Changed

- Linked worktrees are no longer treated as repositories. Worktrunk's default
  layout (`api.feat-auth` beside `api`) previously made `cd api` ambiguous and
  listed worktrees as repos. The index cache format changes, and the cache
  rebuilds itself automatically.
- `worktree list` is read-only and reports worktrees from any tool.
- The Claude Code plugin skill points to `wt` or `claude --worktree` for
  parallel branches.

### Removed

- `worktree add`, `worktree prune` and `worktree remove`. These now exit with
  a pointer to `wt` or `git worktree`.
- The config keys `worktree_root`, `hooks.post_worktree`, `fork` (never
  implemented) and `hooks.post_reconcile` (never ran). They are still accepted,
  with a warning.

### Fixed

- `remove --force` no longer deletes a repository that has linked worktrees.
  Before this fix, the worktrees were left pointing at a deleted git directory.

## [0.2.0] - 2026-11-01

Support for agent-driven workflows.

### Added

- Global `--json` flag. Every command writes one JSON document to stdout, and
  errors become `{"error": {"code", "message", "candidates"?}}`, where an
  ambiguous query lists its candidates.
- `worktree add|list|prune|remove`: linked worktrees at
  `<worktree_root>/<host>/<owner>/<repo>/<branch>` (default
  `<root>/.worktrees`), and cleanup of missing, merged or upstream-deleted
  worktrees. `prune` is a dry run unless `--apply`; dirty and locked worktrees
  are never removed.
- `exec [query] -- <cmd>`: run a command across repositories in parallel, with
  per-repository exit code and captured output.
- `info` (alias `here`): identity, placement, branch, default branch, state and
  worktrees for one repository, by default the one containing the current
  directory.
- Named sets: `[sets]` in the config, addressed as `@name` by `get`, `list`,
  `status`, `pull`, `fetch`, `exec` and `worktree list|prune`.
- `reconcile --apply --plan <file>`: apply exactly a plan reviewed from
  `reconcile --json`, refusing if the tree has changed since.
- `remove --dry-run`.
- `doctor` reports prunable worktrees.
- `hook pre-tool-use`: a Claude Code `PreToolUse` hook that blocks `git clone`
  and suggests the equivalent `fussy-git get`.
- A Claude Code plugin (skill and hook) and marketplace manifest in the
  repository.
- `worktree_root` config key and `post_worktree` hook.

### Changed

- Prompts fail immediately with an explanation when stdin is not a terminal,
  instead of reading EOF as "no" and exiting `0`. Pass `--yes` in scripts that
  previously piped `yes`.
- `get` clones into a hidden staging directory and renames it into place, so a
  failed clone leaves nothing behind and concurrent `get`s of one repository
  cannot collide.
- The index cache is written atomically.
- `cd` reports ambiguity with the same wording as `remove`.
- `list --json` is now the global `--json` flag; `fussy-git list --json` works
  as before.

### Fixed

- `get --update` no longer prints git's "Already up to date." on stdout, which
  ended up inside `cd "$(fussy-git get -u …)"`. Hook output also goes to stderr.

## [0.1.1] - 2026-09-09

### Changed

- Documentation only: the README now lists the Homebrew, crates.io, and
  prebuilt-binary install methods. No code changes from 0.1.0.

## [0.1.0] - 2026-09-09

Initial release.

### Added

- `get` — clone a repository into its canonical path derived from the remote
  URL; idempotent, with `--branch`, `--shallow`, and `--update`.
- `list` — enumerate managed repositories from a mtime-invalidated index, with
  `--path`, `--json`, `--dirty`, `--unpushed`, `--host`, and `--owner` filters.
- `cd` — resolve a query to a single repository path for shell `cd`.
- `status`, `pull`, `fetch` — parallel git operations across every managed
  repository, with a compact report and a partial-failure exit code.
- `reconcile` — report misplaced repositories, duplicates, and path collisions;
  `--apply` performs the moves with a rename (and a cross-filesystem fallback),
  prunes emptied parent directories, and runs `post_move` hooks.
- `adopt` — move an existing checkout to its canonical path, adding an `origin`
  from `--as` when the repository has no usable remote.
- `remove` — delete one managed repository after checking for unsaved work.
- `doctor` — read-only health report covering misplaced, duplicate, broken,
  and detached repositories, plus staleness with `--stale`.
- `browse` — full-screen fuzzy repository picker.
- `shell-init` — `fg` shell integration for bash, zsh, and fish.
- `root` — print the configured roots.
- `--color=auto|never|always` global flag. Output is coloured only on a
  terminal and honours `NO_COLOR` and `CLICOLOR_FORCE`.
- Configuration via `config.toml` with per-host path templates, host aliases,
  ignore globs, collision policy, and lifecycle hooks. SSH `Host` aliases and
  `url.insteadOf` are read live from git and ssh config.

[0.3.0]: https://github.com/jmsnll/fussy-git/releases/tag/v0.3.0
[0.2.0]: https://github.com/jmsnll/fussy-git/releases/tag/v0.2.0
[0.1.1]: https://github.com/jmsnll/fussy-git/releases/tag/v0.1.1
[0.1.0]: https://github.com/jmsnll/fussy-git/releases/tag/v0.1.0
