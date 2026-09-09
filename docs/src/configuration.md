# Configuration

`fussy-git` runs with no config. To customise, create one of:

- `~/.config/fussy-git/config.toml`
- `.fussy-git.toml` in the current directory or any ancestor
- a file pointed at by `$FUSSY_GIT_CONFIG`

## Full example

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

## Keys

| Key                | Default                  | Meaning                                                          |
| ------------------ | ------------------------ | --------------------------------------------------------------- |
| `root`             | `~/git`                  | Where new clones are written.                                    |
| `roots`            | `[root]`                 | Every directory that is *searched*. Only `root` is written to.   |
| `jobs`             | `min(2 × CPUs, 16)`      | Max concurrent `git` processes for `status`/`pull`/`fetch`.      |
| `default_host`     | `github.com`             | Host assumed for the bare `owner/repo` shorthand.                |
| `default_template` | `{host}/{owner}/{repo}`  | Path template used for any host without its own `template`.      |
| `on_collision`     | `fail`                   | What to do when two remotes map to one path: `fail`/`suffix`/`skip`. |
| `fork`             | `prompt`                 | Whether `get` on a repo you cannot push to creates a fork: `prompt`/`always`/`never`. |
| `ignore`           | `[]`                     | Glob patterns for paths `reconcile` and the index skip.          |

### Per-host `[hosts."<host>"]`

| Key            | Meaning                                                                       |
| -------------- | --------------------------------------------------------------------------- |
| `alias`        | Short names usable as `alias:owner/repo`.                                     |
| `ssh`          | Prefer the `git@host:owner/repo.git` form when cloning.                       |
| `template`     | Path template for this host (see below).                                     |
| `clone_scheme` | `ssh`, `https`, `http`, or `git` — overrides `ssh` and leaves `insteadOf` to git otherwise. |

## Path templates

The default template is `{host}/{owner}/{repo}`. Placeholders:

| Placeholder     | Meaning                                                          |
| --------------- | -------------------------------------------------------------- |
| `{host}`        | Remote host, e.g. `github.com`.                                  |
| `{owner}`       | Owner or organisation.                                           |
| `{group_path}`  | Alias for `{owner}` that reads better for nested GitLab groups.  |
| `{repo}`        | Repository name, without `.git`.                                 |
| `{port}`        | Remote port, when the URL specifies one.                         |

## What is read from git, not config

`fussy-git` never duplicates configuration git already owns. These are read live:

- SSH `Host` aliases from `~/.ssh/config`
- `url.<base>.insteadOf` rewrites from git config
- credential helpers
- commit signing settings

Because every network operation shells out to `git`, that configuration always
applies.

## Hooks

Each hook command runs with the repository as its working directory.

- `post_get` — after a successful `get`
- `post_move` — after `reconcile`/`adopt` moves a repository

This is the place to wire in local setup, editor integration, or
`git maintenance`.
