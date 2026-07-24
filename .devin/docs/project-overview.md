# Project Overview

takusu is a planner that automatically builds user schedules and a voice
assistant using LLM as the UI. The design document is `doc/proposal.typ`
(in Japanese).

- **License**: MIT
- **Repository**: https://github.com/takusu-dev/takusu

## References

The agent loop and tool-calling abstractions are informed by the reference
implementation in [pi](https://github.com/earendil-works/pi) (`packages/agent`),
by Mario Zechner, used under the MIT License.

## Tech Stack

- **Language**: Rust (edition 2024, stable toolchain)
- **Kotlin**: Planned for Android app
- **Version Control**: Jujutsu (`jj`) + Git (GitHub) — **Jujutsu is the
  preferred VCS in this workspace.** See `.devin/rules/pr-workflow.md`.
- **Nix**: `flake.nix` provides the dev shell (direnv with `use flake`)

## Key Design Decisions (from `doc/proposal.typ`)

- **Planner**: Uses heuristic algorithms (simulated annealing) with an
  evaluation function, not exact SAT solving. Tasks are discretized into
  5-minute slots.
- **Voice Assistant**: Android `VoiceInteractionService` + server for LLM
  processing. STT uses Sherpa-ONNX local inference. LLM fills in missing
  information (estimates, etc.) using memory of past similar tasks.
- **Task model**: Includes start time, deadline, cost estimate (normal
  distribution), dependencies, parallelizability, and `abandonability`
  (deadline flexibility).
- **Documentation**: `README.md` (overview), `ARCHITECTURE.md` (structure),
  the design document (`doc/proposal.typ`), and `.devin/docs/*.md` serve as the
  primary project documentation.
