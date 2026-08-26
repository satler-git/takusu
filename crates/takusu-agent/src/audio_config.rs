use serde::{Deserialize, Serialize};
use takusu_audio::{
    DEFAULT_ENERGY_THRESHOLD, ExecutionProvider, SHERPA_SAMPLE_RATE, SherpaOnnxModel,
    SpeakerConfig, SttBackend, TtsBackend,
};

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct AudioConfig {
    pub stt: SttConfig,
    pub tts: TtsConfig,
    /// Short spoken cues announced at selected surface transitions
    /// (for example a click-tone substitute when listening starts/ends).
    pub cues: CueConfig,
    /// Optional speaker verification configuration. When `Some`, the audio
    /// adapter will load a speaker embedding model and verify utterances
    /// against enrolled voiceprints.
    pub speaker: Option<SpeakerConfig>,
    /// Conversation-polish settings (WI-19): barge-in, AEC, and latency budget.
    pub barge_in: BargeInConfig,
    pub aec: takusu_audio::AecConfig,
    /// Voice-activity-detection settings for the energy fallback.
    pub vad: VadConfig,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct BargeInConfig {
    /// Whether to listen for voice interruptions during assistant TTS.
    #[serde(default = "default_barge_in_enabled")]
    pub enabled: bool,
    /// Whether to use the software NLMS echo canceller. When `false` the
    /// adapter relies on a platform-provided AEC or falls back to tap-to-stop.
    #[serde(default = "default_barge_in_use_aec")]
    pub use_aec: bool,
    /// Time to wait after TTS starts before barge-in can fire, in milliseconds,
    /// to avoid transients.
    #[serde(default = "default_barge_in_warm_up_ms")]
    pub warm_up_ms: u64,
    /// Estimated playback-to-microphone delay in milliseconds. The TTS
    /// reference is delayed by this amount so the AEC sees the echo at the
    /// same time as the microphone signal.
    #[serde(default = "default_barge_in_reference_delay_ms")]
    pub reference_delay_ms: u64,
    /// Whether to fall back to tap-to-stop when AEC is not in use. When true
    /// and `use_aec` is false, a loud utterance during assistant TTS will stop
    /// playback but will not be transcribed as a barge-in.
    #[serde(default = "default_barge_in_tap_to_stop")]
    pub tap_to_stop: bool,
    /// Whether to record and log latency checkpoints for each turn.
    #[serde(default = "default_barge_in_latency")]
    pub record_latency: bool,
}

impl Default for BargeInConfig {
    fn default() -> Self {
        Self {
            enabled: default_barge_in_enabled(),
            use_aec: default_barge_in_use_aec(),
            warm_up_ms: default_barge_in_warm_up_ms(),
            reference_delay_ms: default_barge_in_reference_delay_ms(),
            tap_to_stop: default_barge_in_tap_to_stop(),
            record_latency: default_barge_in_latency(),
        }
    }
}

fn default_barge_in_enabled() -> bool {
    false
}

fn default_barge_in_use_aec() -> bool {
    true
}

fn default_barge_in_warm_up_ms() -> u64 {
    300
}

fn default_barge_in_reference_delay_ms() -> u64 {
    0
}

fn default_barge_in_tap_to_stop() -> bool {
    true
}

fn default_barge_in_latency() -> bool {
    true
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct SttConfig {
    #[serde(default = "default_stt_backend")]
    pub backend: SttBackend,
    #[serde(default = "default_stt_language")]
    pub language: String,
    #[serde(default)]
    pub model_dir: String,
    #[serde(default = "default_stt_model")]
    pub model: SherpaOnnxModel,
    #[serde(default = "default_stt_use_itn")]
    pub use_itn: bool,
    #[serde(default = "default_stt_num_threads")]
    pub num_threads: i32,
    #[serde(default = "default_stt_provider")]
    pub provider: ExecutionProvider,
    #[serde(default = "default_stt_sample_rate")]
    pub sample_rate: i32,
}

impl Default for SttConfig {
    fn default() -> Self {
        Self {
            backend: default_stt_backend(),
            language: default_stt_language(),
            model_dir: String::new(),
            model: default_stt_model(),
            use_itn: default_stt_use_itn(),
            num_threads: default_stt_num_threads(),
            provider: default_stt_provider(),
            sample_rate: default_stt_sample_rate(),
        }
    }
}

fn default_stt_backend() -> SttBackend {
    SttBackend::Sherpa
}
fn default_stt_language() -> String {
    "ja".into()
}
fn default_stt_model() -> SherpaOnnxModel {
    SherpaOnnxModel::SenseVoice
}
fn default_stt_use_itn() -> bool {
    true
}
fn default_stt_num_threads() -> i32 {
    2
}
fn default_stt_provider() -> ExecutionProvider {
    ExecutionProvider::Cpu
}
fn default_stt_sample_rate() -> i32 {
    SHERPA_SAMPLE_RATE as i32
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct TtsConfig {
    #[serde(default = "default_tts_backend")]
    pub backend: TtsBackend,
    #[serde(default = "default_tts_api_key_env")]
    pub api_key_env: String,
    #[serde(default)]
    pub api_key: String,
    #[serde(default = "default_tts_voice_id")]
    pub voice_id: String,
    #[serde(default = "default_tts_language")]
    pub language: String,
    #[serde(default = "default_tts_sample_rate")]
    pub sample_rate: u32,
    #[serde(default)]
    pub model: String,
    pub speed: Option<f32>,
    #[serde(default)]
    pub mute: bool,
    /// Maximum number of TTS synthesis requests allowed in flight. Must stay
    /// at or below the provider's concurrency limit to avoid HTTP 429.
    #[serde(default = "default_tts_max_concurrent")]
    pub max_concurrent: usize,
}

impl Default for TtsConfig {
    fn default() -> Self {
        Self {
            backend: default_tts_backend(),
            api_key_env: default_tts_api_key_env(),
            api_key: String::new(),
            voice_id: default_tts_voice_id(),
            language: default_tts_language(),
            sample_rate: default_tts_sample_rate(),
            model: String::new(),
            speed: None,
            mute: false,
            max_concurrent: default_tts_max_concurrent(),
        }
    }
}

fn default_tts_backend() -> TtsBackend {
    TtsBackend::Cartesia
}
fn default_tts_api_key_env() -> String {
    "CARTESIA_API_KEY".into()
}
fn default_tts_voice_id() -> String {
    "db6b0ed5-d5d3-463d-ae85-518a07d3c2b4".into()
}
fn default_tts_language() -> String {
    "ja".into()
}
fn default_tts_sample_rate() -> u32 {
    44100
}
fn default_tts_max_concurrent() -> usize {
    2
}

/// Spoken cues announced at selected surface transitions, synthesized via the
/// configured TTS backend and cached so repeated announcements do not
/// re-synthesize.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct CueConfig {
    /// Whether to announce listening start/end cues at all.
    #[serde(default = "default_cues_enabled")]
    pub enabled: bool,
    /// Spoken text announced when listening starts. Empty disables the cue.
    #[serde(default = "default_cue_listen_start")]
    pub listen_start: String,
    /// Spoken text announced when listening ends. Empty disables the cue.
    #[serde(default = "default_cue_listen_end")]
    pub listen_end: String,
}

impl Default for CueConfig {
    fn default() -> Self {
        Self {
            enabled: default_cues_enabled(),
            listen_start: default_cue_listen_start(),
            listen_end: default_cue_listen_end(),
        }
    }
}

fn default_cues_enabled() -> bool {
    true
}
fn default_cue_listen_start() -> String {
    "聞き取り開始".into()
}
fn default_cue_listen_end() -> String {
    "聞き取り終了".into()
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct VadConfig {
    /// RMS energy threshold for the fallback energy VAD. A frame is voiced
    /// when its RMS is at or above this value in the `[-1, 1]` sample range.
    #[serde(default = "default_vad_energy_threshold")]
    pub energy_threshold: f32,
}

impl Default for VadConfig {
    fn default() -> Self {
        Self {
            energy_threshold: default_vad_energy_threshold(),
        }
    }
}

fn default_vad_energy_threshold() -> f32 {
    DEFAULT_ENERGY_THRESHOLD
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cue_config_defaults_enable_announcements_and_provide_text() {
        let cues = CueConfig::default();
        assert!(cues.enabled);
        assert!(!cues.listen_start.is_empty());
        assert!(!cues.listen_end.is_empty());
    }

    #[test]
    fn cue_config_serde_uses_defaults_for_missing_fields() {
        let parsed: CueConfig = toml::from_str("").unwrap();
        assert_eq!(parsed, CueConfig::default());

        let full: CueConfig =
            toml::from_str("enabled = true\nlisten_start = \"開始\"\nlisten_end = \"終了\"")
                .unwrap();
        assert!(full.enabled);
        assert_eq!(full.listen_start, "開始");
        assert_eq!(full.listen_end, "終了");
    }
}
