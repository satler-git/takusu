pub mod aec;
pub mod cartesia;
pub mod fish;
mod http;
pub mod latency;
pub mod models;
#[cfg(feature = "record")]
pub mod play;
#[cfg(feature = "record")]
pub mod record;
#[cfg(feature = "record")]
pub mod record_streaming;
pub mod secrets;
pub mod speaker;
pub mod stt;
pub mod tts;
pub mod tts_normalize;
pub mod vad;
pub mod wav;

#[cfg(feature = "hush")]
pub mod hush;

#[cfg(feature = "sherpa")]
pub mod sherpa;

#[cfg(feature = "hush")]
pub use hush::Hush;
#[cfg(feature = "sherpa")]
pub use sherpa::{OfflineAsrStream, SherpaOnnxAsr, SherpaOnnxAsrConfig, SherpaOnnxStreamingAsr};

pub use aec::{Aec, AecConfig, AecEffectiveness, BargeInDetector, NlmsAec, NoOpAec, evaluate_aec};
pub use cartesia::{
    CartesiaContainer, CartesiaEmotion, CartesiaEncoding, CartesiaGenerationConfig,
    CartesiaOutputFormat, CartesiaSonic, CartesiaSonicConfig,
};
pub use fish::{FishAudio, FishAudioConfig};
pub use latency::{LatencyBudget, LatencyCheckpoint};
pub use models::{
    DownloadProgress, DownloadStage, ModelCache, ModelError, ModelRegistry, ModelSpec,
    ProgressCallback,
};
#[cfg(feature = "record")]
pub use record::{RecordConfig, RecorderError, record};
#[cfg(feature = "record")]
pub use record_streaming::StreamingRecorder;
pub use secrets::{ApiKey, EndpointUrl, EndpointUrlError};
pub use speaker::{
    DEFAULT_SPEAKER_MODEL_ID, DEFAULT_VERIFY_THRESHOLD, MIN_SPEAKER_AUDIO_SECONDS, SpeakerConfig,
    SpeakerError, VerificationResult,
};
#[cfg(feature = "sherpa")]
pub use speaker::{SpeakerEmbeddingMatch, SpeakerVerifier};
pub use stt::{
    AsrStream, ExecutionProvider, SherpaOnnxModel, SpeechToText, StreamingSpeechToText, SttBackend,
    SttError, SttRuntimeConfig,
};
pub use tts::{TextToSpeech, TtsBackend, TtsConfig, TtsError, TtsOptions, TtsRequest, TtsStream};
pub use tts_normalize::normalize_for_tts;
pub use vad::{
    DEFAULT_ENERGY_THRESHOLD, Endpoint, EnergyVad, VadEndpoint, VadEndpointConfig, VadEvent,
    VoiceActivity, default_endpoint, default_endpoint_with_config,
};

#[cfg(feature = "record")]
pub use vad::{default_endpoint_async, default_endpoint_async_with_config};

#[cfg(feature = "sherpa")]
pub use vad::{SileroEndpoint, silero_endpoint_from_cache};
pub use wav::{
    AudioError, I16_MAX_F32, SHERPA_SAMPLE_RATE, mix_to_mono, normalize, read_wav, resample,
    write_wav,
};
