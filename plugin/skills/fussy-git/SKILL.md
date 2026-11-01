---
name: fussy-git
description: Use whenever a task needs another git repository's source (a dependency, a sibling service, an upstream project), a separate worktree or branch checkout to work in parallel, or an action across several repositories (status, a search, a scripted change). Covers cloning with `fussy-git get` instead of `git clone`, creating and cleaning up worktrees with `fussy-git worktree`, finding out where you are with `fussy-git info`, and running commands across repositories with `fussy-git exec`. Also use when the user mentions fussy-git, ~/git, or a named set like `@payments`.
---

# fussy-git

This machine keeps every git checkout at a path derived from its remote URL,
`<root>/<host>/<owner>/<repo>` (for example `~/git/github.com/acme/api`), managed
by the `fussy-git` CLI. Follow these rules so checkouts stay findable and nothing
gets cloned twice.

Add `--json` to any command for a single JSON document on stdout. Errors are
JSON too: `{"error": {"code", "message", "candidates"?}}`. Prefer `--json`
whenever you will read the result.

## Getting another repository

Never `git clone` into the current directory or `/tmp`. Use:

```sh
fussy-git get <url | owner/repo | host/owner/repo>   # prints the path
fussy-git get acme/api --json                        # {path, identity, created, …}
```

`get` is idempotent. If the repository is already on disk it returns the
existing path (`"created": false`) without touching it; pass `--update` to
fast-forward it. Read the source at the returned path. Do not copy it into the
project you are working on.

To see what is already on disk: `fussy-git list --json` (filter with a
substring query, `--host`, `--owner`, `--dirty`, `--unpushed`). To resolve a
name to one path: `fussy-git cd <query> --json`. If several repositories match,
the error lists `candidates`; pick one and retry with its slug.

## Where am I?

```sh
fussy-git info --json        # the repository containing the current directory
fussy-git info acme/api --json
```

This returns the identity, the canonical path and whether the repo is in place,
the branch, the default branch, ahead/behind, dirty, stashes, and linked
worktrees. When run inside a worktree, `worktree` is set and `path` is the
owning repository.

## Working in parallel: worktrees

To work on a branch without disturbing the main checkout, or to run several
tasks side by side:

```sh
fussy-git worktree add . feat/login           # `.` = the repo you are in; prints the path
fussy-git worktree add acme/api fix/timeout --base origin/release --json
```

Worktrees go under `<root>/.worktrees/<host>/<owner>/<repo>/<branch>`. Running
`add` again for the same branch returns the existing worktree. A new branch
starts from the default branch with no upstream. Push it with
`git push -u origin <branch>` when you are ready.

When you are done with one, run `fussy-git worktree remove <path>`. This keeps
the branch and refuses if there are uncommitted changes. To clean up after merged
work, run `fussy-git worktree prune` first: it is a dry run that exits 3 if it
found anything. Then run `fussy-git worktree prune --apply`. Merged branches are
deleted with `git branch -d`; dirty or locked worktrees are never touched.

## Across many repositories

```sh
fussy-git status --json [query | @set]            # branch, ahead, behind, dirty, stashes per repo
fussy-git exec [query | @set] --json -- <cmd> …   # run a command in each, in parallel
fussy-git exec @payments --json -- sh -c 'rg -l "OldClient" || true'
```

`exec` runs the command directly (use `sh -c '…'` for pipes). Each run gets
`FUSSY_GIT_REPO` and `FUSSY_GIT_REPO_PATH` in its environment. Each JSON row has
`repo`, `path`, `ok`, `exit_code`, `stdout`, `stderr`. The batch exits 4 if any
repository failed.

`@name` selects a named set from the user's config (`[sets]`).
`fussy-git get @name` clones every member that is missing.

## Changing the tree

These commands move or delete things. Use them only when the user asked:

- `fussy-git reconcile --json > plan.json` is a dry run that writes the plan.
  Show the user the moves, then apply exactly that plan with
  `fussy-git reconcile --apply --plan plan.json`. This refuses if the tree
  changed since the plan was made.
- `fussy-git remove <query> --dry-run --json` lists blockers such as
  uncommitted changes, unpushed commits and stashes. Removing needs `--yes`
  without a terminal. Never pass `--force` unless the user explicitly accepted
  losing that work.

Commands that would prompt fail immediately without a terminal. They never
silently do nothing.

## Exit codes

`0` success, `1` error, `2` usage error, `3` a read-only check found something
(`reconcile`, `doctor`, `worktree prune` dry runs), `4` some repositories in a
batch failed.
