# Audio (takusu-audio)

## Text-to-Speech

- `TextToSpeech` trait: `synthesize_stream(request) -> Result<TtsStream, TtsError>`; `synthesize`
  collects the stream into a `Vec<u8>` for callers that do not need streaming.
- `TtsRequest`, `TtsOptions`, `TtsConfig`, and `TtsError` are shared types
- `play_stream` plays a raw `TtsStream` through the default output device so
  read-aloud can start before synthesis finishes
- Concurrency: each TTS backend (Cartesia, Fish) bounds in-flight synthesis
  requests to `max_concurrent` (default 2) with a semaphore and retries
  transient HTTP 429 / 5xx failures with backoff. This keeps parallel block
  synthesis below the provider's free-tier concurrency limit.
- A new concrete backend will be added alongside `takusu-audio` STT backends

## Voiceprint / speaker recognition

- Speaker embedding and verification lives in `takusu-audio/src/speaker.rs`
  using Sherpa-ONNX.
- `takusu-audio-cli` (binary `takusu-audio`) is an experimental/test CLI for
  audio recording, STT, and denoising. It is not intended for production use.
- `takusu-cli` (`takusu speaker ...`) is the normal user-facing CLI for
  voiceprint enrollment, verification, deletion, and listing.
