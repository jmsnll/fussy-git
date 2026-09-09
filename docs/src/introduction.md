# fussy-git

`fussy-git` keeps your cloned git repositories organised under a single root,
with each repository's path derived from its remote URL:

```text
~/git/github.com/jmsnll/fussy-git
~/git/gitlab.com/acme/backend/service
```

It clones new repositories straight into the right place, and it can
**reconcile an existing tree**: sweep the root, read every repository's remote,
and move the misplaced ones to where they belong.

## Why

If you clone repositories by hand you end up with `~/code`, `~/projects`,
`~/src`, and a dozen half-remembered folder names. Tools like
[`ghq`](./comparison.md) fix this for *new* clones by imposing a canonical
layout. `fussy-git` does that too, and additionally treats the root as **state
to be reconciled** — it will fix a tree that is already a mess, not just keep a
clean one clean.

## Design

- **git owns git's configuration.** SSH `Host` aliases, `url.insteadOf`,
  credential helpers, and commit signing are read live from git and ssh config,
  never duplicated. Every network and authentication operation shells out to the
  `git` binary, so that configuration always applies. `fussy-git` never links
  libgit2.
- **Dry-run by default.** `reconcile` and `doctor` change nothing unless you
  ask; read-only checks exit non-zero when they find drift, so CI can gate on
  them.
- **Composable output.** stdout carries data (paths, tables, JSON); stderr
  carries progress. `cd "$(fussy-git get …)"` and `fussy-git list --path | fzf`
  work without filtering.

## Next steps

- [Install it](./installation.md)
- [Getting started](./getting-started.md)
- [Configure the layout](./configuration.md)
