---
name: issue-assign
description: Claim a GitHub issue for the current user; error + notify on conflict
argument-hint: "<number> [<number>...] [--assignee <user>]"
allowed-tools:
  - exec
  - read
---

Use the `scripts/issue-assign.sh` helper to claim GitHub issues. The helper
first checks that an issue has zero assignees, then adds the assignee. If the
issue is already assigned, it exits non-zero and fires a `dunstify` desktop
notification so the user can intervene — **stop and report the conflict to the
user; do not continue working on the issue**.

This is the first action on any issue handed to you, before reading the body or
exploring the codebase. Multiple agents share one GitHub account, so the
assignee field is the only reliable ownership signal.

## Commands

- `./scripts/issue-assign.sh <number>`
  - Assign issue `<number>` to `@me` (the current authenticated user) only if
    it has no assignees.
- `./scripts/issue-assign.sh <number> --assignee <user>`
  - Assign to a specific user instead of `@me`.
- `./scripts/issue-assign.sh <number1> <number2> ...`
  - Assign multiple issues in one call.

## Output

- In a terminal: human-readable messages.
- When stdout is not a TTY: TSV `number\tassignee(s)\tstatus`, where `status`
  is `assigned` or `already-assigned`. The `already-assigned` line is written
  to **stderr** and the script exits non-zero.

## When to use

- **First** action on any issue handed to you, before anything else.
- To verify-and-assign an issue after the user has asked you to work on it.

## On conflict

If the script exits non-zero with `already-assigned`:
1. Stop. Do not continue working on the issue.
2. Report the conflict and the existing assignee(s) to the user.
3. Ask the user how to proceed (skip, reassign, or take over explicitly).

## Examples

```
./scripts/issue-assign.sh 416
./scripts/issue-assign.sh 413 414 --assignee satler-git
```
