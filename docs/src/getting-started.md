# Getting started

`fussy-git` works with no configuration at all: root `~/git`, layout
`{host}/{owner}/{repo}`, and `github.com` as the host for bare `owner/repo`
shorthand.

## Clone into the canonical path

```sh
fussy-git get github.com/jmsnll/fussy-git
fussy-git get jmsnll/fussy-git                 # github.com is the default host
fussy-git get git@github.com:jmsnll/fussy-git.git   # any URL form works
```

`get` prints the path it cloned into, so you can jump straight there:

```sh
cd "$(fussy-git get jmsnll/fussy-git)"
```

Running it again is a no-op rather than an error, which makes it safe to put in
setup scripts.

## Fix a tree that already exists

```sh
fussy-git reconcile           # dry-run: prints the plan, changes nothing
fussy-git reconcile --apply   # perform the moves
```

`reconcile` sweeps every root, reads each repository's `origin` remote, works
out where it *should* live, and moves the ones that are in the wrong place. It
prunes directories it empties and runs your `post_move` hooks.

## Work across every repository

```sh
fussy-git status              # working-tree status, one line per repo
fussy-git pull                # fast-forward pull everywhere
fussy-git list --dirty        # just the repos with uncommitted changes
```

## Jump to a repository

```sh
eval "$(fussy-git shell-init zsh)"   # adds an `fg` function
fg fussy                             # cd to the repo matching "fussy"
```

See [shell integration](./commands/shell.md) for bash and fish.
