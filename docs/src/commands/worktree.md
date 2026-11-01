# worktree

```text
fussy-git worktree add <REPO> <BRANCH> [--base <REV>]
fussy-git worktree list [QUERY]
fussy-git worktree prune [QUERY] [--apply] [--all]
fussy-git worktree remove [PATH] [--force]
```

Linked worktrees at predictable paths. `fussy-git` already derives every clone's
path from its identity, and worktrees get the same treatment:

```text
<worktree_root>/<host>/<owner>/<repo>/<branch>
~/git/.worktrees/github.com/acme/api/feat-login
```

`worktree_root` defaults to `<root>/.worktrees`. It is a dot-directory, so
`scan` never mistakes a worktree for a stray clone. A `/` in a branch name
becomes `-`. This is the command to use when several sessions, or several
coding agents, need to work on one repository at the same time.

## add

Create a worktree for `BRANCH` and print its path.

- `REPO` is a query (`acme/api`) or a path. Use `.` for the repository you are
  in; from inside a worktree, `.` still means the owning repository.
- An existing local branch is checked out. A branch that exists only on
  `origin` is created tracking it. Any other name becomes a new branch, started
  from `--base` or the default branch (`origin/HEAD`, falling back to `main` or
  `master`), with **no upstream**, so `git push -u` sets one when you are ready.
- Running it again for the same branch returns the existing worktree, wherever
  it is.
- `hooks.post_worktree` runs in each new worktree, so it is the place for
  `npm ci` or copying a `.env`.

```sh
cd "$(fussy-git worktree add . feat/login)"
fussy-git worktree add acme/api hotfix --base origin/release --json
```

## list

Every linked worktree across your repositories, with its branch and state:
`missing`, `dirty`, `merged`, `upstream gone`, `locked`. `QUERY` and `@set`
filter repositories the same way as [`status`](./bulk.md).

## prune

Clean up what parallel work leaves behind. A worktree is a candidate when:

| Reason            | Meaning                                                            |
| ----------------- | ------------------------------------------------------------------ |
| directory missing | It was deleted by hand; only git's bookkeeping remains.             |
| branch merged     | The branch has commits of its own, and all of them are in the default branch (on the remote or locally). |
| upstream deleted  | The branch's upstream was deleted on the remote, which usually means its pull request was merged. |

A worktree with uncommitted changes, or a locked one, is listed as protected and
never removed. A branch that was created but never committed to is not treated
as merged, even though it is an ancestor of the default branch.

Like [`reconcile`](./reconcile.md), **prune is a dry run unless you pass
`--apply`**, and it exits `3` when it finds candidates. Applying removes each
worktree with `git worktree remove`, which refuses dirty trees. After that it
deletes *merged* branches with `git branch -d`. An upstream-gone branch is kept,
because a squash-merged or abandoned branch may hold commits nothing else has.

By default only worktrees under `worktree_root` are considered (missing ones are
always considered). `--all` includes worktrees made elsewhere.

## remove

Remove the worktree containing `PATH` (default: the current directory). The
branch is kept. git refuses if there are uncommitted changes, unless you pass
`--force`.
