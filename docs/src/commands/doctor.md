# doctor

```text
fussy-git doctor [--stale <DURATION>]
```

A read-only health report for the managed tree. Changes nothing; exits `3` when
it finds something worth acting on so it can run in CI or a shell prompt.

It reports repositories that are:

- **misplaced** — not at their canonical path (fix with [`reconcile`](./reconcile.md))
- **duplicate** — the same remote checked out in two places
- **broken** — no readable `.git`, or no usable remote
- **detached** — `HEAD` not on a branch
- **stale** — last commit older than `--stale`, when given

| Flag       | Meaning                                                       |
| ---------- | --------------------------------------------------------- |
| `--stale`  | Duration threshold for the staleness check, e.g. `90d`, `6mo`. |

```sh
fussy-git doctor
fussy-git doctor --stale 6mo
```
