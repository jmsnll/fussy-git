# worktree

```text
fussy-git worktree list [QUERY | @set]
fussy-git cd <QUERY>@<BRANCH>
```

fussy-git decides where *repositories* live. Creating, switching, merging and
removing worktrees within one repository is left to tools built for it:
[Worktrunk](../worktrunk.md) (`wt`), Claude Code's `--worktree`, or plain
`git worktree`. fussy-git adds the view across every repository, whichever tool
made the worktrees.

Feature workspaces are planned for a later release: one folder holding a
worktree of each repository involved in a cross-repository change, for example
an app plus its puppet and terraform repositories.

## list

Every linked worktree of your repositories, wherever it is on disk, with its
branch and state: `missing`, `dirty`, `locked`. `QUERY` and `@set` filter
repositories the same way as [`status`](./bulk.md). With `--json`, each element
is `{repo, repo_path, path, branch, head, locked, missing, dirty}`.

## Jumping to a branch

`fussy-git cd api@feat-auth` prints the checkout that has `feat-auth` checked
out, and the `fg` shell function uses it, so `fg api@feat-auth` takes you
there. Other cases:

- `api@main` is the main clone when `main` is checked out there.
- `/` and `-` are interchangeable, so `api@feat-auth` finds the branch
  `feat/auth`, matching how Worktrunk names its directories.
- If no checkout has the branch, the error lists the branches that are checked
  out. It never creates a worktree: use `wt switch -c <branch>` for that.

## Worktrees inside the root

Worktrunk's default layout puts worktrees beside the repository, for example
`~/git/github.com/acme/api.feat-auth`. fussy-git recognises these as worktrees
of `api`, not as repositories of their own. `list`, `cd` and `fg` don't offer
them, and `reconcile` and `doctor` don't count them.

[`doctor`](./doctor.md) reports **broken worktrees**, meaning ones whose
directory was deleted by hand. Clean them up with `wt remove` or
`git worktree prune`.

`reconcile` and `remove` won't touch a repository that has linked worktrees,
because moving or deleting it would break them, and `remove --force` doesn't
override this. Remove the worktrees first.

## Removed in 0.3.0

`worktree add`, `worktree prune` and `worktree remove`, along with the
`worktree_root` setting and the `post_worktree` hook, were removed in 0.3.0. The
commands now exit with a pointer to the replacement, and the old config keys
are still accepted, with a warning.
