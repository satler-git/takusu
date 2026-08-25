//! Android bridge over the shared Silero endpoint detector (WI-12).
//!
//! Kotlin `AudioRecorder` owns the microphone; it feeds 16 kHz i16 PCM here in
//! chunks of ~160 ms and asks [`VAD::feed`] whether the current utterance has
//! ended. The decision uses the same sherpa Silero endpoint detector as the
//! desktop session, so both platforms stop ~0.5 s after speech ends.

use std::path::Path;
use std::sync::Mutex;

use takusu_audio::{Endpoint, SileroEndpoint, VadEvent};

use crate::TakusuError;

struct VadInner {
    endpoint: SileroEndpoint,
    stop_requested: bool,
}

/// Reusable utterance endpointing bridge for the Android recorder.
#[derive(uniffi::Object)]
pub struct AndroidVad {
    inner: Mutex<VadInner>,
}

impl std::fmt::Debug for AndroidVad {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AndroidVad").finish()
    }
}

#[uniffi::export]
impl AndroidVad {
    /// Create the bridge from a Silero VAD model file
    /// (`silero_vad.onnx`, as returned by `downloadModel("silero-vad", ...)`).
    #[uniffi::constructor]
    pub fn new(model_path: String) -> Result<Self, TakusuError> {
        let endpoint =
            SileroEndpoint::new(Path::new(&model_path), 16_000, 0.5, 60.0).map_err(|error| {
                TakusuError::Model {
                    detail: error.to_string(),
                }
            })?;
        Ok(Self {
            inner: Mutex::new(VadInner {
                endpoint,
                stop_requested: false,
            }),
        })
    }

    /// Feed 16 kHz mono i16 PCM samples. Returns `true` once the current
    /// utterance's trailing silence has elapsed (recording should stop).
    pub fn feed(&self, samples: Vec<i16>) -> bool {
        let mut inner = self.inner.lock().unwrap_or_else(|error| error.into_inner());
        let f32s: Vec<f32> = samples
            .iter()
            .map(|&s| {
                if s < 0 {
                    s as f32 / 32768.0
                } else if s > 0 {
                    s as f32 / 32767.0
                } else {
                    0.0
                }
            })
            .collect();
        if matches!(inner.endpoint.push(&f32s), Some(VadEvent::SpeechEnd)) {
            inner.stop_requested = true;
        }
        inner.stop_requested
    }

    /// Whether an utterance endpoint has been reached since the last reset.
    pub fn should_stop(&self) -> bool {
        self.inner
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .stop_requested
    }

    /// Prepare for a fresh utterance without rebuilding the model.
    pub fn reset(&self) {
        let mut inner = self.inner.lock().unwrap_or_else(|error| error.into_inner());
        inner.endpoint.reset();
        inner.stop_requested = false;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn error_maps_to_takusu_error_on_bad_model() {
        let err = AndroidVad::new("/nonexistent/silero_vad.onnx".into()).unwrap_err();
        assert!(matches!(err, TakusuError::Model { .. }));
    }
}
