//! Streaming ASR gate — feed normalized audio chunks to a streaming recognizer
//! and produce a final transcript with the raw captured samples.
//!
//! This is the post-wake stage of the ambient pipeline. It does no recording
//! on its own; callers provide chunks (usually from a `StreamingRecorder`).

use std::sync::Arc;

use thiserror::Error;

use crate::stt::{AsrStream, StreamingSpeechToText, SttError};
use crate::wav::normalize;

/// Target RMS for samples fed to the ASR backend.
const ASR_TARGET_RMS: f32 = 0.1;

/// Errors from a streaming ASR session.
#[derive(Debug, Error)]
pub enum StreamAsrError {
    #[error("failed to start stream: {0}")]
    Start(#[from] SttError),
    #[error("ASR session already finished")]
    AlreadyFinished,
}

/// A single streaming ASR session.
///
/// Keeps the raw captured samples for downstream use (e.g. speaker
/// verification) while feeding normalized audio to the ASR backend.
pub struct StreamingAsrSession {
    stream: Box<dyn AsrStream>,
    /// Raw (un-normalized) samples accumulated since session start.
    raw: Vec<f32>,
    finished: bool,
}

impl StreamingAsrSession {
    /// Start a new streaming ASR session for `language`.
    pub async fn new(
        stt: Arc<dyn StreamingSpeechToText>,
        language: &str,
    ) -> Result<Self, StreamAsrError> {
        let stream = stt
            .start_stream(language)
            .await
            .map_err(StreamAsrError::Start)?;
        Ok(Self {
            stream,
            raw: Vec::new(),
            finished: false,
        })
    }

    /// Feed a chunk of f32 PCM samples (typically 16 kHz mono).
    ///
    /// Keeps the raw copy and feeds a normalized copy to the ASR stream.
    pub fn feed(&mut self, samples: &[f32]) {
        if self.finished {
            return;
        }
        self.raw.extend_from_slice(samples);
        self.stream
            .accept_waveform(&normalize(samples, ASR_TARGET_RMS));
    }

    /// Current partial transcript, if any.
    pub fn text(&mut self) -> String {
        self.stream.text()
    }

    /// Finish the stream and return the final transcript and raw samples.
    pub async fn finish(mut self) -> Result<(String, Vec<f32>), StreamAsrError> {
        if self.finished {
            return Err(StreamAsrError::AlreadyFinished);
        }
        self.finished = true;
        let text = self.stream.finish().await.map_err(StreamAsrError::Start)?;
        Ok((text, self.raw))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::SpeechToText;
    use async_trait::async_trait;

    struct MockAsrStream {
        buffers: Vec<Vec<f32>>,
        final_text: String,
    }

    #[async_trait]
    impl AsrStream for MockAsrStream {
        fn accept_waveform(&mut self, samples: &[f32]) {
            self.buffers.push(samples.to_vec());
        }

        fn text(&mut self) -> String {
            if self.buffers.is_empty() {
                String::new()
            } else {
                "partial".into()
            }
        }

        async fn finish(&mut self) -> Result<String, SttError> {
            Ok(self.final_text.clone())
        }
    }

    struct MockStt {
        final_text: String,
    }

    #[async_trait]
    impl StreamingSpeechToText for MockStt {
        async fn start_stream(&self, _language: &str) -> Result<Box<dyn AsrStream>, SttError> {
            Ok(Box::new(MockAsrStream {
                buffers: Vec::new(),
                final_text: self.final_text.clone(),
            }))
        }
    }

    #[async_trait]
    impl SpeechToText for MockStt {
        async fn transcribe(&self, _audio: &[f32]) -> Result<String, SttError> {
            Ok(self.final_text.clone())
        }
    }

    #[tokio::test]
    async fn session_feeds_and_finishes() {
        let stt: Arc<dyn StreamingSpeechToText> = Arc::new(MockStt {
            final_text: "hello world".into(),
        });
        let mut session = StreamingAsrSession::new(stt, "ja").await.unwrap();
        session.feed(&[0.1, 0.2, 0.3]);
        session.feed(&[0.4, 0.5]);
        let (text, raw) = session.finish().await.unwrap();
        assert_eq!(text, "hello world");
        assert_eq!(raw, vec![0.1, 0.2, 0.3, 0.4, 0.5]);
    }
}
