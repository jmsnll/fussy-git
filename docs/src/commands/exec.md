# exec

```text
fussy-git exec [QUERY | @set] [--jobs <N>] [--timeout <SECS>] -- <COMMAND> [ARGS…]
```

Run one command in every selected repository, in parallel, with the repository
as the working directory. Output is captured per repository and printed in a
stable order, so results never interleave.

The command is executed directly, not through a shell. For pipes, globbing or
`&&`, say so explicitly:

```sh
fussy-git exec -- git log -1 --oneline
fussy-git exec @payments -- sh -c 'rg -l "OldClient" || true'
fussy-git exec github.com/acme --json -- cargo update -p serde
```

Each run gets two environment variables:

| Variable              | Value                                              |
| --------------------- | -------------------------------------------------- |
| `FUSSY_GIT_REPO`      | Path relative to its root, e.g. `github.com/acme/api`. |
| `FUSSY_GIT_REPO_PATH` | Absolute path to the repository.                   |

A repository fails if the command exits non-zero, cannot be started, or runs
past `--timeout` (default 600 seconds). Any failure makes the batch exit `4`.
With `--json` each element is
`{repo, path, ok, exit_code, stdout, stderr, error?}`.
