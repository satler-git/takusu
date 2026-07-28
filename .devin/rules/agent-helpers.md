# Agent Helpers

This file describes notifications, helper scripts, and skill invocation rules
for agents. It is referenced from [AGENTS.md](../AGENTS.md).

## Agent Notifications

When a task is finished or when you have a question for the user, send a desktop
notification via `dunstify` so the user is alerted even when not watching the
terminal.

```sh
dunstify "takusu agent" "task finished: <short summary>"
dunstify "takusu agent" "question: <short question>"
```

- Fire the notification **once** at the very end of the task, or when you need
  user input to proceed.
- Keep the body short (one line). Do not dump long output into the notification.
- Use a meaningful summary: what was done, or what you need from the user.

## Helper Scripts

Six shell scripts in `scripts/` wrap common agent workflows. Each has a matching
thin Devin skill in `.devin/skills/<name>/SKILL.md` so the agent can invoke
them via `/issue-view`, `/issue-assign`, `/pr-watch`, `/jj-resolve`,
`/discord-notify`, `/profile`, or `/optimize`. **Prefer the scripts over raw
`gh`/`jj`/`curl`** — they produce stable, agent-friendly output and centralize
flag spelling.

### `issue-view.sh` — GitHub issue viewer

Wraps `gh issue list` / `gh issue view`. Plain-text output (TSV for `list`,
markdown for `show`) so the agent can parse it without TTY-dependent color
codes.

```sh
./scripts/issue-view.sh list [--label <label>] [--assignee <user|@me|unassigned>] \
                             [--state <open|closed|all>] [--limit <N>] [--search <query>]
./scripts/issue-view.sh show <number>
```

- `list` output: `number\ttitle\tlabels\tassignees\tstate` (one issue per line).
- `show` output: title, labels, assignees, body, then the full comment thread.
- Use `--assignee unassigned` to find untriaged issues.

### `issue-assign.sh` — GitHub issue self-assignment

Claims an issue for the current user (or another user) only if it has zero
assignees. **Call this first** on any issue handed to you, before reading the
body or exploring the codebase.

```sh
./scripts/issue-assign.sh <number> [<number>...] [--assignee <user>]
```

- Output (non-TTY): `number\tassignee(s)\tstatus`, where status is `assigned` or
  `already-assigned`.
- On `already-assigned`: exits non-zero, writes the TSV line to **stderr**,
  and fires a `dunstify` desktop notification. **Stop and report the conflict
  to the user; do not continue working on the issue.** Multiple agents share
  one GitHub account, so the assignee field is the only reliable ownership
  signal.

### `pr-watch.sh` — PR CI/review/comment watcher

Wraps `gh pr view --json ...` and presents a stable snapshot of CI checks,
reviews, and comments. Two modes:

```sh
./scripts/pr-watch.sh show <PR>                       # one-shot snapshot
./scripts/pr-watch.sh watch <PR> [--interval 60] [--max 0]  # polling loop
```

- `show` prints the full snapshot once.
- `watch` loops, printing only sections that changed since the last snapshot
  (`--- <section> changed ---` with `<<< before` / `>>> after`). Default
  interval 60s; `--max 0` = unlimited.
- **No desktop/Discord notification** — output goes to stdout only. The agent
  reads stdout and decides what to do (e.g. reply to a review comment, or
  report CI failure to the user).
- Run `watch` in a background shell and poll with `get_output` to integrate with
  the agent loop.

### `jj-resolve.sh` — Jujutsu conflict resolver

Wraps `jj resolve --list` and friends. Use after any `jj rebase` / `jj merge`
/ `jj git fetch` that might conflict.

```sh
./scripts/jj-resolve.sh list          # conflicted file paths (or "no conflicts")
./scripts/jj-resolve.sh status        # "N conflicted file(s)"
./scripts/jj-resolve.sh show [<file>] # conflict marker line numbers
./scripts/jj-resolve.sh edit <file>   # open $EDITOR on the file
./scripts/jj-resolve.sh mark <file>   # verify the file is resolved (no markers + not in jj resolve --list)
./scripts/jj-resolve.sh merge <file>  # launch a 3-way merge tool via jj resolve
./scripts/jj-resolve.sh abort         # print recovery guidance
```

The standard agent workflow is: `list` → `show <file>` → read the file →
`edit` (or use the `edit` tool) to remove conflict markers → `mark <file>`
to verify → repeat until `status` reports 0. jj has no explicit "mark
resolved" command — a file is considered resolved once all conflict markers are
removed, so `mark` just verifies that condition. Use `merge` if you prefer a
3-way merge tool over manual marker editing.

### `profile.sh` — Perf flamegraph + top-function summary

Profiles a Rust example or binary under `perf`, then emits an SVG flamegraph
and sorted `top-self` / `top-total` summaries. Useful because raw `perf report`
output is hard to read for Rust (inlined generic symbols and deep rayon stacks).

```sh
./scripts/profile.sh --example profile -p takusu-core
./scripts/profile.sh --bin takusu-local -p takusu-local
./scripts/profile.sh --example daily -p takusu-core -- --some-arg
```

- Builds with frame pointers + debug info, runs `perf record -e cycles:u -g`,
  then converts with `inferno-collapse-perf` / `inferno-flamegraph`.
- Outputs in `target/profile/` by default (`-o <dir>` to override):
  `flamegraph.svg`, `top.txt`, `top-self.txt`, `top-total.txt`, `collapsed.txt`.
- Requires `perf` and `inferno`; auto-pulls from nixpkgs via `nix` if missing.

### `discord-notify.sh` — Discord webhook sender

Sends a message or embed to Discord via webhook. The URL is read from
`$DISCORD_WEBHOOK_URL` (set in `.envrc` or shell config); the script never
prints it.

```sh
./scripts/discord-notify.sh "plain text"
./scripts/discord-notify.sh --title "T" --desc "D" [--color 0xRRGGBB|#RRGGBB|RRGGBB|decimal] [--url <link>]
./scripts/discord-notify.sh --json <payload.json>
echo '{"content":"hi"}' | ./scripts/discord-notify.sh --stdin
```

- `--color` accepts `0xRRGGBB`, `#RRGGBB`, `RRGGBB`, or a decimal integer.
- `--quiet` suppresses the `discord: sent` confirmation.
- This is **separate from the `dunstify` desktop notifications** in
  "Agent Notifications" above — `dunstify` is for local terminal alerts,
  `discord-notify.sh` is for off-terminal pings.

## Skill invocation

Each helper has a Devin skill in `.devin/skills/<name>/SKILL.md` that
documents the script and tells the agent when to use it. Skills are picked up
at session start; restart the session after adding a new one. The skill files
are thin (just documentation + `allowed-tools: [exec, read]`) — the real
logic lives in the shell scripts so it can be used outside Devin too.
