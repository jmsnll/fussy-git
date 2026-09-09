# remove

```text
fussy-git remove <QUERY> [--force] [--yes]
```

Delete one managed repository from disk after checking for unsaved work. The
query must resolve to a single repository (a case-insensitive substring of
`host/owner/repo`).

Before deleting, `remove` checks for blockers — uncommitted changes, unpushed
commits, stashes — and reports them.

| Flag           | Meaning                                                       |
| -------------- | --------------------------------------------------------- |
| `--force`      | Delete despite the safety checks. Still prompts unless `--yes`. |
| `-y`, `--yes`  | Skip the confirmation prompt.                                 |

```sh
fussy-git remove acme/old-service
fussy-git remove acme/old-service --force --yes
```
