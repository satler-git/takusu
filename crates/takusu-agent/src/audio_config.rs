use serde::Deserialize;
use takusu_audio::SHERPA_SAMPLE_RATE;

#[derive(Debug, Clone, Default, Deserialize, PartialEq)]
#[serde(default)]
pub struct AudioConfig {
    pub stt: SttConfig,
    pub tts: TtsConfig,
}

#[derive(Debug, Clone, Deserialize, PartialEq)]
#[serde(default)]
pub struct SttConfig {
    #[serde(default = "default_stt_backend")]
    pub backend: String,
    #[serde(default = "default_stt_language")]
    pub language: String,
    #[serde(default)]
    pub model_dir: String,
    #[serde(default = "default_stt_model")]
    pub model: String,
    #[serde(default = "default_stt_use_itn")]
    pub use_itn: bool,
    #[serde(default = "default_stt_num_threads")]
    pub num_threads: i32,
    #[serde(default = "default_stt_provider")]
    pub provider: String,
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

fn default_stt_backend() -> String {
    "sherpa".into()
}
fn default_stt_language() -> String {
    "ja".into()
}
fn default_stt_model() -> String {
    "sense-voice".into()
}
fn default_stt_use_itn() -> bool {
    true
}
fn default_stt_num_threads() -> i32 {
    2
}
fn default_stt_provider() -> String {
    "cpu".into()
}
fn default_stt_sample_rate() -> i32 {
    SHERPA_SAMPLE_RATE as i32
}

#[derive(Debug, Clone, Deserialize, PartialEq)]
#[serde(default)]
pub struct TtsConfig {
    #[serde(default = "default_tts_backend")]
    pub backend: String,
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
    pub speed: Option<f32>,
    #[serde(default)]
    pub mute: bool,
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
            speed: None,
            mute: false,
        }
    }
}

fn default_tts_backend() -> String {
    "cartesia".into()
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
