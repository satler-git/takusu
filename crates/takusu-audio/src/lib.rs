pub mod cartesia;
pub mod models;
#[cfg(feature = "record")]
pub mod play;
#[cfg(feature = "record")]
pub mod record;
pub mod stt;
pub mod tts;
pub mod tts_normalize;

#[cfg(feature = "hush")]
pub mod hush;

#[cfg(feature = "sherpa")]
pub mod sherpa;

#[cfg(feature = "hush")]
pub use hush::Hush;
#[cfg(feature = "sherpa")]
pub use sherpa::{SherpaOnnxAsr, SherpaOnnxAsrConfig, SherpaOnnxModel};

pub use cartesia::{
    CartesiaGenerationConfig, CartesiaOutputFormat, CartesiaSonic, CartesiaSonicConfig,
};
pub use models::{
    DownloadProgress, DownloadStage, ModelCache, ModelError, ModelRegistry, ModelSpec,
    ProgressCallback,
};
#[cfg(feature = "record")]
pub use record::{RecordConfig, RecorderError, record};
pub use stt::{SpeechToText, SttError};
pub use tts::{TextToSpeech, TtsBackend, TtsConfig, TtsError, TtsOptions, TtsRequest, TtsStream};
pub use tts_normalize::normalize_for_tts;
