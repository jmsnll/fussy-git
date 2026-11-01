# Changelog

All notable changes to this project are documented here. The format is based on
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and this project
adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

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

[0.2.0]: https://github.com/jmsnll/fussy-git/releases/tag/v0.2.0
[0.1.1]: https://github.com/jmsnll/fussy-git/releases/tag/v0.1.1
[0.1.0]: https://github.com/jmsnll/fussy-git/releases/tag/v0.1.0
