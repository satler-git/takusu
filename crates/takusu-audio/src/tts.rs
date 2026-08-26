//! Text-to-speech provider trait and shared request/response types.
//!
//! `TextToSpeech` returns a chunked byte stream so callers can play audio
//! incrementally. The default `synthesize` method collects that stream into
//! a single `Vec<u8>` for callers that do not need streaming.

use std::path::PathBuf;
use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use futures_util::{Stream, StreamExt};
use serde::{Deserialize, Serialize};
use thiserror::Error;
use tokio::sync::{OwnedSemaphorePermit, Semaphore};

use crate::secrets::{ApiKey, EndpointUrl};

/// Maximum number of synthesis requests we issue per retry attempt in the
/// backends below.
pub(crate) const MAX_TTS_ATTEMPTS: u32 = 3;

/// TTS backend identifier.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum TtsBackend {
    Cartesia,
    Android,
    Fish,
}

impl std::fmt::Display for TtsBackend {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            TtsBackend::Cartesia => write!(f, "cartesia"),
            TtsBackend::Android => write!(f, "android"),
            TtsBackend::Fish => write!(f, "fish"),
        }
    }
}

impl std::str::FromStr for TtsBackend {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "cartesia" => Ok(TtsBackend::Cartesia),
            "android" => Ok(TtsBackend::Android),
            "fish" => Ok(TtsBackend::Fish),
            _ => Err(format!("unsupported TTS backend: {s}")),
        }
    }
}

/// Persistable provider-neutral settings used by Mobile and future backends.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TtsProviderConfig {
    pub id: String,
    pub name: String,
    pub provider: TtsBackend,
    pub voice_id: String,
    pub model: Option<String>,
    pub language: String,
    pub sample_rate: u32,
    pub speed: Option<f32>,
}

#[derive(Debug, Clone)]
pub struct TtsConfig {
    pub backend: TtsBackend,
    pub url: EndpointUrl,
    pub api_key: Option<ApiKey>,
}

#[derive(Debug, Clone, Default)]
pub struct TtsOptions {
    pub response_format: Option<String>,
    pub speed: Option<f32>,
}

#[derive(Debug, Clone, Default)]
pub struct TtsRequest {
    pub text: String,
    pub voice: Option<String>,
    pub reference_audio_path: Option<PathBuf>,
    pub options: TtsOptions,
}

#[derive(Debug, Error)]
pub enum TtsError {
    #[error("http error: {0}")]
    Http(#[from] reqwest::Error),
    #[error("api error {status}: {message}")]
    Api { status: u16, message: String },
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("json error: {0}")]
    Json(#[from] serde_json::Error),
}

/// A stream of audio chunks produced by a TTS backend.
pub type TtsStream = Pin<Box<dyn Stream<Item = Result<Bytes, TtsError>> + Send + 'static>>;

/// Wrap a backend response stream so the concurrency-limit permit stays held
/// until the stream is exhausted or dropped.
///
/// TTS APIs (Fish, Cartesia) count an HTTP request as "concurrent" for as long
/// as the audio connection stays open, not just until headers arrive. Keeping
/// the permit inside the stream closure is what actually bounds the number of
/// in-flight synthesis jobs to `max_concurrent`.
pub(crate) fn stream_with_permit<S>(
    inner: S,
    permit: OwnedSemaphorePermit,
) -> TtsStream
where
    S: Stream<Item = Result<Bytes, reqwest::Error>> + Send + 'static,
{
    Box::pin(inner.map(move |item| {
        // Referencing the permit forces the `move` closure to capture it, so it
        // is released only when this stream wrapper is dropped.
        let _permit = &permit;
        item.map_err(TtsError::Http)
    }))
}

/// Whether an API status code is worth retrying. Rate limiting (429) and
/// server-side 5xx errors are transient; 4xx client errors are not.
pub(crate) fn is_transient_tts_status(status: u16) -> bool {
    matches!(status, 429 | 500..=599)
}

/// Delay before the next TTS retry. Honors the `Retry-After` header when the
/// API provides it; otherwise backs off exponentially from a small base.
pub(crate) fn tts_retry_delay(retry_after: Option<u64>, attempt: u32) -> Duration {
    if let Some(secs) = retry_after {
        return Duration::from_secs(secs);
    }
    let exp = attempt.saturating_sub(1).min(8);
    Duration::from_millis(200 * u64::from(1u32 << exp))
}

/// Read the `Retry-After` header as a whole number of seconds, if present.
pub(crate) fn parse_retry_after(headers: &reqwest::header::HeaderMap) -> Option<u64> {
    headers
        .get(reqwest::header::RETRY_AFTER)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.parse().ok())
}

/// Acquire a permit for a synthesis request, capped at `max_concurrent`
/// in-flight jobs. The permit is returned so the caller can attach it to the
/// response stream via [`stream_with_permit`].
pub(crate) async fn acquire_permit(
    semaphore: &Arc<Semaphore>,
) -> Result<OwnedSemaphorePermit, TtsError> {
    semaphore
        .clone()
        .acquire_owned()
        .await
        .map_err(|_| TtsError::Api {
            status: 503,
            message: "TTS concurrency limiter closed".to_string(),
        })
}

#[async_trait::async_trait]
pub trait TextToSpeech: Send + Sync {
    /// Synthesize the request into a chunked audio stream.
    async fn synthesize_stream(&self, request: &TtsRequest) -> Result<TtsStream, TtsError>;

    /// Synthesize the request into a single audio buffer.
    ///
    /// The default implementation collects `synthesize_stream` into a `Vec<u8>`.
    async fn synthesize(&self, request: &TtsRequest) -> Result<Vec<u8>, TtsError> {
        use futures_util::TryStreamExt;

        let stream = self.synthesize_stream(request).await?;
        let chunks: Vec<Bytes> = stream.try_collect().await?;
        let mut audio = Vec::with_capacity(chunks.iter().map(|c| c.len()).sum());
        for chunk in chunks {
            audio.extend_from_slice(&chunk);
        }
        Ok(audio)
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::time::Duration;

    use super::*;
    use tokio::sync::Semaphore;

    #[test]
    fn retry_delay_classifies_transient_codes() {
        assert!(is_transient_tts_status(429));
        assert!(is_transient_tts_status(500));
        assert!(is_transient_tts_status(503));
        assert!(!is_transient_tts_status(400));
        assert!(!is_transient_tts_status(401));
        assert!(!is_transient_tts_status(404));
    }

    #[test]
    fn retry_delay_honors_retry_after() {
        assert_eq!(tts_retry_delay(Some(2), 1), Duration::from_secs(2));
        assert_eq!(tts_retry_delay(Some(0), 3), Duration::from_secs(0));
    }

    #[test]
    fn retry_delay_backs_off_exponentially() {
        assert_eq!(tts_retry_delay(None, 1), Duration::from_millis(200));
        assert_eq!(tts_retry_delay(None, 2), Duration::from_millis(400));
        assert_eq!(tts_retry_delay(None, 3), Duration::from_millis(800));
    }

    #[test]
    fn parse_retry_after_reads_seconds() {
        let mut headers = reqwest::header::HeaderMap::new();
        assert_eq!(parse_retry_after(&headers), None);
        headers.insert(reqwest::header::RETRY_AFTER, "5".parse().unwrap());
        assert_eq!(parse_retry_after(&headers), Some(5));
        headers.insert(reqwest::header::RETRY_AFTER, "abc".parse().unwrap());
        assert_eq!(parse_retry_after(&headers), None);
    }

    #[tokio::test]
    async fn permit_is_released_after_stream_collected() {
        let semaphore = Arc::new(Semaphore::new(1));
        {
            let permit = acquire_permit(&semaphore).await.unwrap();
            let inner = futures_util::stream::iter(vec![Ok::<_, reqwest::Error>(Bytes::new())]);
            let mut stream = stream_with_permit(inner, permit);
            // While the stream lives, the single permit is taken.
            assert_eq!(semaphore.available_permits(), 0);
            while stream.next().await.is_some() {}
        }
        // Dropping the stream wrapper releases the permit.
        assert_eq!(semaphore.available_permits(), 1);
    }
}
