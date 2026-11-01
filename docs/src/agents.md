# Using fussy-git with coding agents

Coding agents such as Claude Code read other repositories, work on several
branches at once, and make changes that span services. Without a convention
they clone into `/tmp` or the current project, and put worktrees wherever they
happen to choose. fussy-git gives them one convention, and gives them
machine-readable output to work with.

## The Claude Code plugin

The repository ships a Claude Code plugin with two parts:

- A **skill** that tells Claude how to use fussy-git: `get` instead of
  `git clone`, `info` to orient itself, `worktree add` for parallel work, `exec`
  for multi-repository changes, and plan-then-apply for anything that moves
  files.
- A **`PreToolUse` hook** that blocks `git clone` in Bash and replies with the
  equivalent `fussy-git get` command, keeping `--branch` and `--depth`. Claude
  sees the message and retries correctly. To allow one deliberate clone
  elsewhere, prefix the command with `FUSSY_GIT_ALLOW_CLONE=1`.

Install it from inside Claude Code:

```text
/plugin marketplace add jmsnll/fussy-git
/plugin install fussy-git@fussy-git
```

The hook runs `fussy-git hook pre-tool-use`, so `fussy-git` must be on `PATH`. If
it is missing the hook fails open, and Claude Code carries on.

To wire up the hook without the plugin, add this to `.claude/settings.json`:

```json
{
  "hooks": {
    "PreToolUse": [
      {
        "matcher": "Bash",
        "hooks": [{ "type": "command", "command": "fussy-git hook pre-tool-use" }]
      }
    ]
  }
}
```

## JSON everywhere

`--json` is a global flag. Every command then writes exactly one JSON document
to stdout, and progress still goes to stderr. Failures are JSON too:

```json
{
  "error": {
    "code": "ambiguous",
    "message": "\"api\" matches 2 repositories: …",
    "candidates": [
      { "slug": "github.com/acme/api", "path": "/home/me/git/github.com/acme/api" },
      { "slug": "github.com/acme/api-docs", "path": "/home/me/git/github.com/acme/api-docs" }
    ]
  }
}
```

`code` is `ambiguous`, `no_match` or `error`. An ambiguous query lists its
candidates, so the caller can retry without a second lookup.

## Never hangs, never silently skips

A command that would ask for confirmation (`reconcile --apply`, `remove`) fails
straight away when stdin is not a terminal, and tells you to pass `--yes`.
Without that check, an agent would either hang or see `aborted` with exit `0`
and assume the change had been made.

## Plan, review, apply

For changes that move files, the agent proposes and a person approves:

```sh
fussy-git reconcile --json > plan.json             # dry-run; exits 3 if there is drift
# …the human reviews plan.json…
fussy-git reconcile --apply --plan plan.json       # applies exactly that plan
```

`--apply --plan` re-scans the tree and **refuses if the result differs from the
reviewed plan**, so what was approved is exactly what runs. `remove --dry-run`
reports what would be deleted, and why it might be unsafe, without deleting
anything.

## Running agents at the same time

`get` clones into a hidden sibling directory and renames it into place. Two
agents fetching the same repository at once both end up with the one checkout,
and a failed clone leaves nothing half-written. The index cache is written
atomically. `worktree add` gives each agent its own branch checkout at a
predictable path, and `worktree prune` cleans them up after the work is merged.
