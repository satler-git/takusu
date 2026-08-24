use std::path::PathBuf;

use takusu_audio::{DEFAULT_SPEAKER_MODEL_ID, SpeakerConfig, SpeakerError, SpeakerVerifier};

use crate::TakusuError;

/// On-device speaker verifier for Android.
///
/// Wraps `takusu_audio::SpeakerVerifier` and exposes a small UniFFI surface
/// for enrollment, verification, and voiceprint management.
#[derive(uniffi::Object)]
pub struct MobileSpeaker {
    verifier: SpeakerVerifier,
}

#[derive(Debug, Clone, uniffi::Record)]
pub struct SpeakerVerifyResult {
    pub score: f32,
    pub accepted: bool,
    pub speaker: String,
}

#[uniffi::export]
impl MobileSpeaker {
    #[uniffi::constructor]
    pub fn new(model_dir: String, voice_dir: String, threshold: f32) -> Result<Self, TakusuError> {
        let model_dir = PathBuf::from(model_dir);
        let voice_dir = PathBuf::from(voice_dir);
        let model_path = model_dir.join(DEFAULT_SPEAKER_MODEL_ID).join("model.onnx");

        if !model_path.is_file() {
            return Err(TakusuError::Model {
                detail: format!("speaker model not found at {}", model_path.display()),
            });
        }

        let config = SpeakerConfig {
            model_id: DEFAULT_SPEAKER_MODEL_ID.to_string(),
            num_threads: 1,
            provider: takusu_audio::ExecutionProvider::Cpu,
            verify_threshold: threshold,
            voice_dir: None,
        };

        let verifier = SpeakerVerifier::new(config, &model_path, Some(voice_dir))
            .map_err(map_speaker_error)?;

        Ok(Self { verifier })
    }

    /// Enroll a single 16 kHz mono f32 utterance for `name`.
    pub fn enroll(&self, name: String, samples: Vec<f32>) -> Result<(), TakusuError> {
        self.verifier
            .enroll(&name, &samples)
            .map_err(map_speaker_error)
    }

    /// Enroll from multiple 16 kHz mono f32 utterances, averaging embeddings.
    pub fn enroll_list(
        &self,
        name: String,
        samples_list: Vec<Vec<f32>>,
    ) -> Result<(), TakusuError> {
        let refs: Vec<&[f32]> = samples_list.iter().map(|v| v.as_slice()).collect();
        self.verifier
            .enroll_list(&name, &refs)
            .map_err(map_speaker_error)
    }

    /// Verify a single 16 kHz mono f32 utterance against `name`.
    pub fn verify(
        &self,
        name: String,
        samples: Vec<f32>,
    ) -> Result<SpeakerVerifyResult, TakusuError> {
        let result = self
            .verifier
            .verify(&name, &samples)
            .map_err(map_speaker_error)?;
        Ok(SpeakerVerifyResult {
            score: result.score,
            accepted: result.accepted,
            speaker: result.speaker.unwrap_or_default(),
        })
    }

    /// Delete the voiceprint for `name`.
    pub fn delete(&self, name: String) -> Result<(), TakusuError> {
        self.verifier.remove(&name).map_err(map_speaker_error)
    }

    /// List enrolled speaker names.
    pub fn list(&self) -> Vec<String> {
        self.verifier.list()
    }

    /// Check whether a voiceprint for `name` is enrolled.
    pub fn is_enrolled(&self, name: String) -> bool {
        self.verifier.is_enrolled(&name)
    }
}

fn map_speaker_error(error: SpeakerError) -> TakusuError {
    TakusuError::Audio {
        detail: error.to_string(),
    }
}
