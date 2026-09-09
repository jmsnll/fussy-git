# status, pull, fetch

```text
fussy-git status [QUERY] [--jobs <N>]
fussy-git pull   [QUERY] [--jobs <N>]
fussy-git fetch  [QUERY] [--jobs <N>]
```

Run a git operation across every managed repository in parallel and print a
compact report.

| Command  | Operation                                             |
| -------- | -------------------------------------------------- |
| `status` | Working-tree status — branch, ahead/behind, dirt.     |
| `pull`   | Fast-forward-only pull.                               |
| `fetch`  | `git fetch` for every remote.                         |

| Flag           | Meaning                                                       |
| -------------- | --------------------------------------------------------- |
| `QUERY`        | Case-insensitive substring filter on the repository path.     |
| `-j`, `--jobs` | Max concurrent git processes. Defaults to config `jobs`.      |

If any repository fails, the others still run and the command exits `4`.

```sh
fussy-git status                 # everything
fussy-git pull github.com/acme   # just one owner's repos
fussy-git fetch --jobs 4
```
