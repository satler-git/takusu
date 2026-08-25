# Desktop ambient wake-word evaluation

This document tracks the desktop-only evaluation of wake-word approaches for
resident-agent ambient listening (WI-21). The implementation lives in
`crates/takusu-desktop/src/audio.rs`, `tray.rs`, `notify.rs`, and `config.rs`.

## Goal

Decide which wake-word gate to carry forward to the Android microphone service
(WI-23) by measuring, on the user's actual Linux desktop:

1. `WakeWordBackend::AsrTextMatch` — desktop-only streaming-ASR + text match.
2. `WakeWordBackend::SherpaKws` — pretrained sherpa-onnx transducer KWS.
3. (Future) Custom keyword model (openWakeWord or similar), if the above two
   fail on the Japanese wake phrase.

## How to run an evaluation

1. Enable ambient listening in the agent audio config, e.g.
   `~/.config/takusu/agent.toml`:

   ```toml
   [audio.ambient]
   enabled = true
   wake_word = "たくす"
   wake_word_backend = "asr_text_match"  # or "sherpa_kws"
   ```

2. To start ambient when the daemon launches, set in `~/.config/takusu/config.toml`:

   ```toml
   [desktop.ambient]
   auto_start = true
   ```

3. The wake-word log is written to `~/.local/state/takusu/ambient-wake.log` by
   default. Override with `desktop.ambient.log_path` or
   `TAKUSU_DESKTOP_AMBIENT_LOG_PATH`.

4. The tray icon and a persistent notification show when ambient is active.
   Either can stop listening immediately.

## Log format

Each wake event appends one TSV line:

```text
<ISO 8601 timestamp>\t<backend>\t<wake_word>\t<transcript>
```

- `backend`: `AsrTextMatch` or `SherpaKws`.
- `wake_word`: the configured phrase.
- `transcript`: the ASR text after the pipeline strips the leading wake word.
  It is the command the agent tries to process, not the pre-gate raw audio or
  partial transcript.

## Labeling false fires and misses

False fires and misses are user-reported for now. A multi-day run produces a log
that can be reviewed against the user's memory and other device recordings.

- **False fire**: a wake event in the log where the user did **not** say the wake
  word.
- **Miss**: the user said the wake word and expected a response, but no line was
  appended.

The pipeline intentionally does not persist pre-gate audio or partial ASR
transcripts, so the log is the only persistent artifact. This matches the
privacy boundary in `resident-agent.md`: only the utterance that passes the gate
is stored.

## Result template

Fill this table after each multi-day run.

| Backend | Wake word | Run days | Wake events | False fires | Misses | Notes |
|--------|------------|----------|-------------|-------------|--------|-------|
| `AsrTextMatch` | `たくす` | | | | | |
| `SherpaKws` | `たくす` | | | | | |

## Open question update

The open question in `doc/resident-agent.md` asks whether a pretrained KWS model
works for the Japanese wake phrase. The evaluation above decides between
`AsrTextMatch` and `SherpaKws`; if both fail, the next step is a custom keyword
model.
