//! ONNX ASR via `sherpa-onnx` (supports FunASR Nano, SenseVoice,
//! NeMo Parakeet CTC, and Nemotron streaming transducer).
//!
//! This is intended to replace the Python FunASR WebSocket server for local
//! and Android inference. Model files should be downloaded from the
//! `sherpa-onnx` releases and passed via [`SherpaOnnxAsrConfig`].

use std::path::{Path, PathBuf};
use std::sync::Arc;

use crate::stt::{
    AsrStream, ExecutionProvider, SherpaOnnxModel, SpeechToText, StreamingSpeechToText, SttError,
};
use crate::wav::SHERPA_SAMPLE_RATE;
use sherpa_onnx::{
    OfflineFunASRNanoModelConfig, OfflineNemoEncDecCtcModelConfig, OfflineRecognizer,
    OfflineRecognizerConfig, OfflineSenseVoiceModelConfig, OnlineRecognizer,
    OnlineRecognizerConfig, OnlineStream, OnlineTransducerModelConfig,
};

// `SherpaOnnxModel` is imported from `stt` via the `use` above and is also
// re-exported by `lib.rs` directly from `stt`, so no re-export is needed here.

/// Configuration for [`SherpaOnnxAsr`] and [`SherpaOnnxStreamingAsr`].
#[derive(Debug, Clone, Default)]
pub struct SherpaOnnxAsrConfig {
    pub model: SherpaOnnxModel,
    pub model_dir: PathBuf,
    pub tokens: Option<PathBuf>,
    pub num_threads: i32,
    pub provider: ExecutionProvider,
    pub sample_rate: i32,
    /// SenseVoice / Nemotron language, e.g. "auto", "zh", "en", "ja", "ko".
    pub language: Option<String>,
    /// SenseVoice ITN (inverse text normalization).
    pub use_itn: bool,
}

/// ONNX offline ASR backend using `sherpa-onnx`.
#[derive(Clone)]
pub struct SherpaOnnxAsr {
    recognizer: Arc<OfflineRecognizer>,
    sample_rate: i32,
}

impl SherpaOnnxAsr {
    /// Create an ASR backend from a full configuration.
    ///
    /// The directory is expected to contain one of the following layouts:
    /// - SenseVoice: `model*.onnx` and `tokens.txt`
    /// - FunASR Nano: `encoder_adaptor*.onnx`, `llm*.onnx`, `embedding*.onnx`,
    ///   a tokenizer directory (e.g. `Qwen3-0.6B`), and `tokens.txt`
    /// - Parakeet CTC: `model.int8.onnx` and `tokens.txt`
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
            SherpaOnnxModel::ParakeetJaCtc => {
                let model = find_file(dir, "model")
                    .or_else(|| find_file(dir, "model.int8"))
                    .ok_or_else(|| {
                        SttError::Other(format!(
                            "no Parakeet model.onnx found in {}",
                            dir.display()
                        ))
                    })?;
                offline_config.model_config.nemo_ctc = OfflineNemoEncDecCtcModelConfig {
                    model: Some(model.to_string_lossy().to_string()),
                };
                offline_config.model_config.modeling_unit = Some("bpe".to_string());
            }
            SherpaOnnxModel::NemotronMultilingual => {
                return Err(SttError::Other(
                    "use SherpaOnnxStreamingAsr for Nemotron".to_string(),
                ));
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
            recognizer: Arc::new(recognizer),
            sample_rate,
        })
    }
}

#[async_trait::async_trait]
impl SpeechToText for SherpaOnnxAsr {
    async fn transcribe(&self, audio: &[f32]) -> Result<String, SttError> {
        let audio = audio.to_vec();
        let this = self.clone();
        tokio::task::spawn_blocking(move || this.transcribe_sync(&audio))
            .await
            .map_err(|e| SttError::Other(format!("transcription task failed: {e}")))?
    }

    fn transcribe_sync(&self, audio: &[f32]) -> Result<String, SttError> {
        let stream = self.recognizer.create_stream();
        stream.accept_waveform(self.sample_rate, audio);
        self.recognizer.decode(&stream);
        let result = stream.get_result().ok_or(SttError::NoResult)?;
        Ok(result.text)
    }
}

#[async_trait::async_trait]
impl StreamingSpeechToText for SherpaOnnxAsr {
    async fn start_stream(&self, _language: &str) -> Result<Box<dyn AsrStream>, SttError> {
        let asr: Arc<dyn SpeechToText> = Arc::new(self.clone());
        Ok(Box::new(OfflineAsrStream::new(asr)))
    }
}

/// ONNX streaming ASR backend using `sherpa-onnx`.
#[derive(Clone)]
pub struct SherpaOnnxStreamingAsr {
    recognizer: Arc<OnlineRecognizer>,
    sample_rate: i32,
    language: String,
}

impl SherpaOnnxStreamingAsr {
    /// Create a streaming ASR backend from a full configuration.
    ///
    /// The directory is expected to contain `encoder*.onnx`, `decoder*.onnx`,
    /// `joiner*.onnx`, and `tokens.txt`.
    pub fn from_config(config: &SherpaOnnxAsrConfig) -> Result<Self, SttError> {
        if !matches!(config.model, SherpaOnnxModel::NemotronMultilingual) {
            return Err(SttError::Other(
                "SherpaOnnxStreamingAsr only supports Nemotron".to_string(),
            ));
        }

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

        let encoder = find_file(dir, "encoder").ok_or_else(|| {
            SttError::Other(format!("no encoder*.onnx found in {}", dir.display()))
        })?;
        let decoder = find_file(dir, "decoder").ok_or_else(|| {
            SttError::Other(format!("no decoder*.onnx found in {}", dir.display()))
        })?;
        let joiner = find_file(dir, "joiner").ok_or_else(|| {
            SttError::Other(format!("no joiner*.onnx found in {}", dir.display()))
        })?;

        let mut online_config = OnlineRecognizerConfig::default();
        online_config.model_config.tokens = Some(tokens.to_string_lossy().to_string());
        online_config.model_config.num_threads = if config.num_threads > 0 {
            config.num_threads
        } else {
            2
        };
        online_config.model_config.provider = Some(config.provider.to_string());
        online_config.model_config.transducer = OnlineTransducerModelConfig {
            encoder: Some(encoder.to_string_lossy().to_string()),
            decoder: Some(decoder.to_string_lossy().to_string()),
            joiner: Some(joiner.to_string_lossy().to_string()),
        };
        online_config.model_config.model_type = Some("nemo_transducer".to_string());
        online_config.decoding_method = Some("greedy_search".to_string());
        online_config.enable_endpoint = true;

        let recognizer = OnlineRecognizer::create(&online_config).ok_or_else(|| {
            SttError::Other("failed to create sherpa-onnx streaming recognizer".to_string())
        })?;

        let sample_rate = if config.sample_rate > 0 {
            config.sample_rate
        } else {
            SHERPA_SAMPLE_RATE as i32
        };
        let language = config.language.clone().unwrap_or_default();

        Ok(Self {
            recognizer: Arc::new(recognizer),
            sample_rate,
            language,
        })
    }
}

#[async_trait::async_trait]
impl SpeechToText for SherpaOnnxStreamingAsr {
    async fn transcribe(&self, audio: &[f32]) -> Result<String, SttError> {
        let audio = audio.to_vec();
        let this = self.clone();
        tokio::task::spawn_blocking(move || this.transcribe_sync(&audio))
            .await
            .map_err(|e| SttError::Other(format!("transcription task failed: {e}")))?
    }

    fn transcribe_sync(&self, audio: &[f32]) -> Result<String, SttError> {
        let handle = tokio::runtime::Handle::try_current().map_err(|e| {
            SttError::Other(format!(
                "no tokio runtime available for streaming transcription: {e}"
            ))
        })?;
        handle.block_on(async {
            let mut stream = self.start_stream(&self.language).await?;
            stream.accept_waveform(audio);
            stream.finish().await
        })
    }
}

#[async_trait::async_trait]
impl StreamingSpeechToText for SherpaOnnxStreamingAsr {
    async fn start_stream(&self, language: &str) -> Result<Box<dyn AsrStream>, SttError> {
        let stream = self.recognizer.create_stream();
        let lang = if language.is_empty() {
            &self.language
        } else {
            language
        };
        if !lang.is_empty() {
            stream.set_option("language", lang);
        }
        Ok(Box::new(SherpaOnnxStream {
            recognizer: Arc::clone(&self.recognizer),
            stream,
            sample_rate: self.sample_rate,
        }))
    }
}

/// Generic offline ASR stream that buffers samples and transcribes on `finish`.
pub struct OfflineAsrStream {
    asr: Arc<dyn SpeechToText>,
    buffer: Vec<f32>,
}

impl OfflineAsrStream {
    /// Create a new offline stream wrapping the given ASR backend.
    pub fn new(asr: Arc<dyn SpeechToText>) -> Self {
        Self {
            asr,
            buffer: Vec::new(),
        }
    }
}

#[async_trait::async_trait]
impl AsrStream for OfflineAsrStream {
    fn accept_waveform(&mut self, samples: &[f32]) {
        self.buffer.extend_from_slice(samples);
    }

    fn text(&mut self) -> String {
        String::new()
    }

    async fn finish(&mut self) -> Result<String, SttError> {
        let asr = Arc::clone(&self.asr);
        let audio = std::mem::take(&mut self.buffer);
        asr.transcribe(&audio).await
    }
}

struct SherpaOnnxStream {
    recognizer: Arc<OnlineRecognizer>,
    stream: OnlineStream,
    sample_rate: i32,
}

#[async_trait::async_trait]
impl AsrStream for SherpaOnnxStream {
    fn accept_waveform(&mut self, samples: &[f32]) {
        self.stream.accept_waveform(self.sample_rate, samples);
    }

    fn text(&mut self) -> String {
        while self.recognizer.is_ready(&self.stream) {
            self.recognizer.decode(&self.stream);
        }
        self.recognizer
            .get_result(&self.stream)
            .map(|r| r.text)
            .unwrap_or_default()
    }

    async fn finish(&mut self) -> Result<String, SttError> {
        self.stream.input_finished();
        while self.recognizer.is_ready(&self.stream) {
            self.recognizer.decode(&self.stream);
        }
        self.recognizer
            .get_result(&self.stream)
            .ok_or(SttError::NoResult)
            .map(|r| r.text)
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
