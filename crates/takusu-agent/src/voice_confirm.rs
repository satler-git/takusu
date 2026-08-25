//! Server-side voice-approval confirmation.
//!
//! This service decides whether a captured PCM utterance is a verified "yes"
//! or "no" for a pending approval. It is intentionally not responsible for
//! resolving the approval itself: the client calls `resolve_approval` after
//! receiving an unambiguous `approve` / `deny` decision. This keeps the mobile
//! and desktop approval contract identical to the one in
//! `takusu-agent/src/audio.rs`.

use std::sync::Arc;

use crate::audio::AudioError;
use crate::audio_config::{AudioConfig, SttConfig};
use takusu_audio::{SpeakerConfig, SpeakerVerifier, StreamingSpeechToText};

const AFFIRMATIVE: &[&str] = &["はい", "うん", "yes", "ok", "おう"];
const NEGATIVE: &[&str] = &["いいえ", "いや", "no", "やだ", "キャンセル"];

const RETRY_PROMPT: &str = "もう一度お答えください。はい、または、いいえ、でお答えください。";

/// Outcome of one voice-approval attempt.
#[derive(Debug, Clone)]
pub enum VoiceDecision {
    Approve,
    Deny,
    Undecided,
}

/// Result returned to the client after submitting an utterance.
#[derive(Debug, Clone)]
pub struct VoiceConfirmResult {
    pub decision: VoiceDecision,
    pub transcript: String,
    pub score: f32,
    pub accepted: bool,
    pub speaker: Option<String>,
    /// Prompt the client should speak before the next recording, if any.
    pub prompt: Option<String>,
}

/// Shared service that loads STT and a speaker verifier from `AudioConfig` and
/// classifies voice approvals from a captured PCM utterance.
pub struct VoiceConfirmService {
    stt: Arc<dyn StreamingSpeechToText>,
    speaker: Option<Arc<SpeakerVerifier>>,
    config: AudioConfig,
}

impl VoiceConfirmService {
    /// Build the service from an `AudioConfig`. This downloads STT and speaker
    /// models on first call, so it should be run inside `spawn_blocking` or
    /// cached by the caller.
    pub async fn from_config(config: &AudioConfig) -> Result<Self, AudioError> {
        let stt = tokio::task::spawn_blocking({
            let config = config.stt.clone();
            move || crate::audio::build_stt(&config)
        })
        .await
        .map_err(|e| AudioError::Transcribe(format!("stt build task failed: {e}")))??;

        let speaker =
            crate::audio::AudioAdapter::build_speaker_verifier(config.speaker.as_ref()).await?;

        Ok(Self {
            stt,
            speaker,
            config: config.clone(),
        })
    }

    /// Classify a transcript and, if unambiguous, verify the speaker. Returns
    /// the decision and a retry prompt when the answer is ambiguous or the
    /// speaker is not accepted.
    ///
    /// Speaker-verification failures (too short, no speakers, embedding errors)
    /// are returned as `accepted: false` with a retry prompt, not as errors, so
    /// the client can ask the user to repeat their answer.
    pub async fn confirm(&self, samples: &[f32]) -> Result<VoiceConfirmResult, AudioError> {
        let transcript = self
            .stt
            .transcribe(samples)
            .await
            .map_err(|e| AudioError::Transcribe(e.to_string()))?;

        let normalized = normalize_voice_answer(&transcript);
        let is_yes = AFFIRMATIVE.iter().any(|w| normalized == *w);
        let is_no = NEGATIVE.iter().any(|w| normalized == *w);

        let mut result = VoiceConfirmResult {
            decision: VoiceDecision::Undecided,
            transcript,
            score: -1.0,
            accepted: false,
            speaker: None,
            prompt: Some(RETRY_PROMPT.to_string()),
        };

        if !is_yes && !is_no {
            return Ok(result);
        }

        // Speaker verification is required for both yes and no so a third
        // party cannot spoof an approval or a cancellation.
        if let Some(verifier) = &self.speaker {
            let min_samples = (takusu_audio::MIN_SPEAKER_AUDIO_SECONDS
                * self.config.stt.sample_rate as f32) as usize;
            if samples.len() < min_samples {
                return Ok(result);
            }

            match verifier.search(samples) {
                Ok(Some(m)) => {
                    result.score = m.score;
                    result.accepted = m.score >= verifier.config().verify_threshold;
                    result.speaker = Some(m.name.clone());
                }
                Ok(None) | Err(_) => {
                    result.accepted = false;
                }
            }
        } else {
            // No speaker verifier configured; we cannot safely decide.
            return Ok(result);
        }

        if !result.accepted {
            return Ok(result);
        }

        result.decision = if is_yes {
            VoiceDecision::Approve
        } else {
            VoiceDecision::Deny
        };
        result.prompt = None;

        Ok(result)
    }

    /// Access the current STT config for diagnostics.
    pub fn stt_config(&self) -> &SttConfig {
        &self.config.stt
    }

    /// Access the current speaker config, if any.
    pub fn speaker_config(&self) -> Option<&SpeakerConfig> {
        self.config.speaker.as_ref()
    }
}

/// Remove punctuation and whitespace from a voice answer, matching desktop.
pub fn normalize_voice_answer(text: &str) -> String {
    text.trim().to_lowercase().replace(
        |c: char| c.is_ascii_punctuation() || " 。、！？".contains(c),
        "",
    )
}
