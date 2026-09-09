# How fussy-git compares

`fussy-git` is in the same family as [`ghq`](https://github.com/x-motemen/ghq),
[`projj`](https://github.com/popomore/projj),
[`git-repo-manager`](https://github.com/hakoerber/git-repo-manager), and
[`gita`](https://github.com/nosarthur/gita): tools that organise local clones
and/or drive git across many repositories at once.

## What is different about fussy-git

- **It reconciles an existing tree.** Most tools only guarantee the layout for
  repositories *they* cloned. `fussy-git reconcile` sweeps the root, reads every
  repository's remote, and moves the misplaced ones — so you can adopt it on a
  machine that is already a mess, not just a fresh one.
- **It never duplicates git configuration.** SSH `Host` aliases,
  `url.insteadOf`, credential helpers, and commit signing are read live from git
  and ssh. Every network operation shells out to `git`, so whatever works in
  your normal `git clone` works here. No libgit2, no second copy of your auth
  setup.
- **Dry-run by default, CI-friendly exit codes.** `reconcile` and `doctor`
  change nothing unless asked and exit `3` on drift.
- **Bulk operations built in.** `status`, `pull`, and `fetch` run in parallel
  across every managed repository with a partial-failure exit code — no `xargs`
  wrapper needed.
- **A manifest, not a config tree.** `fussy-git sync` reads a flat `repos.toml`
  list and clones what is missing; the on-disk layout is *derived*, not spelled
  out per repo as in `git-repo-manager`. `dump` generates the manifest from a
  tree you already have. It is the `ghq list | ghq get` workflow with a dry-run,
  an `extra` report, and no stop-on-first-error.
- **GitLab subgroups.** The `{group_path}` placeholder spans arbitrarily nested
  groups.

## When to use something else

- You want a TUI-first, multi-repo dashboard → look at `git-repo-manager` or `gita`.
- You are deep in the `ghq` ecosystem (editor plugins, `peco`/`fzf` recipes,
  `ghq` shell hooks) and happy with it → there is no reason to switch.
- You need Windows support → `fussy-git` currently ships macOS and Linux only.

## Coming from ghq

| ghq                     | fussy-git                        |
| ----------------------- | ------------------------------- |
| `ghq get <repo>`        | `fussy-git get <repo>`           |
| `ghq list`              | `fussy-git list`                 |
| `ghq list --full-path`  | `fussy-git list --path`          |
| `ghq root`              | `fussy-git root`                 |
| `ghq list \| ghq get`   | `fussy-git dump` / `fussy-git sync --apply` |
| *(none)*                | `fussy-git reconcile` / `adopt`  |
| *(none)*                | `fussy-git status` / `pull` / `doctor` |

The default root differs: `ghq` uses `~/ghq` (or `$GOPATH/src`), `fussy-git`
uses `~/git`. Point `root` at your existing `ghq` root in `config.toml` and
`fussy-git` will work with the same tree.
