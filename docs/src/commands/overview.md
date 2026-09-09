# Commands

| Command                 | What it does                                                          |
| ----------------------- | ------------------------------------------------------------------- |
| [`get`](./get.md)               | Clone a repository into its canonical path. Idempotent.              |
| [`list`](./list.md)             | Enumerate managed repositories, with filters and JSON output.        |
| [`cd`](./list.md#cd)            | Resolve a query to a single path, for shell `cd`.                    |
| [`status`](./bulk.md)           | Working-tree status across every repository.                         |
| [`pull`](./bulk.md)             | Fast-forward pull across every repository.                           |
| [`fetch`](./bulk.md)            | Fetch across every repository.                                       |
| [`reconcile`](./reconcile.md)   | Move misplaced repositories to their canonical path.                 |
| [`adopt`](./adopt.md)           | Bring an existing checkout under management.                         |
| [`sync`](./sync.md)             | Clone whatever a `repos.toml` manifest lists but the tree is missing. |
| [`dump`](./sync.md#fussy-git-dump) | Write a manifest for the current tree to stdout.                  |
| [`doctor`](./doctor.md)         | Read-only health report for the managed tree.                        |
| [`remove`](./remove.md)         | Delete one managed repository after safety checks.                   |
| [`browse`](./browse.md)         | Interactive fuzzy repository picker.                                  |
| [`shell-init`](./shell.md)      | Print shell integration to pass to `eval`.                           |
| `root`                          | Print the configured root directory or directories.                  |

## Global flags

- `--color auto|always|never` — when to colourise output. `auto` colours only on
  a terminal and honours `NO_COLOR` and `CLICOLOR_FORCE`.

## Exit codes

| Code | Meaning                                                        |
| ---- | ------------------------------------------------------------- |
| `0`  | Success.                                                       |
| `1`  | Runtime error.                                                 |
| `2`  | Usage error.                                                   |
| `3`  | A read-only check found drift (`reconcile` dry-run, `sync` dry-run, `doctor`). |
| `4`  | At least one repository in a batch operation failed (`pull`, `fetch`, `sync --apply`). |

## Output discipline

stdout carries data another program might consume (paths, tables, JSON); stderr
carries progress and diagnostics. This is what makes
`cd "$(fussy-git get …)"` and `fussy-git list --path | fzf` work without
filtering.
