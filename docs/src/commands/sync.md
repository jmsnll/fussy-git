# sync & dump

```text
fussy-git sync [--apply] [--manifest <PATH>] [--jobs <N>]
fussy-git dump [> repos.toml]
```

A **manifest** is a file that lists the repositories a machine should have —
what a `Brewfile` is to Homebrew, or `.mrconfig` to `mr`. Commit it to your
dotfiles and `fussy-git sync --apply` reproduces your working set on a new
machine.

## The manifest

A file named `repos.toml`, discovered the same way as `config.toml`:

1. `--manifest <path>`
2. `$FUSSY_GIT_MANIFEST`
3. `repos.toml` in the current directory or any ancestor
4. `~/.config/fussy-git/repos.toml`

It is a flat list of targets in any form [`get`](./get.md) accepts:

```toml
repos = [
  "jmsnll/fussy-git",                        # owner/repo, default host
  "github.com/rust-lang/rust",               # host/owner/repo
  "gl:acme/backend/service",                 # host alias
  "git@gitlab.com:acme/backend/api.git",     # any URL form
]
```

The manifest never carries executable content — lifecycle hooks stay in the
machine-local `config.toml`, so cloning someone's dotfiles and running `sync`
cannot run their code.

## `fussy-git sync`

**Without `--apply` this is a dry-run** — it reports and changes nothing, and
exits `3` when the tree does not match so CI can gate on it.

```text
sync  2 missing, 1 extra, 3 in place

  missing    github.com/rust-lang/rust
  missing    gitlab.com/acme/backend/service
  extra      github.com/jmsnll/old-experiment   not in manifest

run with --apply to clone the missing repos
```

| Verdict     | Meaning                                                                   |
| ----------- | ------------------------------------------------------------------------ |
| `missing`   | In the manifest, not on disk. `--apply` clones it into its canonical path. |
| `extra`     | On disk, not in the manifest. Reported only — never removed.               |
| `misplaced` | Manifested, on disk, but in the wrong place. Run [`reconcile`](./reconcile.md). |
| `error`     | A manifest entry that does not resolve to a repository.                    |

`--apply` clones the missing repositories in parallel (`--jobs` to bound the
concurrency, default from `config.jobs`). A failed clone does not stop the
others; the command exits `4` on partial failure. `post_get` hooks run per
freshly cloned repository, exactly as `get` does.

`sync` reads every root when deciding `extra` / `in place`, but clones only into
the primary `root`, like `get` and `reconcile`.

## `fussy-git dump`

Walks the managed tree and writes a manifest to stdout — the adoption path:

```sh
fussy-git dump > ~/.config/fussy-git/repos.toml   # on the machine you have
# commit it, then on every other machine:
fussy-git sync --apply
```

Output is sorted, one entry per line, using the shortest unambiguous target
form, so re-running produces a minimal diff. It records repository identities
only, not the checked-out branch or extra remotes.

## Together with `reconcile`

`reconcile` corrects *layout* drift (a repo in the wrong place); `sync` corrects
*set* drift (a repo that should or should not be there at all). Running
`reconcile --apply` then `sync --apply` converges the whole tree.
