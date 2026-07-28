//! ONNX ASR via `sherpa-onnx` (supports FunASR Nano and SenseVoice).
//!
//! This is intended to replace the Python FunASR WebSocket server for local
//! and Android inference. Model files should be downloaded from the
//! `sherpa-onnx` releases and passed via [`SherpaOnnxAsrConfig`].

use std::path::{Path, PathBuf};

use crate::stt::{ExecutionProvider, SherpaOnnxModel, SpeechToText, SttError};
use crate::wav::SHERPA_SAMPLE_RATE;
use sherpa_onnx::{
    OfflineFunASRNanoModelConfig, OfflineRecognizer, OfflineRecognizerConfig,
    OfflineSenseVoiceModelConfig,
};

// `SherpaOnnxModel` is imported from `stt` via the `use` above and is also
// re-exported by `lib.rs` directly from `stt`, so no re-export is needed here.

/// Configuration for [`SherpaOnnxAsr`].
#[derive(Debug, Clone, Default)]
pub struct SherpaOnnxAsrConfig {
    pub model: SherpaOnnxModel,
    pub model_dir: PathBuf,
    pub tokens: Option<PathBuf>,
    pub num_threads: i32,
    pub provider: ExecutionProvider,
    pub sample_rate: i32,
    /// SenseVoice language, e.g. "auto", "zh", "en", "ja", "ko".
    pub language: Option<String>,
    /// SenseVoice ITN (inverse text normalization).
    pub use_itn: bool,
}

/// ONNX ASR backend using `sherpa-onnx`.
pub struct SherpaOnnxAsr {
    recognizer: OfflineRecognizer,
    sample_rate: i32,
}

impl SherpaOnnxAsr {
    /// Create an ASR backend from a full configuration.
    ///
    /// The directory is expected to contain one of the following layouts:
    /// - SenseVoice: `model*.onnx` and `tokens.txt`
    /// - FunASR Nano: `encoder_adaptor*.onnx`, `llm*.onnx`, `embedding*.onnx`,
    ///   a tokenizer directory (e.g. `Qwen3-0.6B`), and `tokens.txt`
    pub fn from_config(config: &SherpaOnnxAsrConfig) -> Result<Self, SttError> {
        let dir = &config.model_dir;
        let tokens = config
            .tokens
            .clone()
            .unwrap_or_else(|| dir.join("tokens.txt"));
        if !tokens.exists() {
            return Err(SttError::Other(format!(
                "tokens.txt not found in {}",
                tokens.display()
            )));
        }

        let mut offline_config = OfflineRecognizerConfig::default();
        offline_config.model_config.tokens = Some(tokens.to_string_lossy().to_string());
        offline_config.model_config.num_threads = if config.num_threads > 0 {
            config.num_threads
        } else {
            2
        };
        offline_config.model_config.provider = Some(config.provider.to_string());

        match config.model {
            SherpaOnnxModel::SenseVoice => {
                let model = find_file(dir, "model")
                    .or_else(|| find_file(dir, "model.int8"))
                    .ok_or_else(|| {
                        SttError::Other(format!(
                            "no SenseVoice model.onnx found in {}",
                            dir.display()
                        ))
                    })?;
                offline_config.model_config.sense_voice = OfflineSenseVoiceModelConfig {
                    model: Some(model.to_string_lossy().to_string()),
                    language: Some(
                        config
                            .language
                            .clone()
                            .unwrap_or_else(|| "auto".to_string()),
                    ),
                    use_itn: config.use_itn,
                };
            }
            SherpaOnnxModel::FunasrNano => {
                let encoder_adaptor = find_file(dir, "encoder_adaptor").ok_or_else(|| {
                    SttError::Other(format!(
                        "no encoder_adaptor*.onnx found in {}",
                        dir.display()
                    ))
                })?;
                let llm = find_file(dir, "llm").ok_or_else(|| {
                    SttError::Other(format!("no llm*.onnx found in {}", dir.display()))
                })?;
                let embedding = find_file(dir, "embedding").ok_or_else(|| {
                    SttError::Other(format!("no embedding*.onnx found in {}", dir.display()))
                })?;
                let tokenizer = find_tokenizer_dir(dir).ok_or_else(|| {
                    SttError::Other(format!("no tokenizer directory found in {}", dir.display()))
                })?;
                offline_config.model_config.funasr_nano = OfflineFunASRNanoModelConfig {
                    encoder_adaptor: Some(encoder_adaptor.to_string_lossy().to_string()),
                    llm: Some(llm.to_string_lossy().to_string()),
                    embedding: Some(embedding.to_string_lossy().to_string()),
                    tokenizer: Some(tokenizer.to_string_lossy().to_string()),
                    ..Default::default()
                };
            }
        }

        let sample_rate = if config.sample_rate > 0 {
            config.sample_rate
        } else {
            SHERPA_SAMPLE_RATE as i32
        };
        Self::with_config(offline_config, sample_rate)
    }

    /// Create an ASR backend from a model directory with sensible defaults.
    pub fn from_model_dir(dir: impl AsRef<Path>, model: SherpaOnnxModel) -> Result<Self, SttError> {
        Self::from_config(&SherpaOnnxAsrConfig {
            model_dir: dir.as_ref().to_path_buf(),
            model,
            ..Default::default()
        })
    }

    /// Create an ASR backend from an explicit `sherpa-onnx` config.
    pub fn with_config(
        config: OfflineRecognizerConfig,
        sample_rate: i32,
    ) -> Result<Self, SttError> {
        let recognizer = OfflineRecognizer::create(&config).ok_or_else(|| {
            SttError::Other("failed to create sherpa-onnx recognizer".to_string())
        })?;
        Ok(Self {
            recognizer,
            sample_rate,
        })
    }
}

#[async_trait::async_trait]
impl SpeechToText for SherpaOnnxAsr {
    async fn transcribe(&self, audio: &[f32]) -> Result<String, SttError> {
        let stream = self.recognizer.create_stream();
        stream.accept_waveform(self.sample_rate, audio);
        self.recognizer.decode(&stream);
        let result = stream.get_result().ok_or(SttError::NoResult)?;
        Ok(result.text)
    }
}

fn find_file(dir: &Path, prefix: &str) -> Option<PathBuf> {
    let names = ["", ".int8"];
    for name in names {
        let candidate = dir.join(format!("{}{}.onnx", prefix, name));
        if candidate.exists() {
            return Some(candidate);
        }
    }
    None
}

fn find_tokenizer_dir(dir: &Path) -> Option<PathBuf> {
    for name in ["Qwen3-0.6B", "tokenizer"] {
        let candidate = dir.join(name);
        if candidate.is_dir() {
            return Some(candidate);
        }
    }
    // fall back to any sub-directory containing tokenizer.json
    for entry in std::fs::read_dir(dir).ok()?.flatten() {
        let path = entry.path();
        if path.is_dir() && path.join("tokenizer.json").exists() {
            return Some(path);
        }
    }
    None
}
