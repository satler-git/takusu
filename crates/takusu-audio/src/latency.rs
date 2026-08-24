//! Latency budget measurement for the voice pipeline.
//!
//! The resident-agent design calls for measuring the interval from speech
//! endpoint (or user action) to the first TTS audio block. These measurements
//! are collected in a `LatencyBudget` and can be reported in the PR or logged at
//! runtime.

use std::collections::BTreeMap;
use std::time::{Duration, Instant};

/// Named checkpoints in the resident voice pipeline.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[non_exhaustive]
pub enum LatencyCheckpoint {
    /// VAD detected the end of the user's utterance.
    VadEndpoint,
    /// ASR produced its final transcript.
    AsrFinal,
    /// LLM turn started.
    LlmStart,
    /// First TTS text block was emitted by the LLM.
    FirstTtsText,
    /// First TTS audio chunk was received from the TTS backend.
    FirstTtsAudio,
    /// First TTS audio chunk started playing on the output device.
    PlaybackStart,
    /// TTS playback finished.
    PlaybackFinished,
    /// Custom checkpoint, e.g. a platform-specific gate.
    Custom(&'static str),
}

impl std::fmt::Display for LatencyCheckpoint {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            LatencyCheckpoint::VadEndpoint => write!(f, "vad_endpoint"),
            LatencyCheckpoint::AsrFinal => write!(f, "asr_final"),
            LatencyCheckpoint::LlmStart => write!(f, "llm_start"),
            LatencyCheckpoint::FirstTtsText => write!(f, "first_tts_text"),
            LatencyCheckpoint::FirstTtsAudio => write!(f, "first_tts_audio"),
            LatencyCheckpoint::PlaybackStart => write!(f, "playback_start"),
            LatencyCheckpoint::PlaybackFinished => write!(f, "playback_finished"),
            LatencyCheckpoint::Custom(s) => write!(f, "custom:{s}"),
        }
    }
}

/// A single latency measurement between two checkpoints.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LatencySpan {
    pub from: LatencyCheckpoint,
    pub to: LatencyCheckpoint,
    pub duration: Duration,
}

impl LatencySpan {
    pub fn as_millis(&self) -> f64 {
        self.duration.as_secs_f64() * 1000.0
    }
}

/// Collects timestamped checkpoints and produces latency spans.
#[derive(Debug, Clone, Default)]
pub struct LatencyBudget {
    start: Option<Instant>,
    checkpoints: BTreeMap<LatencyCheckpoint, Instant>,
}

impl LatencyBudget {
    /// Create a fresh budget. The first `record` implicitly starts the clock if
    /// no start is set.
    pub fn new() -> Self {
        Self::default()
    }

    /// Set the start time explicitly. Any `record` before this is ignored for
    /// span calculations.
    pub fn start(&mut self) {
        let now = Instant::now();
        self.start = Some(now);
        // Drop checkpoints that predate the new start so they cannot cause
        // negative durations later.
        self.checkpoints.retain(|_, v| *v >= now);
    }

    /// Record a checkpoint at the current instant.
    pub fn record(&mut self, checkpoint: LatencyCheckpoint) {
        let now = Instant::now();
        if self.start.is_none() {
            self.start = Some(now);
        }
        self.checkpoints.insert(checkpoint, now);
    }

    /// Return the elapsed time from the start to the given checkpoint, if both
    /// exist.
    pub fn elapsed_to(&self, checkpoint: LatencyCheckpoint) -> Option<Duration> {
        let start = self.start?;
        let at = self.checkpoints.get(&checkpoint)?;
        at.checked_duration_since(start)
    }

    /// Return the span between two recorded checkpoints.
    pub fn span(&self, from: LatencyCheckpoint, to: LatencyCheckpoint) -> Option<LatencySpan> {
        let start = self.checkpoints.get(&from)?;
        let end = self.checkpoints.get(&to)?;
        Some(LatencySpan {
            from,
            to,
            duration: end.checked_duration_since(*start)?,
        })
    }

    /// Return all recorded checkpoints in order.
    pub fn checkpoints(&self) -> Vec<(LatencyCheckpoint, Duration)> {
        let start = self.start.unwrap_or_else(Instant::now);
        self.checkpoints
            .iter()
            .filter_map(|(k, v)| v.checked_duration_since(start).map(|d| (*k, d)))
            .collect()
    }

    /// Return the most useful spans for the voice pipeline.
    pub fn summary(&self) -> Vec<LatencySpan> {
        let mut spans = Vec::new();
        let pairs = [
            (LatencyCheckpoint::VadEndpoint, LatencyCheckpoint::AsrFinal),
            (LatencyCheckpoint::AsrFinal, LatencyCheckpoint::FirstTtsText),
            (
                LatencyCheckpoint::FirstTtsText,
                LatencyCheckpoint::FirstTtsAudio,
            ),
            (
                LatencyCheckpoint::FirstTtsAudio,
                LatencyCheckpoint::PlaybackStart,
            ),
            (
                LatencyCheckpoint::VadEndpoint,
                LatencyCheckpoint::PlaybackStart,
            ),
        ];
        for (from, to) in pairs {
            if let Some(span) = self.span(from, to) {
                spans.push(span);
            }
        }
        spans
    }

    /// Format the summary as a single line.
    pub fn report(&self) -> String {
        let mut out = String::new();
        for span in self.summary() {
            if !out.is_empty() {
                out.push(' ');
            }
            out.push_str(&format!(
                "{}->{}={:.1}ms",
                span.from,
                span.to,
                span.as_millis()
            ));
        }
        out
    }

    /// Reset the budget, discarding all recorded checkpoints.
    pub fn reset(&mut self) {
        *self = Self::default();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn budget_records_and_reports_spans() {
        let mut budget = LatencyBudget::new();
        budget.start();
        std::thread::sleep(Duration::from_millis(2));
        budget.record(LatencyCheckpoint::VadEndpoint);
        std::thread::sleep(Duration::from_millis(3));
        budget.record(LatencyCheckpoint::PlaybackStart);

        let span = budget
            .span(
                LatencyCheckpoint::VadEndpoint,
                LatencyCheckpoint::PlaybackStart,
            )
            .expect("span exists");
        assert!(span.duration >= Duration::from_millis(3));

        let report = budget.report();
        assert!(report.contains("vad_endpoint->playback_start"));
    }

    #[test]
    fn budget_start_is_implicit_on_first_record() {
        let mut budget = LatencyBudget::new();
        budget.record(LatencyCheckpoint::AsrFinal);
        assert!(budget.elapsed_to(LatencyCheckpoint::AsrFinal).is_some());
    }

    #[test]
    fn summary_only_includes_available_pairs() {
        let mut budget = LatencyBudget::new();
        budget.record(LatencyCheckpoint::VadEndpoint);
        budget.record(LatencyCheckpoint::PlaybackStart);
        let summary = budget.summary();
        assert_eq!(summary.len(), 1);
        assert_eq!(summary[0].from, LatencyCheckpoint::VadEndpoint);
        assert_eq!(summary[0].to, LatencyCheckpoint::PlaybackStart);
    }
}
