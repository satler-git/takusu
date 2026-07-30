# CLI and HTTP Client

## takusu-cli

CLI client using clap derive with nested subcommands: `task`, `schedule`,
`token`, `sync`.

- **Uses takusu-local-lib directly**: no network round-trip
- **Storage backends**: `TAKUSU_STORAGE=sqlite` (default) or
  `TAKUSU_STORAGE=workers`
- **Display modes**: `--mode rich` (comfy-table) / `--mode simple` (plain text)
- **Status display**: colored in rich mode (Yellow=pending, Green=scheduled,
  DarkYellow=in_progress, DarkCyan=completed, DarkGrey=skipped); simple mode
  uses markers ([ ], [~], [>], [x], [-])
- **Status update**: `task update --status <value>` or `task edit` includes
  status field
- **Task list filter**: `task list --status <value>`
- **Editor-based editing**: `task edit <ID>` writes task fields to a temp file,
  opens `$EDITOR` (default `vi`), then parses the saved file and sends PATCH.
  Lines starting with `#` are comments. Empty values are not updated.
- **Subcommands**: `task {list,show,create,edit,update,replace,delete,status}`,
  `schedule {get,generate,reschedule,move,clear}`,
  `token {create,list,revoke}`, `sync {settings,setup,login,trigger}`,
  `habit {list,show,create,edit,update,replace,delete}`

## takusu-client

Standalone HTTP client library for the takusu REST API. Reused by any future
client (Android Kotlin, etc.).

- Types mirror `takusu-contracts` model.rs request/response structs (`TaskRow`,
  `CreateTask`, `UpdateTask`, etc.)
- `Client` struct holds `base_url` + `token`, all methods are async
- Error type: `ClientError { Http, Api { status, body } }` — no `thiserror`
  dependency
