//! Speech-to-text provider trait and shared config types.

use std::path::PathBuf;

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
    /// SenseVoice language, e.g. "auto", "zh", "en", "ja", "ko".
    pub language: String,
    pub use_itn: bool,
    pub num_threads: i32,
    pub provider: ExecutionProvider,
    pub sample_rate: i32,
}

impl SttRuntimeConfig {
    /// Build a concrete [`SpeechToText`] backend from this config.
    ///
    /// The `match` on [`SttBackend`] is exhaustive, so adding a new variant
    /// without handling it here is a compile error.
    #[cfg(feature = "sherpa")]
    pub fn build(&self) -> Result<std::sync::Arc<dyn SpeechToText>, SttError> {
        use std::sync::Arc;

        match self.backend {
            SttBackend::Sherpa => {
                use crate::sherpa::{SherpaOnnxAsr, SherpaOnnxAsrConfig};

                let model_dir = match &self.model_dir {
                    Some(dir) => dir.clone(),
                    None => {
                        if matches!(self.model, SherpaOnnxModel::FunasrNano) {
                            return Err(SttError::Other(
                                "sherpa funasr-nano requires a model_dir".to_string(),
                            ));
                        }
                        let cache = crate::ModelCache::default_dir()
                            .map_err(|e| SttError::Other(e.to_string()))?;
                        cache
                            .ensure("sherpa-sense-voice-int8")
                            .map_err(|e| SttError::Other(e.to_string()))?
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
                let asr = SherpaOnnxAsr::from_config(&asr_config)?;
                Ok(Arc::new(asr))
            }
        }
    }
}

#[derive(Debug, Error)]
pub enum SttError {
    #[error("connection error: {0}")]
    Connection(String),
    #[error("server error: {0}")]
    Server(String),
    #[error("no result received")]
    NoResult,
    #[error("other error: {0}")]
    Other(String),
}

#[async_trait::async_trait]
pub trait SpeechToText: Send + Sync {
    async fn transcribe(&self, audio: &[f32]) -> Result<String, SttError>;
}
