//! Acoustic echo cancellation and barge-in detection support.
//!
//! The resident-agent voice loop keeps the microphone open while the assistant
//! is speaking. Without echo cancellation the assistant's own TTS would trigger
//! the VAD and make barge-in impossible. This module provides a small
//! frame-based AEC interface plus evaluation helpers so platforms can supply
//! their own canceller (e.g. hardware AEC on Android) while desktop falls back
//! to a software NLMS canceller.

use std::time::Duration;

use serde::{Deserialize, Serialize};

use crate::vad::{Endpoint, VadEvent};

/// Acoustic echo canceller state.
///
/// `reference` is the signal the device is playing (assistant TTS). `capture`
/// is the raw microphone input. The canceller returns a residual that should
/// contain the user's voice with the played audio removed as much as possible.
pub trait Aec: Send {
    /// Feed one frame of reference and one frame of capture samples and write
    /// the residual into `out`. Both input slices must be the same length and
    /// at the same sample rate (typically 16 kHz mono). `out` is resized to
    /// `capture.len()` and can be reused across calls to avoid per-frame
    /// allocation.
    fn process(&mut self, reference: &[f32], capture: &[f32], out: &mut Vec<f32>);

    /// Reset the filter state. Does not clear the delay line unless the
    /// implementation chooses to.
    fn reset(&mut self);

    /// Whether this AEC is actively cancelling. A no-op or disabled canceller
    /// returns `false`.
    fn is_active(&self) -> bool;
}

/// Configuration for the software NLMS echo canceller.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct AecConfig {
    /// Length of the adaptive filter in samples. At 16 kHz, 1600 samples is
    /// 100 ms of tail, which covers a realistic loudspeaker-to-microphone echo
    /// tail including a small amount of room reverberation. Increase this if
    /// the device is in a reverberant space; decrease it if CPU is limited and
    /// the environment is known to have a very short tail.
    pub filter_len: usize,
    /// Step size (mu). Larger values adapt faster but can diverge.
    pub step_size: f32,
    /// Regularization added to the reference energy to avoid division by zero.
    pub delta: f32,
    /// Number of frames to skip after `reset` while the filter converges.
    pub warm_up_frames: usize,
    /// When the reference RMS is below this threshold the filter stops adapting
    /// so near-silence does not misalign the coefficients.
    pub reference_floor: f32,
}

impl Default for AecConfig {
    fn default() -> Self {
        Self {
            filter_len: 1600,
            step_size: 0.08,
            delta: 1e-10,
            warm_up_frames: 16,
            reference_floor: 0.001,
        }
    }
}

/// AEC effectiveness measurement returned by the evaluation harness.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct AecEffectiveness {
    /// Echo reduction in dB, measured on a synthetic echo-plus-speech fixture.
    /// Positive values mean the residual has less echo than the raw capture.
    pub echo_reduction_db: f32,
    /// Proportion of the user's speech energy that survives cancellation.
    /// Values near 1.0 are good; low values mean the canceller is eating the
    /// user's voice.
    ///
    /// This is the total residual energy divided by the original speech energy,
    /// so it is an upper bound: any residual echo that the AEC failed to remove
    /// is also counted as "retained" speech. A high value alone does not prove
    /// the AEC is working well; use it together with `echo_reduction_db`.
    pub speech_retention: f32,
    /// Number of frames processed in the measurement.
    pub frames: usize,
}

impl AecEffectiveness {
    /// Whether the measurement should be considered usable for barge-in.
    pub fn is_usable(&self) -> bool {
        self.echo_reduction_db > 6.0 && self.speech_retention > 0.5
    }
}

/// AEC that does nothing. Used when no canceller is available or when the
/// platform is known to handle echo cancellation itself.
pub struct NoOpAec;

impl Aec for NoOpAec {
    fn process(&mut self, _reference: &[f32], capture: &[f32], out: &mut Vec<f32>) {
        out.clear();
        out.extend_from_slice(capture);
    }

    fn reset(&mut self) {}

    fn is_active(&self) -> bool {
        false
    }
}

/// Normalized least-mean-squares software echo canceller.
///
/// This is a minimal, cross-platform fallback. It adapts a single filter on the
/// assumption that the reference and capture are already time-aligned and at the
/// same sample rate. It is intended to make barge-in possible on desktop where
/// no OS-level AEC is available; it is not a replacement for a production
/// multi-band AEC.
pub struct NlmsAec {
    config: AecConfig,
    /// Filter coefficients.
    weights: Vec<f32>,
    /// Delay line holding the most recent reference samples.
    x: Vec<f32>,
    /// Head of the delay line.
    x_head: usize,
    /// Reusable window of the most recent `filter_len` reference samples.
    window_buf: Vec<f32>,
    /// Frames processed since the last reset, used for the warm-up period.
    frame_count: usize,
}

impl NlmsAec {
    /// Create a new NLMS canceller with the given configuration.
    pub fn new(config: AecConfig) -> Self {
        let filter_len = config.filter_len;
        Self {
            config,
            weights: vec![0.0; filter_len],
            x: vec![0.0; filter_len],
            x_head: 0,
            window_buf: Vec::with_capacity(filter_len),
            frame_count: 0,
        }
    }

    fn push_reference(&mut self, s: f32) {
        self.x[self.x_head] = s;
        self.x_head = if self.x_head == 0 {
            self.x.len() - 1
        } else {
            self.x_head - 1
        };
    }
}

impl Aec for NlmsAec {
    fn process(&mut self, reference: &[f32], capture: &[f32], out: &mut Vec<f32>) {
        debug_assert_eq!(
            reference.len(),
            capture.len(),
            "reference and capture should be the same length"
        );
        let len = capture.len();
        out.clear();
        out.reserve(len);
        for (i, &d) in capture.iter().enumerate() {
            let r = reference.get(i).copied().unwrap_or(0.0);
            self.push_reference(r);

            // Fill the reusable reference window from the circular delay line.
            // x_head is the next write index, so the latest sample is at
            // x_head + 1 (mod filter_len). The window is [x[n], x[n-1], ...].
            self.window_buf.clear();
            let filter_len = self.x.len();
            let mut j = (self.x_head + 1) % filter_len;
            for _ in 0..filter_len {
                self.window_buf.push(self.x[j]);
                j += 1;
                if j == filter_len {
                    j = 0;
                }
            }

            let y: f32 = self
                .weights
                .iter()
                .zip(self.window_buf.iter())
                .map(|(w, x)| w * x)
                .sum();
            let e = d - y;
            let reference_energy: f32 = self.window_buf.iter().map(|x| x * x).sum();

            // reference_floor is an RMS threshold, so compare against the
            // mean squared energy (energy / filter_len).
            let reference_floor_energy =
                self.config.reference_floor * self.config.reference_floor * filter_len as f32;

            // Update weights in-place using the reusable reference window.
            if self.frame_count >= self.config.warm_up_frames
                && reference_energy >= reference_floor_energy
            {
                let mu = self.config.step_size / (reference_energy + self.config.delta);
                for (w, x) in self.weights.iter_mut().zip(self.window_buf.iter()) {
                    *w += mu * e * *x;
                }
            }

            out.push(e);
        }
        self.frame_count += 1;
    }

    fn reset(&mut self) {
        self.weights.fill(0.0);
        self.x.fill(0.0);
        self.x_head = 0;
        self.window_buf.fill(0.0);
        self.frame_count = 0;
    }

    fn is_active(&self) -> bool {
        true
    }
}

/// Barge-in detector: listens to AEC residual with a VAD endpoint and reports
/// when the user starts speaking over TTS.
pub struct BargeInDetector<E: Endpoint> {
    /// True when TTS is currently playing and we are listening for the user.
    active: bool,
    /// Whether a barge-in was observed while active.
    triggered: bool,
    endpoint: E,
    /// Minimum TTS play time before barge-in can fire, so transients at the
    /// start of playback do not immediately trigger.
    min_play_frames: usize,
    play_frames: usize,
}

impl<E: Endpoint> BargeInDetector<E> {
    /// Create a detector wrapping the given VAD endpoint.
    pub fn new(endpoint: E, min_play_time: Duration) -> Self {
        let min_play_frames = (min_play_time.as_millis() as f32 / 10.0).ceil() as usize;
        Self {
            active: false,
            triggered: false,
            endpoint,
            min_play_frames,
            play_frames: 0,
        }
    }

    /// Start listening for a barge-in. Resets the endpoint and the play frame
    /// counter.
    pub fn start(&mut self) {
        self.active = true;
        self.triggered = false;
        self.endpoint.reset();
        self.play_frames = 0;
    }

    /// Stop listening. A subsequent [`Self::take_triggered`] still reports the
    /// last trigger until it is consumed.
    pub fn stop(&mut self) {
        self.active = false;
    }

    /// Feed one frame of residual samples. Returns `Some(SpeechStart)` when a
    /// barge-in is detected. The caller is responsible for continuing to feed
    /// audio until `SpeechEnd` to capture the full interruption.
    pub fn push(&mut self, residual: &[f32]) -> Option<VadEvent> {
        if !self.active {
            return None;
        }
        self.play_frames += 1;
        if self.play_frames <= self.min_play_frames {
            return None;
        }
        let event = self.endpoint.push(residual);
        if event == Some(VadEvent::SpeechStart) {
            self.triggered = true;
        }
        event
    }

    /// Whether a barge-in was detected since the last `start`.
    pub fn triggered(&self) -> bool {
        self.triggered
    }

    /// Consume and return the trigger flag.
    pub fn take_triggered(&mut self) -> bool {
        std::mem::take(&mut self.triggered)
    }
}

/// Evaluate an AEC implementation on a synthetic echo fixture.
///
/// `echo` is the echo-only signal that the reference produces in the
/// microphone. `speech` is the near-end user speech. The function returns the
/// echo reduction and speech retention measured on the residual.
///
/// Note: `speech_retention` is computed as the ratio of total residual energy
/// to the original speech energy. Residual echo that the AEC did not suppress
/// will therefore inflate this metric. Treat `is_usable()` as a smoke test,
/// not a guarantee that real-world echo cancellation will be good enough.
pub fn evaluate_aec(
    aec: &mut dyn Aec,
    reference: &[f32],
    echo: &[f32],
    speech: &[f32],
    frame_size: usize,
) -> AecEffectiveness {
    let min_len = reference.len().min(echo.len()).min(speech.len());
    let mut raw_capture_energy = 0.0f64;
    let mut residual_echo_energy = 0.0f64;
    let mut residual_speech_energy = 0.0f64;
    let mut original_speech_energy = 0.0f64;
    let mut frames = 0usize;

    for offset in (0..min_len).step_by(frame_size) {
        let end = (offset + frame_size).min(min_len);
        let r = &reference[offset..end];
        let e = &echo[offset..end];
        let s = &speech[offset..end];
        // The microphone signal contains echo and near-end speech only;
        // the reference is fed to the AEC separately.
        let capture: Vec<f32> = e.iter().zip(s.iter()).map(|(e, s)| e + s).collect();
        let mut residual = Vec::new();
        aec.process(r, &capture, &mut residual);

        for (&c, &r) in capture.iter().zip(s.iter()) {
            let echo_only = c - r;
            raw_capture_energy += (echo_only as f64).powi(2);
            original_speech_energy += (r as f64).powi(2);
        }
        for (&r, &s) in residual.iter().zip(s.iter()) {
            let residual_echo = r - s;
            residual_echo_energy += (residual_echo as f64).powi(2);
            residual_speech_energy += (r as f64).powi(2);
        }
        frames += 1;
    }

    let echo_reduction_db = if residual_echo_energy > 0.0 && raw_capture_energy > 0.0 {
        10.0 * (raw_capture_energy / residual_echo_energy).log10() as f32
    } else {
        0.0
    };
    let speech_retention = if original_speech_energy > 0.0 {
        (residual_speech_energy / original_speech_energy).min(1.0) as f32
    } else {
        1.0
    };

    AecEffectiveness {
        echo_reduction_db,
        speech_retention,
        frames,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::vad::{EnergyVad, VadEndpoint, VadEndpointConfig};
    use crate::wav::SHERPA_SAMPLE_RATE;

    #[test]
    fn no_op_aec_passes_capture_unchanged() {
        let mut aec = NoOpAec;
        let reference = [0.5, 0.5, 0.5];
        let capture = [0.1, 0.2, 0.3];
        let mut out = Vec::new();
        aec.process(&reference, &capture, &mut out);
        assert_eq!(out, capture.to_vec());
        assert!(!aec.is_active());
    }

    #[test]
    fn nlms_aec_reduces_synthetic_echo() {
        let config = AecConfig {
            filter_len: 32,
            step_size: 0.2,
            warm_up_frames: 0,
            ..Default::default()
        };
        let mut aec = NlmsAec::new(config);

        // 200 frames of 160 samples = about 2 s at 16 kHz.
        let frames = 200;
        let frame_size = 160;
        let mut reference = vec![0.0f32; frames * frame_size];
        let mut echo = vec![0.0f32; frames * frame_size];
        let mut speech = vec![0.0f32; frames * frame_size];

        // Reference: a 440 Hz tone. Echo: delayed, attenuated copy of the
        // reference. Speech: a 1 kHz tone, intermittent.
        for i in 0..frames * frame_size {
            let t = i as f32 / SHERPA_SAMPLE_RATE as f32;
            reference[i] = 0.3 * (2.0 * std::f32::consts::PI * 440.0 * t).sin();
            if i >= 16 {
                echo[i] = 0.15 * reference[i - 16];
            }
            if i % (frame_size * 4) < frame_size * 2 {
                speech[i] = 0.05 * (2.0 * std::f32::consts::PI * 1000.0 * t).sin();
            }
        }

        let result = evaluate_aec(&mut aec, &reference, &echo, &speech, frame_size);
        assert!(
            result.echo_reduction_db > 3.0,
            "expected some echo reduction, got {} dB",
            result.echo_reduction_db
        );
        assert!(
            result.speech_retention > 0.5,
            "expected most speech to survive, got {}",
            result.speech_retention
        );
    }

    #[test]
    fn barge_in_detector_ignores_first_frames() {
        let config = VadEndpointConfig {
            min_speech: Duration::from_millis(50),
            max_silence: Duration::from_millis(100),
            ..Default::default()
        };
        let endpoint = VadEndpoint::new(EnergyVad::new(0.01), SHERPA_SAMPLE_RATE, config);
        let mut detector = BargeInDetector::new(endpoint, Duration::from_millis(60));
        detector.start();

        // 40 frames of silence-sized zeros should not trigger.
        for _ in 0..40 {
            assert_eq!(detector.push(&[0.0; 160]), None);
        }

        // 5 voiced frames after the warm-up.
        for _ in 0..5 {
            let e = detector.push(&[0.5; 160]);
            if e == Some(VadEvent::SpeechStart) {
                return;
            }
        }
        assert!(detector.triggered(), "detector should have triggered");
    }

    #[test]
    fn aec_effectiveness_is_usable_for_barge_in() {
        let mut aec = NlmsAec::new(AecConfig {
            filter_len: 32,
            step_size: 0.2,
            warm_up_frames: 0,
            ..Default::default()
        });

        let frames = 200;
        let frame_size = 160;
        let mut reference = vec![0.0f32; frames * frame_size];
        let mut echo = vec![0.0f32; frames * frame_size];
        let mut speech = vec![0.0f32; frames * frame_size];

        for i in 0..frames * frame_size {
            let t = i as f32 / SHERPA_SAMPLE_RATE as f32;
            reference[i] = 0.3 * (2.0 * std::f32::consts::PI * 440.0 * t).sin();
            if i >= 16 {
                echo[i] = 0.15 * reference[i - 16];
            }
            if i % (frame_size * 4) < frame_size * 2 {
                speech[i] = 0.05 * (2.0 * std::f32::consts::PI * 1000.0 * t).sin();
            }
        }

        let result = evaluate_aec(&mut aec, &reference, &echo, &speech, frame_size);
        assert!(result.is_usable());
    }

    #[test]
    fn aec_default_filter_len_is_realistic() {
        let config = AecConfig::default();
        assert!(
            config.filter_len >= 800,
            "expected realistic default filter_len, got {}",
            config.filter_len
        );
    }

    #[test]
    fn nlms_aec_default_config_reduces_synthetic_echo() {
        let mut aec = NlmsAec::new(AecConfig::default());

        let frames = 200;
        let frame_size = 160;
        let mut reference = vec![0.0f32; frames * frame_size];
        let mut echo = vec![0.0f32; frames * frame_size];
        let mut speech = vec![0.0f32; frames * frame_size];

        for i in 0..frames * frame_size {
            let t = i as f32 / SHERPA_SAMPLE_RATE as f32;
            reference[i] = 0.3 * (2.0 * std::f32::consts::PI * 440.0 * t).sin();
            if i >= 16 {
                echo[i] = 0.15 * reference[i - 16];
            }
            if i % (frame_size * 4) < frame_size * 2 {
                speech[i] = 0.05 * (2.0 * std::f32::consts::PI * 1000.0 * t).sin();
            }
        }

        let result = evaluate_aec(&mut aec, &reference, &echo, &speech, frame_size);
        assert!(
            result.echo_reduction_db > 3.0,
            "expected some echo reduction with default config, got {} dB",
            result.echo_reduction_db
        );
        assert!(
            result.speech_retention > 0.5,
            "expected most speech to survive with default config, got {}",
            result.speech_retention
        );
    }
}
