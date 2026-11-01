# remove

```text
fussy-git remove <QUERY> [--force] [--yes] [--dry-run]
```

Delete one managed repository from disk after checking for unsaved work. The
query must resolve to a single repository (a case-insensitive substring of
`host/owner/repo`).

Before deleting, `remove` checks for blockers — uncommitted changes, unpushed
commits, stashes, linked worktrees — and reports them. With blockers and no
`--force`, it refuses before asking anything.

| Flag           | Meaning                                                       |
| -------------- | --------------------------------------------------------- |
| `--force`      | Delete despite the safety checks. Still prompts unless `--yes`. |
| `-y`, `--yes`  | Skip the confirmation prompt. Required when stdin is not a terminal. |
| `--dry-run`    | Report what would be removed, and any blockers, then stop.    |

```sh
fussy-git remove acme/old-service
fussy-git remove acme/old-service --force --yes
```
