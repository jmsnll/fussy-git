# info

```text
fussy-git info [QUERY | PATH]
fussy-git here
```

Everything about one repository in one call: identity, path, whether it sits at
its canonical path, remotes, branch, default branch, upstream with ahead/behind,
dirty, stashes, and its linked worktrees.

With no argument (or `here`), it describes the repository containing the current
directory. That is the first thing to ask in an unfamiliar checkout, and the
first thing an agent needs at the start of a session. From inside a linked
worktree, the branch and working-tree fields describe that worktree, and
`worktree` names it. Everything else describes the repository that owns it.

```sh
fussy-git info
fussy-git info acme/api --json | jq .default_branch
```
