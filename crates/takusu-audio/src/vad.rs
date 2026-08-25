//! Voice activity detection and utterance endpointing.
//!
//! [`VadEndpoint`] is a backend-agnostic endpoint state machine: it consumes a
//! chronological sequence of voiced/voiceless decisions and emits
//! `SpeechStart` / `SpeechEnd` transitions once speech has sustained past a
//! minimum duration and then stayed silent past a configurable tail. It is
//! deliberately independent of how the voiced decision is produced so the same
//! endpointing logic is shared by the energy-based fallback and the sherpa
//! Silero backend.
//!
//! Two [`VoiceActivity`] backends are provided:
//! - [`EnergyVad`]: a tiny RMS threshold decision, no model, always available.
//! - [`SileroVad`]: sherpa-onnx Silero behind the `sherpa` feature.

#[cfg(feature = "sherpa")]
use std::path::Path;
use std::time::Duration;

use crate::wav::SHERPA_SAMPLE_RATE;

/// A decision source that classifies a chunk of 16 kHz mono f32 samples as
/// voiced or voiceless.
pub trait VoiceActivity {
    /// Return `true` if the chunk is currently considered voiced.
    fn voiced(&mut self, samples: &[f32]) -> bool;
}

/// One transition produced by an [`Endpoint`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VadEvent {
    /// Speech began after the start debounce.
    SpeechStart,
    /// Speech ended after the configured silence tail.
    SpeechEnd,
}

/// Backend-agnostic utterance endpointing. Hidden behind this trait so both the
/// energy fallback and the sherpa Silero detector present the same `push` /
/// `has_speech` interface to the recording loop.
pub trait Endpoint: Send {
    /// Feed one chunk of 16 kHz mono f32 samples and return any transition it
    /// produced.
    fn push(&mut self, samples: &[f32]) -> Option<VadEvent>;
    /// Whether an utterance is currently open.
    fn has_speech(&self) -> bool;
    /// Reset the detector to prepare for a fresh utterance.
    fn reset(&mut self);
}

impl<E: Endpoint + ?Sized> Endpoint for Box<E> {
    fn push(&mut self, samples: &[f32]) -> Option<VadEvent> {
        (**self).push(samples)
    }
    fn has_speech(&self) -> bool {
        (**self).has_speech()
    }
    fn reset(&mut self) {
        (**self).reset()
    }
}

impl<S: VoiceActivity + Send> Endpoint for VadEndpoint<S> {
    fn push(&mut self, samples: &[f32]) -> Option<VadEvent> {
        VadEndpoint::push(self, samples)
    }
    fn has_speech(&self) -> bool {
        VadEndpoint::has_speech(self)
    }
    fn reset(&mut self) {
        VadEndpoint::reset(self);
    }
}

/// Default RMS energy threshold for the fallback [`EnergyVad`].
///
/// 0.02 is a safer starting point than the previous 0.01 for raw microphone
/// levels: it is high enough to ignore typical quiet-room ambient noise while
/// still catching normal spoken input. Tune it through `VadEndpointConfig` or
/// `AudioConfig` for quiet mics or noisy environments.
pub const DEFAULT_ENERGY_THRESHOLD: f32 = 0.02;

/// Configuration for [`VadEndpoint`].
#[derive(Debug, Clone)]
pub struct VadEndpointConfig {
    /// Continuous voiced time required before a `SpeechStart` fires.
    pub min_speech: Duration,
    /// Silence tail that must elapse after the last voiced frame before a
    /// hard `SpeechEnd` fires.
    pub max_silence: Duration,
    /// Absolute cap on a single utterance; a `SpeechEnd` is forced after this
    /// much active time regardless of whether speech is still present.
    pub max_speech: Duration,
    /// RMS energy threshold used by the `EnergyVad` fallback. A frame is
    /// considered voiced when its RMS is at or above this value in the `[-1, 1]`
    /// sample range. Tunable so the same endpointing logic works on quiet and
    /// noisy microphones.
    pub energy_threshold: f32,
}

impl Default for VadEndpointConfig {
    fn default() -> Self {
        Self {
            min_speech: Duration::from_millis(120),
            max_silence: Duration::from_millis(500),
            max_speech: Duration::from_secs(60),
            energy_threshold: DEFAULT_ENERGY_THRESHOLD,
        }
    }
}

/// Endpointing state machine over a [`VoiceActivity`] decision source.
///
/// Call [`push`](Self::push) with each chunk as it arrives. When a
/// `SpeechEnd` is seen the caller stops recording and hands the accumulated
/// audio to the ASR backend. [`has_speech`](Self::has_speech) is `true`
/// between `SpeechStart` and `SpeechEnd`.
pub struct VadEndpoint<S> {
    vad: S,
    sample_rate: u32,
    config: VadEndpointConfig,
    active: bool,
    /// Consecutive voiced samples (in samples) since the start debounce began.
    voiced_started: u64,
    /// Consecutive voiceless samples while active.
    voiceless_active: u64,
    /// Total voiced samples while active (used for the absolute cap).
    active_samples: u64,
}

impl<S: VoiceActivity> VadEndpoint<S> {
    /// Create an endpoint state machine over the given decision source.
    pub fn new(vad: S, sample_rate: u32, config: VadEndpointConfig) -> Self {
        Self {
            vad,
            sample_rate,
            config,
            active: false,
            voiced_started: 0,
            voiceless_active: 0,
            active_samples: 0,
        }
    }

    /// Feed one chunk of 16 kHz mono f32 samples and return any transition
    /// produced by it.
    pub fn push(&mut self, samples: &[f32]) -> Option<VadEvent> {
        let voiced = self.vad.voiced(samples);
        let n = samples.len() as u64;

        if self.active {
            if voiced {
                self.voiceless_active = 0;
                self.active_samples += n;
            } else {
                self.voiceless_active += n;
            }
            // Absolute cap: force an end regardless of current speech.
            let spoken_ms = self.active_samples * 1000 / self.sample_rate as u64;
            if spoken_ms >= self.config.max_speech.as_millis() as u64 {
                return self.end();
            }
            let silence_ms = self.voiceless_active * 1000 / self.sample_rate as u64;
            if silence_ms >= self.config.max_silence.as_millis() as u64 {
                return self.end();
            }
            None
        } else if voiced {
            self.voiced_started += n;
            let spoken_ms = self.voiced_started * 1000 / self.sample_rate as u64;
            if spoken_ms >= self.config.min_speech.as_millis() as u64 {
                self.active = true;
                self.active_samples = self.voiced_started;
                return Some(VadEvent::SpeechStart);
            }
            None
        } else {
            // A voiceless gap while not yet active discards the running start
            // debounce: speech requires sustained voice.
            self.voiced_started = 0;
            None
        }
    }

    fn end(&mut self) -> Option<VadEvent> {
        self.active = false;
        self.voiced_started = 0;
        self.voiceless_active = 0;
        self.active_samples = 0;
        Some(VadEvent::SpeechEnd)
    }

    /// Whether an utterance is currently open (between start and end).
    pub fn has_speech(&self) -> bool {
        self.active
    }

    /// Reset the endpoint state, discarding the open utterance if any.
    pub fn reset(&mut self) {
        self.active = false;
        self.voiced_started = 0;
        self.voiceless_active = 0;
        self.active_samples = 0;
    }
}

/// RMS energy threshold voice activity detector. No model, cheap, and safe as
/// a default before a proper Silero model is available on the device.
pub struct EnergyVad {
    threshold: f32,
}

impl EnergyVad {
    /// Create a detector with an absolute RMS threshold in the `[-1, 1]`
    /// sample range.
    pub fn new(threshold: f32) -> Self {
        Self { threshold }
    }
}

impl Default for EnergyVad {
    fn default() -> Self {
        // The default is now configurable through `VadEndpointConfig` (and
        // `AudioConfig` in the agent). The constant here is the fallback when
        // `EnergyVad` is constructed directly.
        Self::new(DEFAULT_ENERGY_THRESHOLD)
    }
}

impl VoiceActivity for EnergyVad {
    fn voiced(&mut self, samples: &[f32]) -> bool {
        if samples.is_empty() {
            return false;
        }
        let sum = samples.iter().map(|s| s * s).sum::<f32>();
        let rms = (sum / samples.len() as f32).sqrt();
        rms >= self.threshold
    }
}

/// sherpa-onnx Silero utterance endpointing (feature-gated on `sherpa`).
///
/// Unlike the energy fallback, `SileroEndpoint` uses the detector's own
/// segment-completion signal: a speech segment (including its configured
/// trailing `min_silence_duration`) is queued only once the tail of an
/// utterance has elapsed, so the recording loop can stop right then without a
/// fragile level threshold. Set `min_silence_duration` to the desired
/// post-speech tail (e.g. 0.5 s).
#[cfg(feature = "sherpa")]
pub struct SileroEndpoint {
    detector: sherpa_onnx::VoiceActivityDetector,
    speech_open: bool,
}

#[cfg(feature = "sherpa")]
impl SileroEndpoint {
    /// Create the detector from an ONNX model file. The model must be the
    /// sherpa-onnx Silero VAD export (`silero_vad.onnx`).
    pub fn new(
        model: &Path,
        sample_rate: i32,
        min_silence: f32,
        max_speech: f32,
    ) -> Result<Self, crate::stt::SttError> {
        let config = sherpa_onnx::VadModelConfig {
            silero_vad: sherpa_onnx::SileroVadModelConfig {
                model: Some(model.to_string_lossy().into_owned()),
                threshold: 0.5,
                min_silence_duration: min_silence,
                min_speech_duration: 0.25,
                window_size: 512,
                max_speech_duration: max_speech,
            },
            ten_vad: Default::default(),
            sample_rate,
            num_threads: 2,
            provider: None,
            debug: false,
        };
        // The internal sample buffer must be at least `max_speech_duration`
        // so long utterances are not truncated (WI-12).
        let buffer_size_in_seconds = config.silero_vad.max_speech_duration;
        let detector = sherpa_onnx::VoiceActivityDetector::create(&config, buffer_size_in_seconds)
            .ok_or_else(|| crate::stt::SttError::Other("failed to create Silero VAD".into()))?;
        Ok(Self {
            detector,
            speech_open: false,
        })
    }

    /// Reset the detector so a fresh utterance can be segmented without
    /// rebuilding the model.
    pub fn reset(&mut self) {
        self.detector.reset();
        self.speech_open = false;
    }
}

#[cfg(feature = "sherpa")]
impl Endpoint for SileroEndpoint {
    fn push(&mut self, samples: &[f32]) -> Option<VadEvent> {
        self.detector.accept_waveform(samples);
        let in_speech = self.detector.detected();
        if in_speech && !self.speech_open {
            self.speech_open = true;
            return Some(VadEvent::SpeechStart);
        }
        // A completed segment (speech plus its trailing silence) is queued once
        // an utterance has fully ended.
        if !self.detector.is_empty() {
            self.detector.pop();
            self.speech_open = false;
            return Some(VadEvent::SpeechEnd);
        }
        None
    }

    fn has_speech(&self) -> bool {
        self.speech_open || !self.detector.is_empty() || self.detector.detected()
    }

    fn reset(&mut self) {
        self.detector.reset();
        self.speech_open = false;
    }
}

/// Build an utterance endpoint using the sherpa Silero model from the default
/// model cache when available, else the energy fallback.
#[cfg(feature = "sherpa")]
pub fn silero_endpoint_from_cache(config: &VadEndpointConfig) -> Option<SileroEndpoint> {
    use crate::ModelCache;
    let cache = ModelCache::default_dir().ok()?;
    silero_endpoint_from_cache_with_dir(config, cache.cache_dir())
}

/// Build an utterance endpoint using the sherpa Silero model from an explicit
/// cache directory.
#[cfg(feature = "sherpa")]
pub fn silero_endpoint_from_cache_with_dir(
    config: &VadEndpointConfig,
    cache_dir: &Path,
) -> Option<SileroEndpoint> {
    use crate::ModelCache;
    let cache = ModelCache::new(cache_dir);
    let model = match cache.ensure_silero_vad() {
        Ok(model) => model,
        Err(error) => {
            eprintln!("Silero VAD model unavailable ({error}); using energy fallback");
            return None;
        }
    };
    match SileroEndpoint::new(
        &model,
        SHERPA_SAMPLE_RATE as i32,
        config.max_silence.as_secs_f32(),
        config.max_speech.as_secs_f32(),
    ) {
        Ok(endpoint) => {
            eprintln!("using Silero VAD endpointing");
            Some(endpoint)
        }
        Err(error) => {
            eprintln!("failed to load Silero VAD ({error}); using energy fallback");
            None
        }
    }
}

/// Build the default endpointing backend with a concrete `VadEndpointConfig`.
///
/// Silero is used when its model is present and loadable; otherwise the
/// raw-energy `VadEndpoint<EnergyVad>` fallback is created with the configured
/// `energy_threshold`.
pub fn default_endpoint_with_config(config: VadEndpointConfig) -> Box<dyn Endpoint> {
    #[cfg(feature = "sherpa")]
    if let Some(silero) = silero_endpoint_from_cache(&config) {
        return Box::new(silero);
    }
    default_energy_endpoint(config)
}

fn default_energy_endpoint(config: VadEndpointConfig) -> Box<dyn Endpoint> {
    eprintln!("using energy VAD endpointing");
    Box::new(VadEndpoint::new(
        EnergyVad::new(config.energy_threshold),
        SHERPA_SAMPLE_RATE,
        config,
    ))
}

/// Build the default endpointing backend with an explicit model cache
/// directory. Used on Android where the model cache lives in the app's
/// `noBackupFilesDir` rather than the desktop default.
#[cfg(feature = "sherpa")]
pub fn default_endpoint_with_config_and_cache_dir(
    config: VadEndpointConfig,
    cache_dir: &Path,
) -> Box<dyn Endpoint> {
    if let Some(silero) = silero_endpoint_from_cache_with_dir(&config, cache_dir) {
        return Box::new(silero);
    }
    default_energy_endpoint(config)
}

/// The default endpointing backend: Silero when its model is present and
/// loadable, otherwise the raw-energy `VadEndpoint<EnergyVad>` fallback.
///
/// This is the single construction point used by the recording loops so both
/// the agent and the CLI agree on which detector is active.
pub fn default_endpoint() -> Box<dyn Endpoint> {
    default_endpoint_with_config(VadEndpointConfig::default())
}

/// Async wrapper over [`default_endpoint_with_config`] that performs the
/// (blocking) model download / load on a blocking thread so it can be awaited
/// from an async context without violating tokio's blocking rules.
#[cfg(any(feature = "record", feature = "sherpa"))]
pub async fn default_endpoint_async_with_config(config: VadEndpointConfig) -> Box<dyn Endpoint> {
    tokio::task::spawn_blocking(move || default_endpoint_with_config(config))
        .await
        .unwrap_or_else(|_| {
            Box::new(VadEndpoint::new(
                EnergyVad::new(DEFAULT_ENERGY_THRESHOLD),
                SHERPA_SAMPLE_RATE,
                VadEndpointConfig::default(),
            ))
        })
}

/// Async wrapper over [`default_endpoint`] that performs the (blocking) model
/// download / load on a blocking thread so it can be awaited from an async
/// context without violating tokio's blocking rules.
#[cfg(any(feature = "record", feature = "sherpa"))]
pub async fn default_endpoint_async() -> Box<dyn Endpoint> {
    default_endpoint_async_with_config(VadEndpointConfig::default()).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::wav::SHERPA_SAMPLE_RATE;

    /// A scripted decision source used to validate the endpoint state machine
    /// without any model.
    struct Scripted {
        frames: Vec<bool>,
        index: usize,
    }

    impl Scripted {
        fn new(frames: Vec<bool>) -> Self {
            Self { frames, index: 0 }
        }
    }

    impl VoiceActivity for Scripted {
        fn voiced(&mut self, _samples: &[f32]) -> bool {
            if self.index >= self.frames.len() {
                return false;
            }
            let value = self.frames[self.index];
            self.index += 1;
            value
        }
    }

    /// Frame duration in milliseconds for a given number of samples.
    fn frame_ms(n: u32) -> Duration {
        Duration::from_millis((n * 1000 / SHERPA_SAMPLE_RATE) as u64)
    }

    #[test]
    fn silence_never_starts_speech() {
        let mut endpoint = VadEndpoint::new(
            Scripted::new(vec![false; 10]),
            SHERPA_SAMPLE_RATE,
            VadEndpointConfig::default(),
        );
        // 1600 samples = 100 ms of silence, several frames.
        for _ in 0..8 {
            assert_eq!(endpoint.push(&[0.0; 160]), None);
        }
        assert!(!endpoint.has_speech());
    }

    #[test]
    fn sustained_voice_emits_speech_start_then_end() {
        // min_speech = 120 ms = 1920 samples; three 800-sample frames.
        let config = VadEndpointConfig {
            min_speech: Duration::from_millis(120),
            max_silence: frame_ms(800), // one voiceless frame ends speech
            ..Default::default()
        };
        let mut endpoint =
            VadEndpoint::new(Scripted::new(vec![true; 3]), SHERPA_SAMPLE_RATE, config);
        assert_eq!(endpoint.push(&[0.5; 800]), None);
        assert_eq!(endpoint.push(&[0.5; 800]), None);
        assert_eq!(endpoint.push(&[0.5; 800]), Some(VadEvent::SpeechStart));
        assert!(endpoint.has_speech());
        assert_eq!(endpoint.push(&[0.0; 800]), Some(VadEvent::SpeechEnd));
        assert!(!endpoint.has_speech());
    }

    #[test]
    fn short_burst_does_not_start_speech() {
        // A 60 ms burst under the 120 ms min_speech never starts.
        let config = VadEndpointConfig {
            min_speech: Duration::from_millis(120),
            ..Default::default()
        };
        let mut endpoint =
            VadEndpoint::new(Scripted::new(vec![true; 1]), SHERPA_SAMPLE_RATE, config);
        assert_eq!(endpoint.push(&[0.5; 960]), None);
        assert!(!endpoint.has_speech());
    }

    #[test]
    fn silence_gap_within_utterance_does_not_split() {
        // A gap shorter than max_silence keeps the utterance open.
        let config = VadEndpointConfig {
            min_speech: Duration::from_millis(120),
            max_silence: frame_ms(1600),
            ..Default::default()
        };
        let mut endpoint = VadEndpoint::new(
            Scripted::new(vec![true, true, true, false, true, false, false]),
            SHERPA_SAMPLE_RATE,
            config,
        );
        // Three voiced frames open the utterance.
        assert_eq!(endpoint.push(&[0.5; 800]), None);
        assert_eq!(endpoint.push(&[0.5; 800]), None);
        assert_eq!(endpoint.push(&[0.5; 800]), Some(VadEvent::SpeechStart));
        // 800-sample gap (50 ms) < 100-ms max_silence: still open.
        assert_eq!(endpoint.push(&[0.0; 800]), None);
        assert!(endpoint.has_speech());
        // Next voiced frame clears the gap.
        assert_eq!(endpoint.push(&[0.5; 800]), None);
        assert!(endpoint.has_speech());
        // Two voiceless frames accumulate 100 ms of silence and end speech.
        assert_eq!(endpoint.push(&[0.0; 800]), None);
        assert_eq!(endpoint.push(&[0.0; 800]), Some(VadEvent::SpeechEnd));
    }

    #[test]
    fn max_speech_caps_an_open_utterance() {
        let config = VadEndpointConfig {
            min_speech: Duration::from_millis(120),
            max_silence: Duration::from_secs(100),
            max_speech: Duration::from_millis(200),
            energy_threshold: DEFAULT_ENERGY_THRESHOLD,
        };
        let mut endpoint =
            VadEndpoint::new(Scripted::new(vec![true; 10]), SHERPA_SAMPLE_RATE, config);
        // 3 x 800 samples = 2400 = 150 ms >= 120 ms min_speech -> start.
        assert_eq!(endpoint.push(&[0.5; 800]), None);
        assert_eq!(endpoint.push(&[0.5; 800]), None);
        assert_eq!(endpoint.push(&[0.5; 800]), Some(VadEvent::SpeechStart));
        // 4 x 800 = 3200 = 200 ms >= 200 ms max_speech while still voiced.
        assert_eq!(endpoint.push(&[0.5; 800]), Some(VadEvent::SpeechEnd));
    }

    #[test]
    fn energy_vad_classifies_silence_and_speech() {
        let mut vad = EnergyVad::default();
        assert!(!vad.voiced(&[0.0; 160]));
        assert!(vad.voiced(&[0.5; 160]));
        assert!(vad.voiced(&[-0.5; 160]));
        let mut quiet = EnergyVad::new(0.5);
        assert!(!quiet.voiced(&[0.1; 160]));
    }

    /// Feeds a synthetic tone-burst-then-silence signal through a real
    /// [`VadEndpoint`] over [`EnergyVad`] and asserts the utterance is
    /// segmented at the expected points. This exercises the actual audio
    /// path without requiring a microphone, so it runs in CI.
    #[test]
    fn synthetic_tone_burst_is_segmented_into_one_utterance() {
        const FRAME: usize = 160; // 10 ms at 16 kHz
        let config = VadEndpointConfig {
            min_speech: Duration::from_millis(120),
            max_silence: Duration::from_millis(300),
            ..Default::default()
        };
        let mut endpoint = VadEndpoint::new(EnergyVad::default(), SHERPA_SAMPLE_RATE, config);

        // 40 frames of near-silence (no start).
        for _ in 0..40 {
            assert_eq!(endpoint.push(&[0.0; FRAME]), None);
        }

        // 24 frames (240 ms) of a 440 Hz tone at amplitude 0.5 → SpeechStart
        // once the 120 ms min_speech debounce elapses.
        let mut started = false;
        let mut last = None;
        for i in 0..24 {
            let start = i * FRAME;
            let signal: Vec<f32> = (start..start + FRAME)
                .map(|n| {
                    let t = n as f32 / SHERPA_SAMPLE_RATE as f32;
                    0.5 * (2.0 * std::f32::consts::PI * 440.0 * t).sin()
                })
                .collect();
            let event = endpoint.push(&signal);
            if event == Some(VadEvent::SpeechStart) {
                started = true;
            }
            last = event;
        }
        assert!(started, "sustained tone must start the utterance");
        assert_eq!(last, None, "utterance must stay open during tone");

        // 40 frames of silence (400 ms) → SpeechEnd after the 300 ms tail.
        let mut seen_end = false;
        for _ in 0..40 {
            if endpoint.push(&[0.0; FRAME]) == Some(VadEvent::SpeechEnd) {
                seen_end = true;
                break;
            }
        }
        assert!(seen_end, "silence tail must end the utterance");
        assert!(!endpoint.has_speech());
    }

    #[test]
    fn energy_vad_default_threshold_is_safer() {
        let config = VadEndpointConfig::default();
        assert_eq!(config.energy_threshold, DEFAULT_ENERGY_THRESHOLD);

        let mut vad = EnergyVad::default();
        // Just above the default threshold should be voiced.
        let above = config.energy_threshold * 1.5;
        assert!(vad.voiced(&[above; 160]));
        // Just below the default threshold should not be voiced.
        let below = config.energy_threshold * 0.5;
        assert!(!vad.voiced(&[below; 160]));
    }

    #[test]
    fn energy_vad_threshold_is_configurable() {
        let high = 0.5;
        let low = 0.01;

        let mut high_vad = EnergyVad::new(high);
        assert!(!high_vad.voiced(&[0.1; 160]));

        let mut low_vad = EnergyVad::new(low);
        assert!(low_vad.voiced(&[0.05; 160]));
    }

    /// Sherpa-gated: the Silero model downloads (~600 KB, network on first
    /// run) and the endpoint constructs and reports silence, proving the model
    /// cache → `SileroEndpoint` path works end to end. Skipped unless the
    /// `sherpa` feature is enabled.
    #[cfg(feature = "sherpa")]
    #[test]
    fn silero_endpoint_constructs_from_model_cache() {
        let cache_dir = std::env::temp_dir().join("takusu-vad-test-cache");
        let cache = crate::ModelCache::new(&cache_dir);
        let model = cache
            .ensure_silero_vad()
            .expect("download silero model from cache");
        let mut endpoint = SileroEndpoint::new(&model, SHERPA_SAMPLE_RATE as i32, 0.5, 60.0)
            .expect("construct Silero endpoint");
        // Feeding silence must not panic and must not open an utterance.
        for _ in 0..20 {
            assert_eq!(endpoint.push(&[0.0; 512]), None, "silence must not fire");
        }
        assert!(!endpoint.has_speech());
    }
}
