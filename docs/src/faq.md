# FAQ

## Does it work with my existing `ghq` / `~/code` tree?

Yes. Point `root` at that directory in `config.toml` and run
`fussy-git reconcile` to see what it would change. Nothing moves until you pass
`--apply`.

## Does it need a GitHub token / API access?

No. `fussy-git` never talks to a forge API. It reads remote URLs from git and
shells out to the `git` binary for anything that touches the network, so it uses
exactly the auth you have already configured.

## What about private hosts, self-hosted GitLab, Gitea?

Add a `[hosts."<host>"]` block with a `template` (and `alias` if you want a
shorthand). `{group_path}` handles nested GitLab subgroups.

## Will it delete anything?

Only `remove`, and only after checking for uncommitted changes, unpushed
commits, and stashes — and it prompts unless you pass `--yes`. `reconcile`
moves directories with a rename; it never deletes a repository.

## Why not libgit2?

So that your `~/.gitconfig`, `~/.ssh/config`, credential helpers, and signing
setup always apply without `fussy-git` reimplementing or mirroring them.

## Windows?

Not currently. The reconcile/adopt symlink fallback uses Unix APIs. macOS and
Linux (x86-64 and arm64) are supported.

## How do I change the layout?

Set `default_template`, or a per-host `template`. Placeholders: `{host}`,
`{owner}`, `{group_path}`, `{repo}`, `{port}`. See
[Configuration](./configuration.md).

## Where does the config file live?

`~/.config/fussy-git/config.toml`, or `.fussy-git.toml` in the working directory
or an ancestor, or `$FUSSY_GIT_CONFIG`.
