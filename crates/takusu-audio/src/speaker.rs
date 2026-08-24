//! Speaker embedding extraction and verification using sherpa-onnx.
//!
//! The core [`SpeakerVerifier`] is available when the `sherpa` feature is
//! enabled. Configuration and result types are always available so callers can
//! build agent configs without pulling in the full ONNX runtime.

#[cfg(feature = "sherpa")]
use std::path::{Path, PathBuf};
#[cfg(feature = "sherpa")]
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::stt::ExecutionProvider;
#[cfg(feature = "sherpa")]
use crate::wav::SHERPA_SAMPLE_RATE;

/// Default model id for the recommended 3D-Speaker CAM++ Chinese-English
/// advanced model (~27 MB, 192-dim embeddings).
pub const DEFAULT_SPEAKER_MODEL_ID: &str = "sherpa-speaker-campplus-zh-en";

/// Default cosine-similarity threshold for accepting a speaker match.
pub const DEFAULT_VERIFY_THRESHOLD: f32 = 0.5;

/// Minimum number of seconds of audio recommended for a reliable embedding.
pub const MIN_SPEAKER_AUDIO_SECONDS: f32 = 0.5;

/// Configuration for a [`SpeakerVerifier`].
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct SpeakerConfig {
    /// Model id used by [`ModelCache`](crate::models::ModelCache) to locate the
    /// `.onnx` file.
    pub model_id: String,
    /// Number of ONNX Runtime threads.
    pub num_threads: i32,
    /// ONNX execution provider.
    pub provider: ExecutionProvider,
    /// Cosine-similarity threshold for verification.
    pub verify_threshold: f32,
    /// Optional directory for persisted voiceprints. When empty, a default
    /// per-platform data directory is used.
    pub voice_dir: Option<String>,
}

impl Default for SpeakerConfig {
    fn default() -> Self {
        Self {
            model_id: DEFAULT_SPEAKER_MODEL_ID.to_string(),
            num_threads: 1,
            provider: ExecutionProvider::Cpu,
            verify_threshold: DEFAULT_VERIFY_THRESHOLD,
            voice_dir: None,
        }
    }
}

impl SpeakerConfig {
    /// Create a config with the default model id.
    pub fn new() -> Self {
        Self::default()
    }

    /// Builder-style setter for the model id.
    pub fn with_model_id(mut self, model_id: impl Into<String>) -> Self {
        self.model_id = model_id.into();
        self
    }

    /// Builder-style setter for the number of threads.
    pub fn with_num_threads(mut self, num_threads: i32) -> Self {
        self.num_threads = num_threads;
        self
    }

    /// Builder-style setter for the execution provider.
    pub fn with_provider(mut self, provider: ExecutionProvider) -> Self {
        self.provider = provider;
        self
    }

    /// Builder-style setter for the verification threshold.
    pub fn with_verify_threshold(mut self, threshold: f32) -> Self {
        self.verify_threshold = threshold;
        self
    }

    /// Builder-style setter for the voiceprint storage directory.
    pub fn with_voice_dir(mut self, voice_dir: impl Into<String>) -> Self {
        self.voice_dir = Some(voice_dir.into());
        self
    }
}

/// Result of a single speaker verification attempt.
#[derive(Debug, Clone, PartialEq)]
pub struct VerificationResult {
    /// Cosine similarity between the test sample and the enrolled speaker.
    pub score: f32,
    /// Whether the sample was accepted against the configured threshold.
    pub accepted: bool,
    /// Name of the enrolled speaker, if any.
    pub speaker: Option<String>,
}

/// Errors from speaker verification operations.
#[derive(Debug, Error)]
pub enum SpeakerError {
    #[error("speaker model file not found: {0}")]
    ModelNotFound(String),
    #[error("failed to create speaker embedding extractor")]
    CreateExtractor,
    #[error("failed to create speaker embedding manager")]
    CreateManager,
    #[error("failed to create audio stream")]
    CreateStream,
    #[error("audio too short to compute a speaker embedding")]
    InputTooShort,
    #[error("failed to compute embedding")]
    ComputeEmbedding,
    #[error("no enrolled speakers")]
    NoSpeakers,
    #[error("speaker not enrolled: {0}")]
    SpeakerNotFound(String),
    #[error("enrollment failed")]
    EnrollFailed,
    #[error("invalid speaker name: {0}")]
    InvalidName(String),
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("persistence error: {0}")]
    Persist(String),
}

#[cfg(feature = "sherpa")]
pub use sherpa_onnx::SpeakerEmbeddingMatch;
#[cfg(feature = "sherpa")]
use sherpa_onnx::{
    SpeakerEmbeddingExtractor, SpeakerEmbeddingExtractorConfig, SpeakerEmbeddingManager,
};

/// On-disk representation of a single enrolled speaker embedding.
#[cfg(feature = "sherpa")]
#[derive(Debug, Clone, Serialize, Deserialize)]
struct StoredVoiceprint {
    version: u32,
    created_at: u64,
    embedding: Vec<f32>,
}

#[cfg(feature = "sherpa")]
impl StoredVoiceprint {
    const VERSION: u32 = 1;

    fn new(embedding: Vec<f32>) -> Self {
        let created_at = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        Self {
            version: Self::VERSION,
            created_at,
            embedding,
        }
    }
}

#[cfg(feature = "sherpa")]
fn voiceprint_path(voice_dir: &Path, name: &str) -> PathBuf {
    voice_dir.join(format!("{name}.json"))
}

#[cfg(feature = "sherpa")]
fn validate_speaker_name(name: &str) -> Result<(), SpeakerError> {
    if name.is_empty() {
        return Err(SpeakerError::InvalidName(
            "speaker name must not be empty".to_string(),
        ));
    }
    if name == "." || name == ".." {
        return Err(SpeakerError::InvalidName(format!(
            "speaker name must not be '{name}'"
        )));
    }
    if name
        .bytes()
        .any(|b| b == b'/' || b == b'\\' || b == b'\0' || b < 0x20)
    {
        return Err(SpeakerError::InvalidName(format!(
            "speaker name contains path separator or control character: {name}"
        )));
    }
    Ok(())
}

/// Extracts speaker embeddings and verifies them against enrolled voiceprints.
///
/// The verifier is thread-safe (`Send + Sync`) and can be shared across an
/// async runtime via `std::sync::Arc`.
#[cfg(feature = "sherpa")]
pub struct SpeakerVerifier {
    extractor: SpeakerEmbeddingExtractor,
    manager: SpeakerEmbeddingManager,
    config: SpeakerConfig,
    voice_dir: Option<PathBuf>,
}

#[cfg(feature = "sherpa")]
impl SpeakerVerifier {
    /// Create a verifier from a model file and optional persistent voiceprint
    /// directory.
    ///
    /// When `voice_dir` is provided and exists, existing `*.json` voiceprints
    /// are loaded into the manager. New enrollments are written there and
    /// deletions remove the corresponding file.
    pub fn new(
        config: SpeakerConfig,
        model_path: impl AsRef<Path>,
        voice_dir: Option<PathBuf>,
    ) -> Result<Self, SpeakerError> {
        let model_path = model_path.as_ref();
        if !model_path.exists() {
            return Err(SpeakerError::ModelNotFound(
                model_path.display().to_string(),
            ));
        }

        let extractor_config = SpeakerEmbeddingExtractorConfig {
            model: Some(model_path.to_string_lossy().to_string()),
            num_threads: config.num_threads,
            debug: false,
            provider: Some(config.provider.to_string()),
        };

        let extractor = SpeakerEmbeddingExtractor::create(&extractor_config)
            .ok_or(SpeakerError::CreateExtractor)?;
        let dim = extractor.dim();
        let manager = SpeakerEmbeddingManager::create(dim).ok_or(SpeakerError::CreateManager)?;

        let verifier = Self {
            extractor,
            manager,
            config,
            voice_dir,
        };

        if let Some(dir) = &verifier.voice_dir {
            verifier.load_stored_voiceprints(dir)?;
        }

        Ok(verifier)
    }

    /// Return the current configuration.
    pub fn config(&self) -> &SpeakerConfig {
        &self.config
    }

    /// Extract an embedding from a 16 kHz mono f32 buffer.
    pub fn extract(&self, samples: &[f32]) -> Result<Vec<f32>, SpeakerError> {
        self.extract_with_rate(samples, SHERPA_SAMPLE_RATE as i32)
    }

    /// Extract an embedding from audio at an explicit sample rate.
    pub fn extract_with_rate(
        &self,
        samples: &[f32],
        sample_rate: i32,
    ) -> Result<Vec<f32>, SpeakerError> {
        let min_samples = (MIN_SPEAKER_AUDIO_SECONDS * sample_rate as f32) as usize;
        if samples.len() < min_samples {
            return Err(SpeakerError::InputTooShort);
        }

        let stream = self
            .extractor
            .create_stream()
            .ok_or(SpeakerError::CreateStream)?;
        stream.accept_waveform(sample_rate, samples);
        stream.input_finished();

        if !self.extractor.is_ready(&stream) {
            return Err(SpeakerError::InputTooShort);
        }

        self.extractor
            .compute(&stream)
            .ok_or(SpeakerError::ComputeEmbedding)
    }

    /// Enroll a single speaker from one audio clip.
    pub fn enroll(&self, name: &str, samples: &[f32]) -> Result<(), SpeakerError> {
        validate_speaker_name(name)?;
        let embedding = self.extract(samples)?;
        self.add_embedding(name, embedding)
    }

    /// Enroll a speaker from multiple audio clips.
    ///
    /// The embeddings are averaged, normalized, and stored as a single
    /// voiceprint so reloaded embeddings match the averaged enrollment.
    pub fn enroll_list(&self, name: &str, samples_list: &[&[f32]]) -> Result<(), SpeakerError> {
        validate_speaker_name(name)?;
        let embeddings: Vec<Vec<f32>> = samples_list
            .iter()
            .map(|s| self.extract(s))
            .collect::<Result<Vec<_>, _>>()?;

        if embeddings.is_empty() {
            return Err(SpeakerError::InputTooShort);
        }

        self.add_list(name, embeddings)
    }

    /// Verify a test sample against a named enrolled speaker.
    pub fn verify(&self, name: &str, samples: &[f32]) -> Result<VerificationResult, SpeakerError> {
        validate_speaker_name(name)?;
        if !self.manager.contains(name) {
            return Err(SpeakerError::SpeakerNotFound(name.to_string()));
        }

        let embedding = self.extract(samples)?;
        let num_speakers = self.manager.num_speakers();
        let matches = self.manager.get_best_matches(&embedding, 0.0, num_speakers);

        let matched = matches.into_iter().find(|m| m.name == name);
        let score = matched.map_or(-1.0, |m| m.score);
        let accepted = score >= self.config.verify_threshold;

        Ok(VerificationResult {
            score,
            accepted,
            speaker: Some(name.to_string()),
        })
    }

    /// Search for the best matching enrolled speaker.
    pub fn search(&self, samples: &[f32]) -> Result<Option<SpeakerEmbeddingMatch>, SpeakerError> {
        let embedding = self.extract(samples)?;
        let num_speakers = self.manager.num_speakers();
        if num_speakers == 0 {
            return Ok(None);
        }
        Ok(self
            .manager
            .get_best_matches(&embedding, 0.0, num_speakers)
            .into_iter()
            .next())
    }

    /// Return the best matching speaker and score, or an error if none are enrolled.
    pub fn score(&self, samples: &[f32]) -> Result<SpeakerEmbeddingMatch, SpeakerError> {
        self.search(samples)?.ok_or(SpeakerError::NoSpeakers)
    }

    /// Remove all embeddings for a speaker and delete the on-disk voiceprint if
    /// persistence is enabled.
    pub fn remove(&self, name: &str) -> Result<(), SpeakerError> {
        validate_speaker_name(name)?;
        self.manager.remove(name);
        if let Some(dir) = &self.voice_dir {
            let path = voiceprint_path(dir, name);
            if path.exists() {
                std::fs::remove_file(&path)?;
            }
        }
        Ok(())
    }

    /// List all enrolled speaker names.
    pub fn list(&self) -> Vec<String> {
        self.manager.get_all_speakers()
    }

    /// Check whether a speaker is enrolled.
    pub fn is_enrolled(&self, name: &str) -> bool {
        self.manager.contains(name)
    }

    /// Number of enrolled speakers.
    pub fn num_speakers(&self) -> i32 {
        self.manager.num_speakers()
    }

    fn add_embedding(&self, name: &str, embedding: Vec<f32>) -> Result<(), SpeakerError> {
        self.manager.remove(name);
        if !self.manager.add(name, &embedding) {
            return Err(SpeakerError::EnrollFailed);
        }
        if let Some(dir) = &self.voice_dir {
            self.save_voiceprint(dir, name, &embedding)?;
        }
        Ok(())
    }

    fn add_list(&self, name: &str, embeddings: Vec<Vec<f32>>) -> Result<(), SpeakerError> {
        let average = Self::average_embeddings(&embeddings)?;
        self.add_embedding(name, average)
    }

    fn average_embeddings(embeddings: &[Vec<f32>]) -> Result<Vec<f32>, SpeakerError> {
        let first = embeddings.first().ok_or(SpeakerError::InputTooShort)?;
        let dim = first.len();
        let mut sum = vec![0.0f32; dim];
        for emb in embeddings {
            for (i, v) in emb.iter().enumerate() {
                sum[i] += v;
            }
        }
        let count = embeddings.len() as f32;
        for v in &mut sum {
            *v /= count;
        }
        let norm = sum.iter().map(|v| v * v).sum::<f32>().sqrt();
        if norm > 0.0 {
            for v in &mut sum {
                *v /= norm;
            }
        }
        Ok(sum)
    }

    fn save_voiceprint(
        &self,
        voice_dir: &Path,
        name: &str,
        embedding: &[f32],
    ) -> Result<(), SpeakerError> {
        std::fs::create_dir_all(voice_dir)?;
        let path = voiceprint_path(voice_dir, name);
        let stored = StoredVoiceprint::new(embedding.to_vec());
        let json = serde_json::to_string_pretty(&stored)
            .map_err(|e| SpeakerError::Persist(format!("failed to serialize voiceprint: {e}")))?;
        std::fs::write(&path, json)?;
        Ok(())
    }

    fn load_stored_voiceprints(&self, voice_dir: &Path) -> Result<(), SpeakerError> {
        if !voice_dir.is_dir() {
            return Ok(());
        }

        for entry in std::fs::read_dir(voice_dir)? {
            let entry = entry?;
            let path = entry.path();
            if path.extension().and_then(|s| s.to_str()) != Some("json") {
                continue;
            }
            let name = path
                .file_stem()
                .and_then(|s| s.to_str())
                .ok_or_else(|| SpeakerError::Persist("invalid voiceprint file name".to_string()))?
                .to_string();

            let json = std::fs::read_to_string(&path)?;
            let stored: StoredVoiceprint = serde_json::from_str(&json).map_err(|e| {
                SpeakerError::Persist(format!("failed to parse {}: {e}", path.display()))
            })?;

            if !self.manager.add(&name, &stored.embedding) {
                tracing::warn!("failed to load stored voiceprint for {name}");
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn speaker_config_default() {
        let config = SpeakerConfig::default();
        assert_eq!(config.model_id, DEFAULT_SPEAKER_MODEL_ID);
        assert_eq!(config.num_threads, 1);
        assert_eq!(config.provider, ExecutionProvider::Cpu);
        assert_eq!(config.verify_threshold, DEFAULT_VERIFY_THRESHOLD);
        assert_eq!(config.voice_dir, None);
    }

    #[test]
    fn speaker_config_builders() {
        let config = SpeakerConfig::new()
            .with_model_id("custom")
            .with_num_threads(4)
            .with_provider(ExecutionProvider::CoreMl)
            .with_verify_threshold(0.7)
            .with_voice_dir("/tmp/v");
        assert_eq!(config.model_id, "custom");
        assert_eq!(config.num_threads, 4);
        assert_eq!(config.provider, ExecutionProvider::CoreMl);
        assert_eq!(config.verify_threshold, 0.7);
        assert_eq!(config.voice_dir, Some("/tmp/v".to_string()));
    }

    #[test]
    fn speaker_config_serde_roundtrip() {
        let config = SpeakerConfig::new().with_verify_threshold(0.6);
        let json = serde_json::to_string(&config).unwrap();
        let decoded: SpeakerConfig = serde_json::from_str(&json).unwrap();
        assert_eq!(config, decoded);
    }

    #[test]
    fn speaker_config_deser_default() {
        let decoded: SpeakerConfig = serde_json::from_str("{}").unwrap();
        assert_eq!(decoded, SpeakerConfig::default());
    }

    #[cfg(feature = "sherpa")]
    #[test]
    fn verifier_rejects_missing_model() {
        let config = SpeakerConfig::default();
        let result = SpeakerVerifier::new(config, "/nonexistent/model.onnx", None);
        assert!(matches!(result, Err(SpeakerError::ModelNotFound(_))));
    }
}
