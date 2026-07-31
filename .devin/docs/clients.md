# CLI and HTTP Client

## takusu-cli

CLI client using clap derive with verb-first top-level commands. Task and
schedule operations are flat top-level verbs; lower-frequency domains are
`noun`-grouped.

- **Uses takusu-local-lib directly**: no network round-trip
- **Storage backends**: `TAKUSU_STORAGE=sqlite` (default) or
  `TAKUSU_STORAGE=workers`
- **Display modes**: `--mode rich` or `--mode simple`; auto-detected from TTY
  (`--plain` forces simple)
- **Status display**: colored in rich mode (Yellow=pending, Green=scheduled,
  DarkYellow=in_progress, DarkCyan=completed, DarkGrey=skipped); simple mode
  uses markers ([ ], [~], [>], [x], [-])
- **Status update**: `start`, `pause`, `done`, `skip` verbs; `edit --status`
  for other status transitions
- **Task list filter**: `ls [query...] [--status <value>] [--all] [--no-overdue]
  [--habit-id <id>] [--ical-uid <uid>]` (default shows actionable tasks)
- **Editor-based editing**: `edit <REF>` with no flags opens `$EDITOR`
  (default `vi`) and sends PATCH; with flags it sends PATCH directly.
- **Subcommands**:
  - Task verbs: `add <title>`, `ls [query...]`, `show <ref>`, `start <ref>`,
    `pause <ref>`, `done <ref>`, `skip <ref>`, `edit <ref> [--flags]`,
    `rm <ref>`, `progress <ref> <quantity>`, `split <ref> --keep <quantity>`,
    `import <file.ics|->`, `deps [--check]`
  - Schedule verbs: `agenda [--day <date>]`, `plan [--from <dt>] [--until <dt>]
    [--tasks ref...] [--pin ref...] [--sleep s]`, `move <ref> <start_at>`,
    `unplan`
  - Default with no subcommand: **`agenda`**. This replaces the previous
    `takusu` → TUI behavior and is a breaking change for existing users.
  - Noun groups: `habit {add, ls, show, edit, rm, pause, pauses {ls, rm},
    steps {ls, edit, set, check}}`, `memory {add, ls, show, edit, rm,
    search <q>, similar <title>}`, `skill {add, ls, show, edit, rm}`,
    `token {add, ls, rm}`, `sync {status, setup, login, run, mappings, purge}`,
    `config {show, init, set <key> <value>, workers {set <url> <token>,
    health}}`, `system {health, gen-root-token, license, completion <shell>}`
  - Top level: `tui`, `web [--bind addr]`, `mcp`

## takusu-client

Standalone HTTP client library for the takusu REST API. Reused by any future
client (Android Kotlin, etc.).

- Types mirror `takusu-contracts` model.rs request/response structs (`TaskRow`,
  `CreateTask`, `UpdateTask`, etc.)
- `Client` struct holds `base_url` + `token`, all methods are async
- Error type: `ClientError { Http, Api { status, body } }` — no `thiserror`
  dependency
