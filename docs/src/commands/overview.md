# Commands

| Command                 | What it does                                                          |
| ----------------------- | ------------------------------------------------------------------- |
| [`get`](./get.md)               | Clone a repository into its canonical path. Idempotent.              |
| [`list`](./list.md)             | Enumerate managed repositories, with filters and JSON output.        |
| [`cd`](./list.md#cd)            | Resolve a query to a single path, for shell `cd`.                    |
| [`info`](./info.md)             | Describe one repository (default: the one you are in).               |
| [`status`](./bulk.md)           | Working-tree status across every repository.                         |
| [`pull`](./bulk.md)             | Fast-forward pull across every repository.                           |
| [`fetch`](./bulk.md)            | Fetch across every repository.                                       |
| [`exec`](./exec.md)             | Run a command in every selected repository, in parallel.             |
| [`reconcile`](./reconcile.md)   | Move misplaced repositories to their canonical path.                 |
| [`adopt`](./adopt.md)           | Bring an existing checkout under management.                         |
| [`doctor`](./doctor.md)         | Read-only health report for the managed tree.                        |
| [`remove`](./remove.md)         | Delete one managed repository after safety checks.                   |
| [`worktree`](./worktree.md)     | Create, list, prune and remove worktrees at canonical paths.         |
| [`browse`](./browse.md)         | Interactive fuzzy repository picker.                                  |
| [`shell-init`](./shell.md)      | Print shell integration to pass to `eval`.                           |
| `root`                          | Print the configured root directory or directories.                  |
| `hook pre-tool-use`             | Claude Code hook that redirects `git clone` ([details](../agents.md)). |

## Global flags

- `--color auto|always|never` — when to colourise output. `auto` colours only on
  a terminal and honours `NO_COLOR` and `CLICOLOR_FORCE`.
- `--json` — write one JSON document to stdout instead of the human report.
  Errors become `{"error": {"code", "message", "candidates"?}}` on stdout.

Wherever a command takes a repository query, `@name` selects a named
[set](../configuration.md#sets) instead.

## Exit codes

| Code | Meaning                                                        |
| ---- | ------------------------------------------------------------- |
| `0`  | Success.                                                       |
| `1`  | Runtime error.                                                 |
| `2`  | Usage error.                                                   |
| `3`  | A read-only check found drift (`reconcile` and `worktree prune` dry-runs, `doctor`). |
| `4`  | At least one repository in a batch operation failed.           |

## Output discipline

stdout carries data another program might consume (paths, tables, JSON); stderr
carries progress and diagnostics. This is what makes
`cd "$(fussy-git get …)"` and `fussy-git list --path | fzf` work without
filtering.
