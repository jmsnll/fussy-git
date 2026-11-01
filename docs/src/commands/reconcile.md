# reconcile

```text
fussy-git reconcile [--apply [--plan <FILE>]] [--yes] [--allow-dirty]
                    [--leave-symlink] [--on-collision fail|suffix|skip]
```

Sweep every root, read each repository's `origin` remote, work out its canonical
path, and move the ones that are misplaced. **Without `--apply` this is a
dry-run** — it prints the plan, changes nothing, and exits `3` if there is
drift so CI can gate on it.

`--apply` performs the moves with a rename (falling back to a copy across
filesystems), prunes directories it empties, and runs `hooks.post_move` in each
moved repository.

| Flag                        | Meaning                                                       |
| --------------------------- | --------------------------------------------------------- |
| `--apply`                   | Perform the moves. Default is a dry-run.                      |
| `--plan <FILE>`           | With `--apply`: apply exactly this reviewed plan (`-` reads stdin). |
| `--yes`                   | Skip the confirmation prompt. Required when stdin is not a terminal. |
| `--allow-dirty`             | Move repositories even with uncommitted changes.             |
| `--leave-symlink`           | Leave a symlink at each old location so hard-coded paths keep working. |
| `--on-collision`            | Override the config policy when two identities map to one path. |

## Typical use

```sh
fussy-git reconcile              # review the plan
fussy-git reconcile --apply      # do it, with a prompt
fussy-git reconcile --apply --yes --leave-symlink
```

## Plan, review, apply

`--json` turns the dry-run into a plan document: the moves, duplicates,
collisions and skips, plus the options the plan was made with. Applying it with
`--plan` re-scans the tree and refuses if anything changed since the review, so
what was approved is exactly what runs. A reviewed plan counts as confirmation,
so there is no prompt.

```sh
fussy-git reconcile --json > plan.json
less plan.json
fussy-git reconcile --apply --plan plan.json
```

This is the safe way for a coding agent to propose a reorganisation: it writes
the plan, a person reads it, and the apply step cannot drift from it.

## In CI

```sh
fussy-git reconcile || echo "tree has drifted"   # exit 3 on drift
```
