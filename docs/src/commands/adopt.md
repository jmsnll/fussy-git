# adopt

```text
fussy-git adopt [PATH] [--as <IDENTITY>] [--allow-dirty] [--leave-symlink]
```

Bring a single existing checkout under management by moving it to its canonical
path. `PATH` defaults to the current directory. Prints the new path.

Where `reconcile` works on the whole tree, `adopt` is the one-repository
version — useful right after you clone something by hand into the wrong place.

| Flag              | Meaning                                                             |
| ----------------- | --------------------------------------------------------------- |
| `--as <IDENTITY>` | Force the target identity when the repo has no usable remote: `host/owner/repo`, `alias:owner/repo`, or a URL. An `origin` is added from it. |
| `--allow-dirty`   | Adopt even with uncommitted changes.                                |
| `--leave-symlink` | Leave a symlink at the old location.                                |

```sh
cd ~/Downloads/some-clone
fussy-git adopt

fussy-git adopt ./vendored-thing --as github.com/acme/vendored-thing
```

The index is refreshed afterwards so `list` reflects the move immediately.
