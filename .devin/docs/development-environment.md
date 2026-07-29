# Development Environment

Use `nix develop` or `direnv allow` to enter the development shell. The flake
provides:
- `rust-bin` (from `rust-toolchain.toml`: stable + rustfmt + clippy)
- `cargo-expand`, `cargo-nextest`

## Key Commands

| Command | Description |
|---------|-------------|
| `cargo check` | Type-check all crates |
| `cargo fmt` / `treefmt` | Format code |
| `cargo clippy` | Lint |
| `cargo nextest run --workspace` | Run all tests (~171 across crates) |
| `./scripts/coverage.sh [text\|lcov\|html\|open]` | Measure Rust workspace coverage with `cargo-llvm-cov` (excludes `takusu-worker`); `lcov` writes `target/coverage/lcov.info` + html |
| `cargo nextest run -p takusu-core` | Run core planner tests (45) |
| `cargo nextest run -p takusu-local` | Run local server integration tests (39) |
| `cargo nextest run -p takusu-ical` | Run iCal parser tests (15) |
| `cargo bench -p takusu-core` | Run core benchmarks (synthetic plan and realworld habit fixtures) |
| `cargo bench -p takusu-habit` | Run habit recurrence expansion benchmarks |
| `cargo bench -p takusu-ical` | Run iCal parsing benchmark |
| `cargo codspeed build` and `cargo codspeed run` | Build/run all benchmarks with CodSpeed |
| `cargo flamegraph --bench realworld` | Generate a flamegraph for the realworld benchmark |
| `./scripts/profile.sh --example profile -p takusu-core` | Profile a target under perf, output flamegraph + top-self summary |
| `cargo run -p takusu-habit --example expand_realworld -- --horizon-days N --output ...` | Regenerate real-world task fixtures from the habit fixture |
| `cargo test -p takusu-worker` | Run takusu-worker unit tests (6 auth tests) |
| `cargo test -p takusu-worker --test auth -- --ignored` | Run takusu-worker auth integration tests (requires `wrangler`) |
| `cargo test -p takusu-local --test storage_suite -- --ignored --test-threads=1` | Run parameterized storage-suite (sqlite + real worker via `wrangler dev`; requires `wrangler` + `worker-build --release` first) |
| `cargo run --example daily` | Run daily schedule example |
| `cargo run -p takusu-cli -- --help` | Run CLI client |
| `cargo run -p takusu-local` | Start local server |
| `cargo run -p takusu-audio-cli -- record` | Record microphone audio |
| `cargo run -p takusu-audio-cli -- transcribe audio.wav` | Transcribe a WAV file with Sherpa-ONNX |
| `./scripts/issue-view.sh list [--label L] [--assignee A] [--state S]` | List GitHub issues (TSV: number, title, labels, assignees, state) |
| `./scripts/issue-view.sh show <N>` | Show issue title, body, labels, assignees, and full comment thread |
| `./scripts/issue-assign.sh <N> [<N>...] [--assignee <user>]` | Assign an unassigned issue to the current user (or another user) |
| `./scripts/pr-watch.sh show <PR>` | One-shot snapshot of PR CI checks, reviews, and comments |
| `./scripts/pr-watch.sh watch <PR> [--interval N] [--max N]` | Poll a PR and print diffs to stdout (no notification); default 60s |
| `./scripts/jj-resolve.sh list\|status\|show\|edit\|mark\|merge\|abort` | Inspect and resolve Jujutsu merge conflicts |
| `./scripts/discord-notify.sh "text" \| --title T --desc D` | Send a message/embed to Discord (`$DISCORD_WEBHOOK_URL`) |
