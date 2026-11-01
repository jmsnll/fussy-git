# list & cd

## list

```text
fussy-git list [QUERY] [--host <H>] [--owner <O>] [--dirty] [--unpushed]
               [--path | --json] [--no-cache]
```

Enumerate managed repositories from an index that is invalidated by directory
mtime, so it is cheap to run repeatedly.

| Flag         | Meaning                                                        |
| ------------ | ----------------------------------------------------------- |
| `QUERY`      | Case-insensitive substring of `host/owner/repo`, or `@set`.    |
| `--host`     | Filter by host.                                                |
| `--owner`    | Filter by owner.                                               |
| `--dirty`    | Only repositories with uncommitted changes.                    |
| `--unpushed` | Only repositories with unpushed commits.                       |
| `--path`     | Print absolute paths, one per line (pairs well with `fzf`).    |
| `--json`     | Print JSON.                                                    |
| `--no-cache` | Rebuild the index instead of trusting the cache.               |

```sh
fussy-git list --dirty
fussy-git list --host github.com --owner jmsnll
fussy-git list --path | fzf
fussy-git list --json | jq '.[].path'
```

## cd

```text
fussy-git cd <QUERY>
```

Resolve a query to the path of the **single** matching repository and print it.
Ambiguous or unmatched queries are an error. This is the primitive the
[shell integration](./shell.md) is built on; most people use the `fzg` function
rather than calling `cd` directly.

`QUERY@BRANCH` picks the checkout of that branch, which is either the main
clone or a linked [worktree](./worktree.md): `fussy-git cd api@feat-auth`.
Linked worktrees are never matched as repositories in their own right.

```sh
cd "$(fussy-git cd fussy)"
fussy-git cd fussy --json      # {"path": …}, or {"error": {"code": "ambiguous", "candidates": […]}}
```

The index notices new and removed repositories through directory mtimes. It
does not notice a changed remote URL; run `list --no-cache` after re-pointing
a remote.
