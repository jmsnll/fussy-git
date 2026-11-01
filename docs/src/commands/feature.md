# feature

```text
fussy-git feature new <NAME> [@set | repo…] [--branch <B>] [--base <REV>] [--reuse]
                      [--depends <FOLDER=DEP,…>]…
fussy-git feature list
fussy-git feature path [NAME] [MEMBER]
fussy-git feature status [NAME]
fussy-git feature exec [NAME] -- <COMMAND> [ARGS…]
fussy-git feature done <NAME> [--force]
```

One folder per change that spans repositories. Say a change touches an app,
its puppet code, the terraform that deploys it and the terraform modules it
uses. You get one folder with a worktree of each repository, all on the same
branch:

```text
~/git/.features/sso/
  .fussy-feature.toml   manifest: repositories, branch, bases, dependencies
  CLAUDE.md             generated from the manifest; facts only
  NOTES.md              yours; never rewritten
  api/                  worktree of github.com/acme/api on branch sso
  puppet-control/
  terraform/
  tf-modules/
```

That folder is the feature's whole context, for you, your editor, or a coding
agent started in it (`cd "$(fussy-git feature path sso)" && claude`). Your main
clones stay on their own branches the whole time. Single-repository worktrees
are better handled by [Worktrunk](../worktrunk.md); `feature` covers the
cross-repository case, which Worktrunk doesn't.

It is **not isolation**. Every worktree writes its commits into its main
clone's `.git`, so anything that can commit in `api/` can also change other
branches of that repository.

## new

```sh
fussy-git feature new sso @payments --depends terraform=tf-modules --depends api=terraform
```

1. Expands `@set`s and clones any repository that isn't on disk yet.
2. Fetches each repository, so new branches start from the remote's current
   default branch.
3. Checks every repository **before creating anything**. If any of them has a
   problem, nothing is created and each problem is reported with a stable code:

   | Code                           | Meaning                                                    |
   | ------------------------------ | ---------------------------------------------------------- |
   | `branch_checked_out_elsewhere` | The branch is checked out somewhere else (often the main clone). `other_path` says where. |
   | `branch_exists`                | A local branch of that name exists. Pass `--reuse` to check it out. |
   | `path_exists`                  | The member folder exists but isn't a worktree on the branch. |
   | `bad_base`                     | `--base` doesn't exist in that repository.                  |
   | `clone_failed`                 | The repository couldn't be cloned.                          |
   | `not_planned`                  | Skipped because another repository had a problem.           |

4. Creates each worktree: it tracks `origin/<branch>` if that exists,
   otherwise it starts a new branch from `--base` or the repository's default
   branch, with no upstream. Each worktree is locked with
   `git worktree lock`, so other tools leave it alone.
5. Writes `CLAUDE.md`, and creates `NOTES.md` if it doesn't exist.

Running `new` again **resumes or extends** the feature: existing members are
left alone, missing ones are recreated, and any extra repositories you name
are added. `--depends` entries are merged into the manifest, and you can also
edit `depends` in `.fussy-feature.toml` directly.

The branch is the feature name unless you pass `--branch`. Feature names are
lowercase letters, digits, `_`, `-` and `.`.

If your tooling restricts branch names, set `feature_branch_pattern` in the
config, and `new` will refuse a branch that doesn't match, before creating
anything. For example, Puppet r10k turns each control-repo branch into an
environment, and environment names may only use `a-z`, `0-9` and `_`:

```toml
feature_branch_pattern = "^[a-z0-9_]+$"
```

## status

Branch, upstream, ahead/behind, dirty and **unpushed** for each repository,
**listed so dependencies come first**. That is the order to push and open pull
requests in. "Unpushed" counts commits that are on no `origin` branch, so a
new branch with commits but no upstream counts them all. With no name, it uses
the feature containing the current directory.

## exec

```sh
fussy-git feature exec -- git push -u origin HEAD
fussy-git feature exec sso --json -- git log --oneline origin/HEAD..
```

Like [`exec`](./exec.md), across the feature's worktrees, dependencies first.

## done

Removes every worktree with plain `git worktree remove` (never forced), keeps
every branch, and moves the folder (manifest, CLAUDE.md, NOTES.md) to
`~/git/.features/.archive/<name>-<timestamp>`.

It refuses, changing nothing, if any repository has:

- uncommitted or untracked changes, whatever flags you pass;
- unpushed commits, unless you pass `--force` (for example after a
  squash-merge);
- a different branch checked out.

## Elsewhere

- **Other commands:** `list`, `cd` and `fzg` show the main clones, never feature
  worktrees. [`worktree list`](./worktree.md) shows each worktree with its
  `feature`, and `fussy-git cd api@sso` jumps to it.
- **`reconcile` and `remove`** won't touch a repository while it belongs to a
  feature, even with `--force`.
- **`doctor`** reports a member folder that was deleted by hand. Run
  `feature new <name>` again to recreate it.

## Configuration

| Key                      | Default            | Meaning                                                  |
| ------------------------ | ------------------ | -------------------------------------------------------- |
| `features_root`          | `<root>/.features` | Where features live.                                     |
| `feature_branch_pattern` | unset              | A regular expression that every feature branch must match. |
