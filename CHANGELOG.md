# Changelog

All notable changes to this project are documented here. The format is based on
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and this project
adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

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

[0.1.1]: https://github.com/jmsnll/fussy-git/releases/tag/v0.1.1
[0.1.0]: https://github.com/jmsnll/fussy-git/releases/tag/v0.1.0
