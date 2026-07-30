# Project Structure

```
takusu/
├── doc/                      # 設計ドキュメント
│   ├── plan/                 # プランナー・機能計画 (markdown)
│   ├── mock/                 # UI モック (HTML)
│   └── proposal.typ          # 全体設計ドキュメント (Typst)
├── Cargo.toml                # Rust workspace root
├── crates/
│   ├── takusu-core/          # Core planner (data types, scheduling algorithm)
│   │   ├── src/lib.rs        #   Public API (Point, Task, NormalDist, Planner, Plan, RescheduleRange)
│   │   ├── src/evaluate.rs   #   Evaluation function (8 components)
│   │   ├── src/anneal.rs     #   SA + LNS + Tabu Search (full + partial)
│   │   ├── src/solver.rs     #   Parallel restart + solve/solve_partial entry points
│   │   ├── benches/plan.rs            #   Criterion benchmark (25 tasks)
│   │   ├── benches/realworld.rs       #   Criterion benchmark (real-world habit fixtures)
│   │   ├── examples/daily.rs          #   Example: 1-day schedule with 9 tasks
│   │   ├── examples/common/mod.rs     #   Shared fixture loader for examples
│   │   ├── examples/profile.rs        #   Profile planner.plan() under perf
│   │   └── examples/score_check.rs    #   Microbenchmark evaluate() on a fixed plan
│   ├── takusu-local/         # Local server (axum + SQLite, uses takusu-local-lib)
│   ├── takusu-local-lib/     # Business logic library (shared by takusu-local and takusu-cli)
│   │   ├── src/
│   │   │   ├── app.rs        #   TakusuApp: all business logic + two storage backends
│   │   │   ├── config.rs     #   LocalConfig (env-based, TAKUSU_* prefix)
│   │   │   ├── error.rs      #   AppError enum
│   │   │   ├── storage_sqlite.rs  # SqliteStorage (direct sqlx)
│   │   │   ├── storage_workers.rs # WorkersStorage (HTTP → Cloudflare Worker)
│   │   │   ├── token_cache.rs     # TTL-based token verification cache
│   │   │   └── auth.rs       #   SHA-256 token hashing
│   │   └── migrations/
│   │       ├── 001_init.sql        # DB schema
│   │       ├── 002_google_cal.sql  # Google Calendar settings & event mappings
│   │       ├── 003_settings.sql   # User settings (tz, sleep_start, sleep_end)
│   │       └── 004_indexes.sql
│   ├── takusu-contracts/       # Pluggable storage trait + shared types
│   │   └── src/
│   │       ├── storage.rs    #   Async Storage trait
│   │       ├── model.rs      #   Shared types (TaskRow, HabitRow, ScheduleEntry, etc.)
│   │       └── error.rs      #   StorageError enum
│   ├── takusu-ical/          # iCalendar parser (pure, no HTTP dependency)
│   │   └── src/lib.rs        #   parse_ical(input, tz) → Result<Vec<IcalTask>, IcalError>
│   ├── takusu-habit/         # Recurrence rule engine (RRULE expansion)
│   │   └── src/lib.rs
│   ├── takusu-audio/         # Audio processing (recording + STT backends + TTS trait)
│   │   └── src/
│   │       ├── lib.rs
│   │       ├── record.rs     #   Microphone recording (cpal)
│   │       ├── stt.rs       #   SpeechToText trait
│   │       ├── sherpa.rs    #   Sherpa-ONNX local ASR
│   │       ├── hush.rs      #   Hush ONNX denoiser
│   │       └── tts.rs       #   TextToSpeech trait and shared TTS types
│   ├── takusu-audio-cli/     # CLI for audio recording, transcription, and denoising
│   │   └── src/main.rs      #   STT via Sherpa-ONNX
│   ├── takusu-client/         # HTTP client library for takusu REST API
│   │   └── src/lib.rs         #   Client, all request/response types
│   ├── takusu-cli/            # CLI client (clap derive, editor-based task editing)
│   │   └── src/
│   │       ├── main.rs        #   CLI entry point + subcommand routing
│   │       ├── editor.rs      #   $EDITOR-based task editing (format/parse/open)
│   │       ├── display_rich.rs#   Table display (comfy-table + colored)
│   │       └── display_simple.rs # Plain-text display
│   ├── takusu-worker/         # Cloudflare Worker (Rust/WASM + D1)
│   │   └── src/
│   ├── takusu-util/           # Shared utilities (duration parsing, datetime parsing, token generation)
│   │   └── src/lib.rs
│   └── google-cal/            # Google Calendar API client (reqwest + OAuth2)
│       └── src/lib.rs         #   Client, sync(), delete_all(), OAuth2 helpers
├── flake.nix                 # Nix development environment
├── rust-toolchain.toml       # Rust toolchain config
├── .envrc                    # direnv config
├── AGENTS.md                 # Agent behavior contract (always-on rules)
├── .devin/
│   ├── rules/                # Focused agent rules referenced from AGENTS.md
│   │   ├── pr-workflow.md    # When and how to push / create PRs
│   │   └── agent-helpers.md  # Notifications, scripts, and skills
│   ├── docs/                 # Detailed agent reference
│   │   ├── project-overview.md
│   │   ├── project-structure.md
│   │   ├── development-environment.md
│   │   ├── core-planner.md
│   │   ├── audio.md
│   │   ├── local-api.md
│   │   ├── clients.md
│   │   ├── code-style.md
│   │   ├── agent-implementation.md
│   │   └── dependencies.md
│   └── skills/               # Devin CLI skills (thin wrappers around scripts/)
│       ├── issue-view/SKILL.md
│       ├── issue-assign/SKILL.md
│       ├── pr-watch/SKILL.md
│       ├── jj-resolve/SKILL.md
│       ├── discord-notify/SKILL.md
│       ├── profile/SKILL.md  # Perf flamegraph + top-function summary helper
│       └── optimize/SKILL.md # Benchmark-driven takusu-core optimization workflow
└── scripts/                  # Agent + user helper scripts
    ├── issue-view.sh         # GitHub issue list/show (label/assignee/state filters)
    ├── issue-assign.sh       # Assign an unassigned issue to the current user
    ├── pr-watch.sh           # PR CI/review/comment snapshot + polling watch loop
    ├── jj-resolve.sh         # Jujutsu conflict list/show/edit/mark/merge/abort
    ├── discord-notify.sh     # Discord webhook sender (DISCORD_WEBHOOK_URL env)
    └── profile.sh            # Perf profiling with flamegraph + top-self summary
```
