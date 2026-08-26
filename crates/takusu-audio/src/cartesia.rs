//! Cartesia Sonic streaming text-to-speech backend.
//!
//! Uses Cartesia's `POST /tts/bytes` endpoint to stream audio for a complete
//! transcript. The response body is a stream of raw bytes (e.g. WAV) that is
//! exposed as a [`TtsStream`](crate::tts::TtsStream).

use std::sync::Arc;

use serde::{Deserialize, Serialize};
use takusu_types::enum_label;
use tokio::sync::Semaphore;

use crate::secrets::{ApiKey, EndpointUrl};
use crate::tts::{
    MAX_TTS_ATTEMPTS, TextToSpeech, TtsError, TtsRequest, TtsStream, acquire_permit,
    is_transient_tts_status, parse_retry_after, stream_with_permit, tts_retry_delay,
};

const DEFAULT_URL: &str = "https://api.cartesia.ai/tts/bytes";
const DEFAULT_VERSION: &str = "2026-03-01";
const DEFAULT_MODEL_ID: &str = "sonic-3.5";
const DEFAULT_VOICE_ID: &str = "db6b0ed5-d5d3-463d-ae85-518a07d3c2b4";
/// Default cap on in-flight synthesis requests, kept at or below typical free
/// tier limits so unrelated requests do not trip the provider's 429.
const DEFAULT_MAX_CONCURRENT: usize = 2;

enum_label! {
    /// Audio container format for Cartesia Sonic.
    ///
    /// See <https://docs.cartesia.ai/api-reference/tts/bytes> for the list of
    /// supported containers.
    pub enum CartesiaContainer {
        #[default] Wav = "wav",
        Raw = "raw",
        Mp3 = "mp3",
    }
}

enum_label! {
    /// PCM encoding for Cartesia Sonic raw/wav output.
    ///
    /// Cartesia accepts a fixed set of PCM encodings; this enum prevents
    /// arbitrary strings from reaching the API. The wire labels are defined
    /// once via `enum_label!` and shared by `Display`, `FromStr`, and serde.
    pub enum CartesiaEncoding {
        #[default] PcmS16Le = "pcm_s16le",
        PcmF32Le = "pcm_f32le",
        PcmMulaw = "pcm_mulaw",
        PcmAlaw = "pcm_alaw",
    }
}

impl CartesiaEncoding {
    /// Parse an encoding from its Cartesia wire string (case-insensitive).
    ///
    /// The `enum_label!`-generated `FromStr` is case-sensitive (labels are
    /// lowercase), so we lower-case the input first.
    pub fn from_wire_str(s: &str) -> Option<Self> {
        s.to_lowercase().parse().ok()
    }
}

enum_label! {
    /// Emotion for Cartesia Sonic generation.
    ///
    /// Cartesia only accepts `neutral` / `happy` / `sad` / `angry`; this enum
    /// prevents arbitrary strings from reaching the API.
    pub enum CartesiaEmotion {
        #[default] Neutral = "neutral",
        Happy = "happy",
        Sad = "sad",
        Angry = "angry",
    }
}

/// Audio output format for Cartesia Sonic.
#[derive(Debug, Clone, Serialize)]
pub struct CartesiaOutputFormat {
    pub container: CartesiaContainer,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub encoding: Option<CartesiaEncoding>,
    pub sample_rate: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bit_rate: Option<u32>,
}

impl Default for CartesiaOutputFormat {
    fn default() -> Self {
        Self {
            container: CartesiaContainer::Wav,
            encoding: Some(CartesiaEncoding::PcmS16Le),
            sample_rate: 44100,
            bit_rate: None,
        }
    }
}

impl CartesiaOutputFormat {
    /// Raw PCM output.
    pub fn raw(encoding: CartesiaEncoding, sample_rate: u32) -> Self {
        Self {
            container: CartesiaContainer::Raw,
            encoding: Some(encoding),
            sample_rate,
            bit_rate: None,
        }
    }

    /// WAV output.
    pub fn wav(encoding: CartesiaEncoding, sample_rate: u32) -> Self {
        Self {
            container: CartesiaContainer::Wav,
            encoding: Some(encoding),
            sample_rate,
            bit_rate: None,
        }
    }

    /// MP3 output.
    pub fn mp3(sample_rate: u32, bit_rate: u32) -> Self {
        Self {
            container: CartesiaContainer::Mp3,
            encoding: None,
            sample_rate,
            bit_rate: Some(bit_rate),
        }
    }
}

/// Generation configuration (speed, volume, emotion).
#[derive(Debug, Clone, Default, Serialize)]
pub struct CartesiaGenerationConfig {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub volume: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub speed: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub emotion: Option<CartesiaEmotion>,
}

/// Configuration for the Cartesia Sonic TTS backend.
#[derive(Debug, Clone)]
pub struct CartesiaSonicConfig {
    pub api_key: ApiKey,
    pub url: EndpointUrl,
    pub version: String,
    pub model_id: String,
    pub voice_id: String,
    pub language: Option<String>,
    pub output_format: CartesiaOutputFormat,
    pub generation_config: Option<CartesiaGenerationConfig>,
    pub mute: bool,
    /// Maximum number of synthesis requests allowed in flight. This must stay
    /// below the provider's concurrency limit (typically 2 on the free tier) so
    /// the agent's parallel block synthesis does not trip HTTP 429.
    pub max_concurrent: usize,
}

impl Default for CartesiaSonicConfig {
    fn default() -> Self {
        Self {
            api_key: ApiKey::default(),
            url: EndpointUrl::new(DEFAULT_URL).expect("DEFAULT_URL is a valid URL"),
            version: DEFAULT_VERSION.to_string(),
            model_id: DEFAULT_MODEL_ID.to_string(),
            voice_id: DEFAULT_VOICE_ID.to_string(),
            language: None,
            output_format: CartesiaOutputFormat::default(),
            generation_config: None,
            mute: false,
            max_concurrent: DEFAULT_MAX_CONCURRENT,
        }
    }
}

impl CartesiaSonicConfig {
    /// Create a config with the given API key and otherwise default settings.
    pub fn new(api_key: impl Into<ApiKey>) -> Self {
        Self {
            api_key: api_key.into(),
            ..Self::default()
        }
    }

    /// Create a config from the environment, reading `CARTESIA_API_KEY`.
    pub fn from_env() -> Result<Self, TtsError> {
        let api_key = std::env::var("CARTESIA_API_KEY").map_err(|_| TtsError::Api {
            status: 401,
            message: "CARTESIA_API_KEY environment variable not set".to_string(),
        })?;
        Ok(Self::new(api_key))
    }
}

/// Cartesia Sonic TTS client.
#[derive(Debug, Clone)]
pub struct CartesiaSonic {
    client: reqwest::Client,
    config: CartesiaSonicConfig,
    /// Bounds in-flight synthesis requests to `config.max_concurrent`.
    concurrency: Arc<Semaphore>,
}

impl CartesiaSonic {
    /// Create a new client from the given config.
    pub fn new(config: CartesiaSonicConfig) -> Self {
        let client = crate::http::tls_client();
        let concurrency = Arc::new(Semaphore::new(config.max_concurrent));
        Self {
            client,
            config,
            concurrency,
        }
    }

    /// Create a new client from the environment, reading `CARTESIA_API_KEY`.
    pub fn from_env() -> Result<Self, TtsError> {
        Ok(Self::new(CartesiaSonicConfig::from_env()?))
    }
}

#[async_trait::async_trait]
impl TextToSpeech for CartesiaSonic {
    async fn synthesize_stream(&self, request: &TtsRequest) -> Result<TtsStream, TtsError> {
        if self.config.mute {
            return Ok(Box::pin(futures_util::stream::empty()));
        }
        if self.config.api_key.is_empty() {
            return Err(TtsError::Api {
                status: 401,
                message: "missing Cartesia API key".to_string(),
            });
        }

        let voice_id = request.voice.as_deref().unwrap_or(&self.config.voice_id);
        let output_format = output_format_for_request(&self.config.output_format, request);

        let mut generation_config = self.config.generation_config.clone();
        if let Some(speed) = request.options.speed {
            let mut gc = generation_config.unwrap_or_default();
            gc.speed = Some(speed);
            generation_config = Some(gc);
        }

        let body = TtsBytesRequest {
            model_id: &self.config.model_id,
            transcript: &request.text,
            voice: VoiceSpecifier {
                mode: "id",
                id: voice_id,
            },
            output_format: &output_format,
            language: self.config.language.as_deref(),
            generation_config: generation_config.as_ref(),
            pronunciation_dict_id: None,
        };

        let json = serde_json::to_vec(&body)?;

        // Cap in-flight synthesis to stay under the provider's concurrency
        // limit and retry transient failures (429 / 5xx) with backoff instead
        // of failing the whole block.
        let permit = acquire_permit(&self.concurrency).await?;
        let mut attempt = 0u32;
        loop {
            attempt += 1;
            let response = self
                .client
                .post(self.config.url.as_str())
                .header("Authorization", format!("Bearer {}", self.config.api_key))
                .header("Cartesia-Version", &self.config.version)
                .header("Content-Type", "application/json")
                .body(json.clone())
                .send()
                .await?;

            let status = response.status();
            if status.is_success() {
                let stream = response.bytes_stream();
                return Ok(stream_with_permit(stream, permit));
            }

            if !is_transient_tts_status(status.as_u16()) || attempt >= MAX_TTS_ATTEMPTS {
                let body_text = response.text().await.unwrap_or_default();
                let message = parse_error_message(&body_text);
                return Err(TtsError::Api {
                    status: status.as_u16(),
                    message,
                });
            }

            let retry_after = parse_retry_after(response.headers());
            tracing::warn!(
                status = %status,
                attempt,
                "cartesia tts rate limited; retrying after backoff"
            );
            tokio::time::sleep(tts_retry_delay(retry_after, attempt)).await;
        }
    }
}

#[derive(Debug, Serialize)]
struct VoiceSpecifier<'a> {
    mode: &'a str,
    id: &'a str,
}

#[derive(Debug, Serialize)]
struct TtsBytesRequest<'a> {
    model_id: &'a str,
    transcript: &'a str,
    voice: VoiceSpecifier<'a>,
    output_format: &'a CartesiaOutputFormat,
    #[serde(skip_serializing_if = "Option::is_none")]
    language: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    generation_config: Option<&'a CartesiaGenerationConfig>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pronunciation_dict_id: Option<&'a str>,
}

#[derive(Debug, Deserialize)]
struct CartesiaApiError {
    title: Option<String>,
    message: Option<String>,
}

fn parse_error_message(body: &str) -> String {
    if let Ok(error) = serde_json::from_str::<CartesiaApiError>(body) {
        if let Some(message) = error.message {
            return message;
        }
        if let Some(title) = error.title {
            return title;
        }
    }

    let trimmed = body.trim();
    if trimmed.is_empty() {
        "unknown Cartesia API error".to_string()
    } else {
        trimmed.to_string()
    }
}

fn output_format_for_request(
    config_format: &CartesiaOutputFormat,
    request: &TtsRequest,
) -> CartesiaOutputFormat {
    let Some(response_format) = request.options.response_format.as_deref() else {
        return config_format.clone();
    };

    // The response_format comes from the provider-neutral `TtsOptions` which
    // is shared across backends, so it stays a `String`. Here we map it to
    // Cartesia's typed container/encoding enums.
    let lower = response_format.to_lowercase();
    if let Some(encoding) = CartesiaEncoding::from_wire_str(&lower) {
        // Bare encoding names (pcm_s16le, pcm_f32le, ...) imply raw container.
        return CartesiaOutputFormat::raw(encoding, config_format.sample_rate);
    }
    match lower.as_str() {
        "wav" => CartesiaOutputFormat::wav(CartesiaEncoding::PcmS16Le, config_format.sample_rate),
        "mp3" => CartesiaOutputFormat::mp3(config_format.sample_rate, 128_000),
        "raw" => CartesiaOutputFormat::raw(CartesiaEncoding::PcmS16Le, config_format.sample_rate),
        _ => config_format.clone(),
    }
}

#[cfg(test)]
mod tests {
    use std::str::FromStr;

    use crate::tts::{TextToSpeech, TtsBackend, TtsOptions, TtsRequest};

    use super::*;

    #[test]
    fn tts_backend_parses_known_backends() {
        assert_eq!(
            TtsBackend::from_str("cartesia").unwrap(),
            TtsBackend::Cartesia
        );
        assert_eq!(
            TtsBackend::from_str("CARTESIA").unwrap(),
            TtsBackend::Cartesia
        );
        assert_eq!(
            TtsBackend::from_str("android").unwrap(),
            TtsBackend::Android
        );
        assert_eq!(
            TtsBackend::from_str("ANDROID").unwrap(),
            TtsBackend::Android
        );
        assert!(TtsBackend::from_str("unknown").is_err());
    }

    #[test]
    fn output_format_respects_response_format() {
        let config = CartesiaOutputFormat::default();
        let request = |format: &str| TtsRequest {
            text: "hello".to_string(),
            options: TtsOptions {
                response_format: Some(format.to_string()),
                ..Default::default()
            },
            ..Default::default()
        };

        let wav = output_format_for_request(&config, &request("wav"));
        assert_eq!(wav.container, CartesiaContainer::Wav);
        assert_eq!(wav.encoding, Some(CartesiaEncoding::PcmS16Le));

        let raw = output_format_for_request(&config, &request("raw"));
        assert_eq!(raw.container, CartesiaContainer::Raw);
        assert_eq!(raw.encoding, Some(CartesiaEncoding::PcmS16Le));

        let mp3 = output_format_for_request(&config, &request("mp3"));
        assert_eq!(mp3.container, CartesiaContainer::Mp3);
        assert_eq!(mp3.bit_rate, Some(128_000));

        let f32le = output_format_for_request(&config, &request("pcm_f32le"));
        assert_eq!(f32le.container, CartesiaContainer::Raw);
        assert_eq!(f32le.encoding, Some(CartesiaEncoding::PcmF32Le));
    }

    #[tokio::test]
    async fn muted_sonic_returns_empty_audio() {
        let config = CartesiaSonicConfig {
            mute: true,
            ..Default::default()
        };
        let tts = CartesiaSonic::new(config);
        let request = TtsRequest {
            text: "hello".to_string(),
            ..Default::default()
        };
        let audio = tts.synthesize(&request).await.unwrap();
        assert!(audio.is_empty());
    }

    // --- Characterization tests for JSON wire format ---
    //
    // These tests pin down the exact JSON sent to the Cartesia API so that
    // refactoring `container` / `encoding` / `emotion` from `String` to enums
    // does not change the on-the-wire representation.

    #[test]
    fn output_format_default_serializes_to_wav_pcm_s16le() {
        let format = CartesiaOutputFormat::default();
        let json = serde_json::to_value(&format).unwrap();
        assert_eq!(
            json,
            serde_json::json!({
                "container": "wav",
                "encoding": "pcm_s16le",
                "sample_rate": 44100,
            })
        );
    }

    #[test]
    fn output_format_mp3_skips_encoding_field() {
        let format = CartesiaOutputFormat::mp3(44100, 128_000);
        let json = serde_json::to_value(&format).unwrap();
        assert_eq!(
            json,
            serde_json::json!({
                "container": "mp3",
                "sample_rate": 44100,
                "bit_rate": 128_000,
            })
        );
        // encoding must not appear in the serialized output for mp3.
        assert!(json.get("encoding").is_none());
    }

    #[test]
    fn output_format_raw_serializes_encoding() {
        let format = CartesiaOutputFormat::raw(CartesiaEncoding::PcmF32Le, 16_000);
        let json = serde_json::to_value(&format).unwrap();
        assert_eq!(
            json,
            serde_json::json!({
                "container": "raw",
                "encoding": "pcm_f32le",
                "sample_rate": 16000,
            })
        );
        assert!(json.get("bit_rate").is_none());
    }

    #[test]
    fn output_format_wav_serializes_encoding() {
        let format = CartesiaOutputFormat::wav(CartesiaEncoding::PcmMulaw, 8_000);
        let json = serde_json::to_value(&format).unwrap();
        assert_eq!(
            json,
            serde_json::json!({
                "container": "wav",
                "encoding": "pcm_mulaw",
                "sample_rate": 8000,
            })
        );
    }

    #[test]
    fn generation_config_emotion_serializes_as_string() {
        let config = CartesiaGenerationConfig {
            emotion: Some(CartesiaEmotion::Happy),
            ..Default::default()
        };
        let json = serde_json::to_value(&config).unwrap();
        assert_eq!(
            json,
            serde_json::json!({
                "emotion": "happy",
            })
        );
    }

    #[test]
    fn generation_config_empty_skips_all_fields() {
        let config = CartesiaGenerationConfig::default();
        let json = serde_json::to_value(&config).unwrap();
        assert_eq!(json, serde_json::json!({}));
    }

    #[test]
    fn output_format_for_request_unknown_format_falls_back_to_config() {
        let config = CartesiaOutputFormat::default();
        let request = TtsRequest {
            text: "hello".to_string(),
            options: TtsOptions {
                response_format: Some("ogg".to_string()),
                ..Default::default()
            },
            ..Default::default()
        };
        let result = output_format_for_request(&config, &request);
        // Unknown formats fall back to the config format unchanged.
        assert_eq!(result.container, config.container);
        assert_eq!(result.encoding, config.encoding);
    }

    #[test]
    fn output_format_for_request_is_case_insensitive() {
        let config = CartesiaOutputFormat::default();
        let request = |format: &str| TtsRequest {
            text: "hello".to_string(),
            options: TtsOptions {
                response_format: Some(format.to_string()),
                ..Default::default()
            },
            ..Default::default()
        };

        let wav = output_format_for_request(&config, &request("WAV"));
        assert_eq!(wav.container, CartesiaContainer::Wav);
        assert_eq!(wav.encoding, Some(CartesiaEncoding::PcmS16Le));

        let mp3 = output_format_for_request(&config, &request("MP3"));
        assert_eq!(mp3.container, CartesiaContainer::Mp3);
    }

    #[test]
    fn cartesia_encoding_from_wire_str_roundtrips() {
        for encoding in [
            CartesiaEncoding::PcmS16Le,
            CartesiaEncoding::PcmF32Le,
            CartesiaEncoding::PcmMulaw,
            CartesiaEncoding::PcmAlaw,
        ] {
            let s = encoding.to_string();
            assert_eq!(CartesiaEncoding::from_wire_str(&s), Some(encoding));
            assert_eq!(
                CartesiaEncoding::from_wire_str(&s.to_uppercase()),
                Some(encoding),
            );
        }
        assert_eq!(CartesiaEncoding::from_wire_str("unknown"), None);
    }

    #[test]
    fn cartesia_emotion_serializes_to_lowercase() {
        assert_eq!(
            serde_json::to_value(CartesiaEmotion::Neutral).unwrap(),
            serde_json::json!("neutral"),
        );
        assert_eq!(
            serde_json::to_value(CartesiaEmotion::Happy).unwrap(),
            serde_json::json!("happy"),
        );
        assert_eq!(
            serde_json::to_value(CartesiaEmotion::Sad).unwrap(),
            serde_json::json!("sad"),
        );
        assert_eq!(
            serde_json::to_value(CartesiaEmotion::Angry).unwrap(),
            serde_json::json!("angry"),
        );
    }

    #[test]
    fn cartesia_container_serializes_to_lowercase() {
        assert_eq!(
            serde_json::to_value(CartesiaContainer::Wav).unwrap(),
            serde_json::json!("wav"),
        );
        assert_eq!(
            serde_json::to_value(CartesiaContainer::Raw).unwrap(),
            serde_json::json!("raw"),
        );
        assert_eq!(
            serde_json::to_value(CartesiaContainer::Mp3).unwrap(),
            serde_json::json!("mp3"),
        );
    }
}
