# get

```text
fussy-git get <TARGET> [--branch <NAME>] [--shallow] [--update]
```

Clone a repository into its canonical path and print that path on stdout.
Repeating the command is a no-op rather than an error, so it is safe in setup
scripts.

## Target forms

| Form                                | Example                                    |
| ----------------------------------- | ------------------------------------------ |
| Full URL (any scheme)               | `git@github.com:jmsnll/fussy-git.git`      |
| `host/owner/repo`                   | `github.com/jmsnll/fussy-git`              |
| `owner/repo` (uses `default_host`)  | `jmsnll/fussy-git`                         |
| `alias:owner/repo`                  | `gh:jmsnll/fussy-git`                      |

SSH `Host` aliases from `~/.ssh/config` and `url.insteadOf` rewrites are honoured
because the clone shells out to `git`.

## Flags

| Flag              | Meaning                                              |
| ----------------- | -------------------------------------------------- |
| `-b`, `--branch`  | Check out a specific branch.                         |
| `--shallow`       | Shallow clone (`--depth 1`).                         |
| `-u`, `--update`  | Fast-forward pull if the repository already exists.  |

## Hooks

After a successful clone, each `hooks.post_get` command runs with the new
repository as its working directory.

## Examples

```sh
cd "$(fussy-git get jmsnll/fussy-git)"
fussy-git get --shallow --branch main github.com/rust-lang/rust
fussy-git get gh:jmsnll/fussy-git --update
```
