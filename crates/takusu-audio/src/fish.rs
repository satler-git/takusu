//! Fish Audio text-to-speech backend.
//!
//! Uses Fish Audio's `POST /v1/tts` endpoint. The model is selected via the
//! required `model` request header; the voice is supplied as a `reference_id`
//! in the JSON body. The endpoint returns raw audio bytes in the requested
//! format (mp3, wav, pcm, opus) as a chunked stream.

use futures_util::TryStreamExt;
use serde::{Deserialize, Serialize};

use crate::secrets::{ApiKey, EndpointUrl};
use crate::tts::{TextToSpeech, TtsError, TtsRequest, TtsStream};

const DEFAULT_URL: &str = "https://api.fish.audio/v1/tts";
const DEFAULT_MODEL: &str = "s2.1-pro-free";
const DEFAULT_VOICE_ID: &str = "";
const DEFAULT_SAMPLE_RATE: u32 = 44100;

/// Configuration for the Fish Audio TTS backend.
#[derive(Debug, Clone)]
pub struct FishAudioConfig {
    pub api_key: ApiKey,
    pub url: EndpointUrl,
    pub model: String,
    pub voice_id: String,
    pub sample_rate: u32,
    pub mute: bool,
}

impl Default for FishAudioConfig {
    fn default() -> Self {
        Self {
            api_key: ApiKey::default(),
            url: EndpointUrl::new(DEFAULT_URL).expect("DEFAULT_URL is a valid URL"),
            model: DEFAULT_MODEL.to_string(),
            voice_id: DEFAULT_VOICE_ID.to_string(),
            sample_rate: DEFAULT_SAMPLE_RATE,
            mute: false,
        }
    }
}

impl FishAudioConfig {
    /// Create a config with the given API key and otherwise default settings.
    pub fn new(api_key: impl Into<ApiKey>) -> Self {
        Self {
            api_key: api_key.into(),
            ..Self::default()
        }
    }

    /// Create a config from the environment, reading `FISH_API_KEY`.
    pub fn from_env() -> Result<Self, TtsError> {
        let api_key = std::env::var("FISH_API_KEY").map_err(|_| TtsError::Api {
            status: 401,
            message: "FISH_API_KEY environment variable not set".to_string(),
        })?;
        Ok(Self::new(api_key))
    }
}

/// Fish Audio TTS client.
#[derive(Debug, Clone)]
pub struct FishAudio {
    client: reqwest::Client,
    config: FishAudioConfig,
}

impl FishAudio {
    /// Create a new client from the given config.
    pub fn new(config: FishAudioConfig) -> Self {
        let client = crate::http::tls_client();
        Self { client, config }
    }

    /// Create a new client from the environment, reading `FISH_API_KEY`.
    pub fn from_env() -> Result<Self, TtsError> {
        Ok(Self::new(FishAudioConfig::from_env()?))
    }
}

#[async_trait::async_trait]
impl TextToSpeech for FishAudio {
    async fn synthesize_stream(&self, request: &TtsRequest) -> Result<TtsStream, TtsError> {
        if self.config.mute {
            return Ok(Box::pin(futures_util::stream::empty()));
        }
        if self.config.api_key.is_empty() {
            return Err(TtsError::Api {
                status: 401,
                message: "missing Fish Audio API key".to_string(),
            });
        }

        let model = if self.config.model.trim().is_empty() {
            DEFAULT_MODEL
        } else {
            &self.config.model
        };

        let (format, sample_rate) = output_format_for_request(self.config.sample_rate, request);

        let reference_id = request
            .voice
            .as_deref()
            .filter(|v| !v.is_empty())
            .or(Some(self.config.voice_id.as_str()))
            .filter(|v| !v.is_empty())
            .map(|v| v.to_string());

        let mut body = TtsRequestBody {
            text: request.text.clone(),
            reference_id,
            format: format.to_string(),
            sample_rate: Some(sample_rate),
            mp3_bitrate: None,
            prosody: None,
        };

        if format == "mp3" {
            body.mp3_bitrate = Some(128);
        }

        if let Some(speed) = request.options.speed {
            body.prosody = Some(ProsodyControl {
                speed: Some(speed),
                volume: None,
                normalize_loudness: None,
            });
        }

        let json = serde_json::to_vec(&body)?;
        let response = self
            .client
            .post(self.config.url.as_str())
            .header("Authorization", format!("Bearer {}", self.config.api_key))
            .header("Content-Type", "application/json")
            .header("model", model)
            .body(json)
            .send()
            .await?;

        let status = response.status();
        if !status.is_success() {
            let body_text = response.text().await.unwrap_or_default();
            let message = parse_error_message(&body_text);
            return Err(TtsError::Api {
                status: status.as_u16(),
                message,
            });
        }

        let stream = response.bytes_stream().map_err(TtsError::Http);
        Ok(Box::pin(stream))
    }
}

#[derive(Debug, Serialize)]
struct TtsRequestBody {
    text: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    reference_id: Option<String>,
    format: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    sample_rate: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    mp3_bitrate: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    prosody: Option<ProsodyControl>,
}

#[derive(Debug, Serialize)]
struct ProsodyControl {
    #[serde(skip_serializing_if = "Option::is_none")]
    speed: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    volume: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    normalize_loudness: Option<bool>,
}

#[derive(Debug, Deserialize)]
struct FishApiError {
    message: Option<String>,
}

fn parse_error_message(body: &str) -> String {
    if let Ok(error) = serde_json::from_str::<FishApiError>(body)
        && let Some(message) = error.message
    {
        return message;
    }

    let trimmed = body.trim();
    if trimmed.is_empty() {
        "unknown Fish Audio API error".to_string()
    } else {
        trimmed.to_string()
    }
}

fn output_format_for_request(sample_rate: u32, request: &TtsRequest) -> (&'static str, u32) {
    let lower = request
        .options
        .response_format
        .as_deref()
        .map(|s| s.to_lowercase())
        .unwrap_or_default();

    match lower.as_str() {
        "wav" => ("wav", supported_sample_rate(sample_rate)),
        "pcm" | "pcm_s16le" | "pcm_f32le" | "pcm_mulaw" | "pcm_alaw" => {
            ("pcm", supported_sample_rate(sample_rate))
        }
        "opus" => ("opus", 48000),
        // Default to mp3 for anything else, including "mp3" and mobile playback.
        _ => ("mp3", mp3_sample_rate(sample_rate)),
    }
}

fn supported_sample_rate(rate: u32) -> u32 {
    const SUPPORTED: &[u32] = &[8000, 16000, 24000, 32000, 44100];
    *SUPPORTED
        .iter()
        .min_by_key(|&&r| r.abs_diff(rate))
        .unwrap_or(&44100)
}

fn mp3_sample_rate(rate: u32) -> u32 {
    const SUPPORTED: &[u32] = &[32000, 44100];
    *SUPPORTED
        .iter()
        .min_by_key(|&&r| r.abs_diff(rate))
        .unwrap_or(&44100)
}

#[cfg(test)]
mod tests {
    use std::str::FromStr;

    use super::*;
    use crate::tts::{TtsBackend, TtsOptions, TtsRequest};

    #[test]
    fn output_format_maps_pcm_s16le_to_pcm() {
        let request = TtsRequest {
            text: "hello".to_string(),
            options: TtsOptions {
                response_format: Some("pcm_s16le".to_string()),
                ..Default::default()
            },
            ..Default::default()
        };
        let (format, sample_rate) = output_format_for_request(44100, &request);
        assert_eq!(format, "pcm");
        assert_eq!(sample_rate, 44100);
    }

    #[test]
    fn output_format_defaults_to_mp3() {
        let request = TtsRequest {
            text: "hello".to_string(),
            ..Default::default()
        };
        let (format, sample_rate) = output_format_for_request(44100, &request);
        assert_eq!(format, "mp3");
        assert_eq!(sample_rate, 44100);
    }

    #[test]
    fn output_format_clamps_pcm_sample_rate() {
        let request = TtsRequest {
            text: "hello".to_string(),
            options: TtsOptions {
                response_format: Some("pcm".to_string()),
                ..Default::default()
            },
            ..Default::default()
        };
        let (format, sample_rate) = output_format_for_request(48000, &request);
        assert_eq!(format, "pcm");
        assert_eq!(sample_rate, 44100);
    }

    #[test]
    fn tts_backend_parses_fish() {
        assert_eq!(TtsBackend::from_str("fish").unwrap(), TtsBackend::Fish);
        assert_eq!(TtsBackend::from_str("FISH").unwrap(), TtsBackend::Fish);
    }
}
