# Using with Worktrunk

[Worktrunk](https://worktrunk.dev) (`wt`) manages git worktrees for parallel
work, and is built with coding agents in mind. fussy-git and Worktrunk split
the job:

| fussy-git: **repositories**                            | Worktrunk: **branches inside one repository**       |
| ------------------------------------------------------ | --------------------------------------------------- |
| Where every clone lives (`get`, path templates)        | Creating and switching worktrees (`wt switch -c`)   |
| Fixing a messy tree (`reconcile`, `adopt`, `doctor`)    | Merging and cleaning up (`wt merge`, `wt remove`)   |
| Acting across repositories (`status`, `exec`, `@sets`) | Per-worktree setup hooks, launching agents (`-x`)   |
| Finding things (`list`, `cd`, `fzg`, `info`)            | Per-repository status (`wt list`)                   |

## Layout

Leave Worktrunk on its default `worktree-path`. It puts each worktree next to
its repository, inside fussy-git's root:

```text
~/git/github.com/acme/api              ← fussy-git clone
~/git/github.com/acme/api.feat-auth    ← wt worktree
```

Everything for one repository sorts together, and fussy-git treats
`api.feat-auth` as a worktree of `api`, not as a separate repository.

## A typical day

```sh
fussy-git get acme/api                 # clone into ~/git/github.com/acme/api
fzg api                                 # jump to it
wt switch -c -x claude feat/auth       # new worktree + branch, start Claude in it
fzg api@feat-auth                       # jump back later, from anywhere
fussy-git worktree list @payments      # every worktree across a set of repos
wt merge                               # when it's done
```

For Worktrunk's richer status across many repositories, run it through
`exec`:

```sh
fussy-git exec @payments --json -- wt list --format json
```

## With Claude Code

Install both plugins. They don't overlap:

- **Worktrunk's plugin** handles the worktrees Claude Code creates
  (`claude --worktree`, subagent isolation), routing them through `wt`.
- **fussy-git's plugin** stops Claude from cloning other repositories into
  random places, by sending `git clone` to `fussy-git get`. Its skill tells
  Claude to use `wt` for branches when it is installed.

fussy-git deliberately doesn't install a `WorktreeCreate` hook, so it never
competes with Worktrunk's.

## Changes across several repositories

Worktrunk handles one repository at a time. When a change spans several
(an app, its puppet code, terraform and terraform modules), use
[`fussy-git feature`](./commands/feature.md). It gives you one folder with a
worktree of each repository on the same branch. `wt list` and `wt merge` still
work inside each of those worktrees.

## Moving repositories

`reconcile` won't move a repository that has linked worktrees, because the
worktrees record absolute paths back to it. Remove them first (`wt remove`),
or leave that repository where it is for now. Moving a repository together
with its worktrees is planned.
