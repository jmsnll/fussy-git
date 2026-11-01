---
name: fussy-git
description: Use whenever a task needs another git repository's source (a dependency, a sibling service, an upstream project), finding an existing checkout or worktree, a change that spans several repositories (app, puppet, terraform, terraform modules…), or an action across several repositories (status, a search, a scripted change). Covers cloning with `fussy-git get` instead of `git clone`, finding repositories and worktrees, finding out where you are with `fussy-git info`, and running commands across repositories with `fussy-git exec`. Also use when the user mentions fussy-git, ~/git, or a named set like `@payments`.
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

fussy-git does not create worktrees. To work on a branch without disturbing the
main checkout, or to run tasks side by side:

- If `wt` (Worktrunk) is installed (`command -v wt`), use it:
  `wt switch -c <branch>` creates a worktree, `wt remove` deletes it.
- Otherwise use `claude --worktree` or `git worktree add`.

To find worktrees that already exist, whichever tool made them:

```sh
fussy-git worktree list --json [query | @set]   # {repo, path, branch, dirty, missing, …}
fussy-git cd acme/api@feat-auth --json          # the checkout of that branch
```

`/` and `-` are interchangeable in `repo@branch`. If the branch isn't checked
out anywhere, the error lists the branches that are.

## A change that spans several repositories

When one change touches several repositories, work in a feature folder: one
worktree of each repository, all on the same branch.

```sh
fussy-git feature new <name> @set [repo…] --depends terraform=tf-modules --json
cd "$(fussy-git feature path <name>)"
```

- Start with `fussy-git feature status --json`. It lists the members in
  dependency order, each with branch, upstream, ahead/behind, dirty and
  unpushed.
- **One writer per worktree.** Each member folder is its own repository.
  Commit in each one separately.
- **Push and open pull requests in dependency order.** That is the order
  `status` lists, and it puts modules before their consumers. Link the pull
  requests to each other in their descriptions.
- **Cross-repository references are pinned.** Examples: a terraform
  `source = "git::…?ref=v1.2"`, or a Puppetfile entry pinned by `:ref`. A
  consumer does not see its sibling's branch until you point that reference at
  the branch (for example `?ref=<branch>`). Before merging, point it back at a
  released tag. Never commit a local `../` module source.
- **Never** run `terraform apply` or a puppet deploy. Never switch branches or
  create worktrees inside the feature folder. Never edit the main clones.
- **To add a repository**, run `feature new <name> <repo>` again. It is
  idempotent and extends the feature.
- **If `new` reports errors**, each one has a `code`, for example
  `branch_checked_out_elsewhere`, which comes with `other_path`. Nothing was
  created; fix the cause and run it again.
- **When finished**, run `fussy-git feature done <name>`. It refuses while
  anything is uncommitted or unpushed, and that refusal is the safety check:
  don't pass `--force` unless the user confirms the branches were
  squash-merged.

Feature worktrees are not isolated. Commits land in each main clone's `.git`.

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
(`reconcile` and `doctor`), `4` some repositories in a
batch failed.
