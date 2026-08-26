# Audio (takusu-audio)

## Wake word / keyword spotting

- The ambient gate (`AmbientPipeline` in `takusu-audio/src/ambient.rs`) runs
  VAD → wake word → streaming ASR.
- `WakeWordBackend::AsrTextMatch` is the **default**: it matches the wake phrase
  in a streaming ASR partial transcript. It is language-agnostic and is the only
  backend that reliably detects a Japanese wake word such as 「たくす」.
- `WakeWordBackend::SherpaKws` uses a sherpa-onnx transducer keyword spotter.
  The pretrained KWS models (WenetSpeech Chinese / GigaSpeech English) do not
  cover Japanese, and sherpa-onnx has no Japanese KWS model; the multilingual
  streaming zipformer also fails to fire on multi-character keywords. It is
  retained for Chinese/English keywords and for evaluation.
- See `doc/ambient-wake-evaluation.md` for the desktop wake-word evaluation log
  and how to run one.

## Text-to-Speech

- `TextToSpeech` trait: `synthesize_stream(request) -> Result<TtsStream, TtsError>`; `synthesize`
  collects the stream into a `Vec<u8>` for callers that do not need streaming.
- `TtsRequest`, `TtsOptions`, `TtsConfig`, and `TtsError` are shared types
- `play_stream` plays a raw `TtsStream` through the default output device so
  read-aloud can start before synthesis finishes
- A new concrete backend will be added alongside `takusu-audio` STT backends

## Voiceprint / speaker recognition

- Speaker embedding and verification lives in `takusu-audio/src/speaker.rs`
  using Sherpa-ONNX.
- `takusu-audio-cli` (binary `takusu-audio`) is an experimental/test CLI for
  audio recording, STT, and denoising. It is not intended for production use.
- `takusu-cli` (`takusu speaker ...`) is the normal user-facing CLI for
  voiceprint enrollment, verification, deletion, and listing.
