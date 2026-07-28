---
name: jj-resolve
description: Inspect and resolve Jujutsu merge conflicts (list, show markers, edit, mark resolved)
argument-hint: "[list|status|show|edit|mark|abort] [file]"
allowed-tools:
  - exec
  - read
  - edit
---

Use the `scripts/jj-resolve.sh` helper when a `jj rebase`, `jj merge`, or
`jj git pull` leaves the working copy in a conflicted state. Do not run raw
`jj resolve` — the helper gives a stable interface and prints guidance.

## Commands

- `./scripts/jj-resolve.sh list`
  - Lists conflicted file paths (one per line), or `no conflicts`.
- `./scripts/jj-resolve.sh status`
  - One-line summary: `N conflicted file(s)`.
- `./scripts/jj-resolve.sh show [<file>]`
  - Prints conflict marker line numbers for one file (or all conflicted
    files if no argument). This repo uses git-style conflict markers
    (`<<<<<<<`, `|||||||`, `=======`, `>>>>>>>`), configured via
    `ui.conflict-marker-style = "git"` in `.jj/repo/config.toml`. Use this
    to understand the conflict shape before editing.
- `./scripts/jj-resolve.sh edit <file>`
  - Opens `$EDITOR` on the file. After saving, run `mark` to verify the
    conflict is resolved.
- `./scripts/jj-resolve.sh mark <file>`
  - Verifies the file is resolved. jj has no explicit "mark resolved" command
    — a file is considered resolved once all conflict markers are removed.
    This command checks that no markers remain and the file is no longer in
    `jj resolve --list`. Prints how many files remain conflicted.
- `./scripts/jj-resolve.sh merge <file>`
  - Launches a 3-way merge tool via `jj resolve <file>`, then verifies
    resolution. Useful when you'd rather use a visual merge tool than edit
    markers by hand.
- `./scripts/jj-resolve.sh abort`
  - Prints recovery guidance (jj has no universal abort; options are
    `jj abandon @`, `jj op restore <id>`, or `jj undo`).

## When to use

- After any `jj rebase` / `jj merge` / `jj git fetch` that might conflict.
- When `jj st` shows conflicted files.
- Before pushing, to confirm the working copy is clean.

## Conflict resolution workflow

1. `./scripts/jj-resolve.sh list` — see what's conflicted.
2. `./scripts/jj-resolve.sh show <file>` — see where the markers are.
3. Read the file with the `read` tool to see the full conflict context.
4. Use the `edit` tool to resolve the conflict (pick a side or merge both).
   Remove all `<<<<<<<`, `=======`, `>>>>>>>` markers.
5. `./scripts/jj-resolve.sh mark <file>` — verify jj considers it resolved.
   (Alternatively, `./scripts/jj-resolve.sh merge <file>` to use a 3-way
   merge tool.)
6. Repeat until `./scripts/jj-resolve.sh status` reports `0 conflicted file(s)`.
7. Continue the original operation (e.g. `jj describe` then push).

## Examples

```
./scripts/jj-resolve.sh status
./scripts/jj-resolve.sh list
./scripts/jj-resolve.sh show crates/takusu-core/src/lib.rs
./scripts/jj-resolve.sh mark crates/takusu-core/src/lib.rs
```
