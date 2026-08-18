//! Speech-to-text provider trait and shared config types.

use std::path::PathBuf;
#[cfg(feature = "sherpa")]
use std::sync::Arc;

use serde::{Deserialize, Serialize};
use thiserror::Error;

/// STT backend identifier, symmetric with [`crate::tts::TtsBackend`].
///
/// Adding a new variant forces every `match` on this enum to be updated,
/// so backends cannot be silently missed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
#[cfg_attr(feature = "clap", derive(clap::ValueEnum))]
pub enum SttBackend {
    #[default]
    #[cfg_attr(feature = "clap", value(name = "sherpa"))]
    Sherpa,
}

impl std::fmt::Display for SttBackend {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SttBackend::Sherpa => write!(f, "sherpa"),
        }
    }
}

impl std::str::FromStr for SttBackend {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "sherpa" => Ok(SttBackend::Sherpa),
            _ => Err(format!("unsupported STT backend: {s}")),
        }
    }
}

/// ONNX execution provider for Sherpa-ONNX inference.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
#[cfg_attr(feature = "clap", derive(clap::ValueEnum))]
pub enum ExecutionProvider {
    #[default]
    #[cfg_attr(feature = "clap", value(name = "cpu"))]
    Cpu,
    #[cfg_attr(feature = "clap", value(name = "cuda"))]
    Cuda,
    #[cfg_attr(feature = "clap", value(name = "coreml"))]
    CoreMl,
}

impl std::fmt::Display for ExecutionProvider {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ExecutionProvider::Cpu => write!(f, "cpu"),
            ExecutionProvider::Cuda => write!(f, "cuda"),
            ExecutionProvider::CoreMl => write!(f, "coreml"),
        }
    }
}

impl std::str::FromStr for ExecutionProvider {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "cpu" => Ok(ExecutionProvider::Cpu),
            "cuda" => Ok(ExecutionProvider::Cuda),
            "coreml" => Ok(ExecutionProvider::CoreMl),
            _ => Err(format!("unsupported execution provider: {s}")),
        }
    }
}

/// Model family to load.
///
/// Defined here (rather than only in `sherpa.rs`) so that config types can
/// reference it without requiring the `sherpa` feature.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "kebab-case")]
#[cfg_attr(feature = "clap", derive(clap::ValueEnum))]
pub enum SherpaOnnxModel {
    /// SenseVoice (multilingual, smaller, faster).
    #[default]
    #[cfg_attr(feature = "clap", value(name = "sense-voice"))]
    SenseVoice,
    /// FunASR Nano (LLM-based, higher quality, larger).
    #[cfg_attr(feature = "clap", value(name = "funasr-nano"))]
    FunasrNano,
    /// NeMo Parakeet TDT CTC for Japanese.
    #[serde(rename = "parakeet-ctc-ja")]
    #[cfg_attr(feature = "clap", value(name = "parakeet-ctc-ja"))]
    ParakeetJaCtc,
    /// Multilingual streaming NeMo Transducer (Japanese + 39 other locales).
    #[serde(rename = "nemotron-ja")]
    #[cfg_attr(feature = "clap", value(name = "nemotron-ja"))]
    NemotronMultilingual,
}

/// Handle for one streaming (online) ASR session.
#[async_trait::async_trait]
pub trait AsrStream: Send {
    /// Append the next chunk of f32 PCM samples (typically 16 kHz mono).
    fn accept_waveform(&mut self, samples: &[f32]);
    /// Decode if enough audio is available and return the current partial transcript.
    fn text(&mut self) -> String;
    /// Signal end of input and return the final transcript.
    async fn finish(&mut self) -> Result<String, SttError>;
}

#[async_trait::async_trait]
pub trait SpeechToText: Send + Sync {
    async fn transcribe(&self, audio: &[f32]) -> Result<String, SttError>;

    /// Synchronous version of [`transcribe`].
    ///
    /// This is used by offline streaming wrappers (e.g. [`OfflineAsrStream`])
    /// to avoid blocking a tokio async worker. Backends that are inherently
    /// async may return an error if a sync transcription is not supported.
    fn transcribe_sync(&self, audio: &[f32]) -> Result<String, SttError> {
        let _ = audio;
        Err(SttError::Other(
            "transcribe_sync is not supported by this backend".to_string(),
        ))
    }
}

#[async_trait::async_trait]
pub trait StreamingSpeechToText: SpeechToText + Send + Sync {
    /// Start a new streaming ASR session. `language` is the desired language hint
    /// (e.g. "ja", "auto"). For offline models this can be ignored.
    async fn start_stream(&self, language: &str) -> Result<Box<dyn AsrStream>, SttError>;
}

/// Provider-neutral STT configuration used by the CLI and the agent.
///
/// This is a runtime config (not deserialized from TOML directly — the
/// agent wraps it in its own serde struct). The CLI constructs it from
/// command-line arguments and calls [`SttRuntimeConfig::build`] to obtain a
/// concrete [`SpeechToText`] backend.
///
/// Named `SttRuntimeConfig` (not `SttConfig`) to avoid collision with
/// `takusu_agent::audio_config::SttConfig`, which is the serde/persisted
/// config deserialized from TOML.
#[derive(Debug, Clone, Default)]
pub struct SttRuntimeConfig {
    pub backend: SttBackend,
    pub model: SherpaOnnxModel,
    pub model_dir: Option<PathBuf>,
    /// SenseVoice / Nemotron language, e.g. "auto", "zh", "en", "ja", "ko".
    pub language: String,
    pub use_itn: bool,
    pub num_threads: i32,
    pub provider: ExecutionProvider,
    pub sample_rate: i32,
}

impl SttRuntimeConfig {
    /// Default streaming mode for the selected model. Offline models stream
    /// through the offline recognizer as a compatibility wrapper.
    pub fn default_streaming(&self) -> bool {
        matches!(self.model, SherpaOnnxModel::NemotronMultilingual)
    }

    /// Build a concrete [`StreamingSpeechToText`] backend from this config.
    ///
    /// The `match` on [`SttBackend`] is exhaustive, so adding a new variant
    /// without handling it here is a compile error.
    #[cfg(feature = "sherpa")]
    pub fn build_streaming(&self) -> Result<Arc<dyn StreamingSpeechToText>, SttError> {
        match self.backend {
            SttBackend::Sherpa => {
                use crate::sherpa::{SherpaOnnxAsr, SherpaOnnxAsrConfig, SherpaOnnxStreamingAsr};

                let model_dir = match &self.model_dir {
                    Some(dir) => dir.clone(),
                    None => {
                        if matches!(self.model, SherpaOnnxModel::FunasrNano) {
                            return Err(SttError::Other(
                                "sherpa funasr-nano requires a model_dir".to_string(),
                            ));
                        }
                        let cache = crate::ModelCache::default_dir()?;
                        let id = match self.model {
                            SherpaOnnxModel::SenseVoice => "sherpa-sense-voice-int8",
                            SherpaOnnxModel::ParakeetJaCtc => "sherpa-parakeet-ctc-ja-0.6b",
                            SherpaOnnxModel::NemotronMultilingual => "sherpa-nemotron-ja-0.6b",
                            SherpaOnnxModel::FunasrNano => unreachable!(),
                        };
                        cache.ensure(id)?
                    }
                };

                let asr_config = SherpaOnnxAsrConfig {
                    model_dir,
                    model: self.model,
                    tokens: None,
                    num_threads: self.num_threads,
                    provider: self.provider,
                    sample_rate: self.sample_rate,
                    language: Some(self.language.clone()),
                    use_itn: self.use_itn,
                };

                let asr: Arc<dyn StreamingSpeechToText> = match self.model {
                    SherpaOnnxModel::NemotronMultilingual => {
                        Arc::new(SherpaOnnxStreamingAsr::from_config(&asr_config)?)
                    }
                    _ => Arc::new(SherpaOnnxAsr::from_config(&asr_config)?),
                };
                Ok(asr)
            }
        }
    }

    /// Build a concrete [`StreamingSpeechToText`] backend from this config.
    ///
    /// This is a thin wrapper over [`SttRuntimeConfig::build_streaming`] that
    /// keeps the existing [`SpeechToText`] API available through the
    /// supertrait.
    #[cfg(feature = "sherpa")]
    pub fn build(&self) -> Result<Arc<dyn StreamingSpeechToText>, SttError> {
        self.build_streaming()
    }
}

#[derive(Debug, Error)]
pub enum SttError {
    #[error("http error: {0}")]
    Http(#[from] reqwest::Error),
    #[error("api error {status}: {code}: {message}")]
    Api {
        status: u16,
        code: String,
        message: String,
    },
    #[error("model error: {0}")]
    Model(#[from] crate::models::ModelError),
    #[error("no result received")]
    NoResult,
    #[error("other error: {0}")]
    Other(String),
}
