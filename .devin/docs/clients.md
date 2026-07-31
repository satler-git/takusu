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
  (default `vi`) with a temporary `.toml` file and sends PATCH; with flags it
  sends PATCH directly. Values match CLI flags (`30m`, `1h30m`,
  `2025-06-05 23:59`). Parse/validation errors are inserted as comments at the
  top of the file and the editor reopens for correction. Habits and habit steps
  use the same TOML format; `habit steps set` reads a TOML file.
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
  - Agent: `agent ["text"] [--yes] [--allow <perm>]... [--deny <perm>]...
    [--continue] [--new]`, `agent config {show, set <key> <value>}`,
    `agent allow <key>`, `agent deny <key>`, `agent stats [--clear]`
  - Top level: `tui`, `web [--bind addr]`, `mcp`

### Agent

- `takusu agent` starts an interactive REPL; streaming and session persistence
  are planned to be reworked on top of `ratatui` inline viewport.
- `takusu agent "text"` runs a single turn with `run_turn_stream` and prints
  the response. TTY output streams tokens; non-TTY and `--plain` collect and
  print at the end.
- Approval prompts support `y` (approve), `n` (deny), and `a` (approve and
  promote the needed permission to the session allow-list).
- `takusu agent config show` displays the effective configuration, including
  defaults, and masks `api_key` / `token` values as `<set>`.
- `--continue` resumes the previous CLI session from
  `$XDG_STATE_HOME/takusu/agent-session.json`; `--new` starts fresh.

## takusu-client

Standalone HTTP client library for the takusu REST API. Reused by any future
client (Android Kotlin, etc.).

- Types mirror `takusu-contracts` model.rs request/response structs (`TaskRow`,
  `CreateTask`, `UpdateTask`, etc.)
- `Client` struct holds `base_url` + `token`, all methods are async
- Error type: `ClientError { Http, Api { status, body } }` — no `thiserror`
  dependency
