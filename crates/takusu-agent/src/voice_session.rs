//! Continuous voice session loop.
//!
//! [`VoiceSession`] runs the minimal resident voice loop: after an explicit
//! start it repeatedly captures one utterance, routes it through the agent,
//! optionally speaks the reply, and then keeps listening until the user ends
//! the session or an idle timeout fires. The session is backend-agnostic: it
//! drives a [`VoiceSessionIo`] implementation (the concrete recorder/ASR/TTS
//! wiring lives in `audio.rs` for the desktop runtime and behind the platform
//! module for mobile), which keeps the state-machine and modality logic pure
//! and unit-testable.

use std::time::{Duration, Instant};

use thiserror::Error;

use crate::capability::InputPath;
use crate::{AgentError, TurnResult};

/// How a turn entered the session.
///
/// Modality is decided here so the agent only automatically speaks turns that
/// began with voice input; text and background events follow the reactive
/// private-channel rules instead of short-circuiting to speech.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InputOrigin {
    /// The user spoke the utterance (session started by voice).
    Voice,
    /// The user typed it into the open session.
    Text,
    /// A reactive event is being handled inside the session.
    Background,
}

impl InputOrigin {
    /// Whether a reply for a turn of this origin should auto-speak.
    ///
    /// Only voice-origin turns auto-speak inside a session. Text and
    /// background turns surface through the ordinary notification path.
    pub fn auto_speaks(self) -> bool {
        matches!(self, InputOrigin::Voice)
    }
}

/// A single processed turn with the information the loop needs to continue.
#[derive(Debug)]
pub struct ProcessedTurn {
    pub result: TurnResult,
}

/// Recoverable failures while capturing or processing an utterance.
#[derive(Debug, Error)]
pub enum VoiceSessionError {
    #[error("capture failed: {0}")]
    Capture(String),
    #[error("agent turn failed: {0}")]
    Agent(String),
    #[error("io terminated by user")]
    UserCancelled,
}

impl From<AgentError> for VoiceSessionError {
    fn from(e: AgentError) -> Self {
        Self::Agent(e.to_string())
    }
}

/// The concrete capture/process backend driving one utterance.
///
/// `capture` records until VAD endpointing (or an explicit stop) and returns
/// `None` when no speech was detected. `process` runs the agent turn and
/// speaks the reply only when `origin.auto_speaks()`.
#[async_trait::async_trait]
pub trait VoiceSessionIo: Send {
    /// Capture one utterance. Returns `None` when the user cancelled or no
    /// speech began, which the loop treats as a reason to keep listening or
    /// exit depending on the timeout path.
    async fn capture(&mut self, origin: InputOrigin) -> Result<Option<String>, VoiceSessionError>;
    /// Route an utterance through the agent and deliver the reply.
    async fn process(
        &mut self,
        text: &str,
        origin: InputOrigin,
        input_path: InputPath,
    ) -> Result<ProcessedTurn, VoiceSessionError>;
}

/// Why a session stopped its loop.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionOutcome {
    /// The user explicitly ended the session.
    UserExited,
    /// No activity for more than [`VoiceSession::idle_timeout`].
    IdleTimeout,
    /// A terminal backend failure.
    Failed,
}

/// Configuration for the session loop.
#[derive(Debug, Clone)]
pub struct VoiceSessionConfig {
    /// How long the loop may wait for a new utterance (and between turns)
    /// before exiting on its own.
    pub idle_timeout: Duration,
}

impl Default for VoiceSessionConfig {
    fn default() -> Self {
        Self {
            idle_timeout: Duration::from_secs(30),
        }
    }
}

/// A single continuous voice session. Owns no recording or TTS state; that
/// lives in the [`VoiceSessionIo`] implementation. This keeps session lifecycle
/// (idle timeout, continuation, exit) pure and testable without audio devices.
pub struct VoiceSession {
    config: VoiceSessionConfig,
    /// The origin that frames every free turn in this session. Explicit voice
    /// sessions default to [`InputOrigin::Voice`].
    origin: InputOrigin,
    /// The trusted input path for authorization. Defaults to
    /// [`InputPath::ExplicitVoiceSession`] for voice-origin sessions.
    input_path: InputPath,
}

impl VoiceSession {
    /// Create a session that frames turns with the given origin and input path.
    pub fn new(config: VoiceSessionConfig, origin: InputOrigin, input_path: InputPath) -> Self {
        Self {
            config,
            origin,
            input_path,
        }
    }

    /// Run the session against `io`.
    ///
    /// The loop is `begin -> capture -> process -> capture -> ...` until the
    /// user exits or the idle deadline (reset after each processed utterance)
    /// elapses. When `capture` reports no speech the deadline is not reset, so
    /// sustained audio that never becomes an utterance does not hold the
    /// session open forever.
    pub async fn run<I>(self, io: &mut I) -> SessionOutcome
    where
        I: VoiceSessionIo,
    {
        let deadline = Instant::now() + self.config.idle_timeout;
        let idle = tokio::time::sleep_until(tokio::time::Instant::from_std(deadline));
        tokio::pin!(idle);

        loop {
            tokio::select! {
                _ = &mut idle => {
                    return SessionOutcome::IdleTimeout;
                }
                captured = io.capture(self.origin) => {
                    match captured {
                        Ok(Some(text)) => {
                            if io.process(&text, self.origin, self.input_path)
                                .await
                                .is_err()
                            {
                                return SessionOutcome::Failed;
                            }
                            // Real activity resets the idle deadline so the
                            // session survives long conversations.
                            idle.as_mut().reset(tokio::time::Instant::now() + self.config.idle_timeout);
                        }
                        Ok(None) => {
                            // No speech this attempt; yield so the idle timer
                            // advances instead of busy-spinning on an always-
                            // ready capture future, then keep listening until
                            // the idle deadline fires.
                            tokio::task::yield_now().await;
                        }
                        Err(VoiceSessionError::UserCancelled) => {
                            return SessionOutcome::UserExited;
                        }
                        Err(_) => {
                            return SessionOutcome::Failed;
                        }
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A scripted backend that returns a fixed sequence of transcript results
    /// followed by empty captures, with a pluggable failure.
    struct ScriptedIo {
        voice: Vec<Option<String>>,
        processed: usize,
        /// When set, subsequent captures cancel the session.
        exit_on: usize,
        capture_index: usize,
    }

    #[async_trait::async_trait]
    impl VoiceSessionIo for ScriptedIo {
        async fn capture(
            &mut self,
            _origin: InputOrigin,
        ) -> Result<Option<String>, VoiceSessionError> {
            if self.capture_index >= self.exit_on {
                return Err(VoiceSessionError::UserCancelled);
            }
            let result = self.voice.get(self.capture_index).cloned().flatten();
            self.capture_index += 1;
            Ok(result)
        }

        async fn process(
            &mut self,
            _text: &str,
            _origin: InputOrigin,
            _input_path: InputPath,
        ) -> Result<ProcessedTurn, VoiceSessionError> {
            self.processed += 1;
            Ok(ProcessedTurn {
                result: TurnResult {
                    text: String::new(),
                    changes: Vec::new(),
                    schedule_dirty: false,
                    approval_request: None,
                    presentation: None,
                },
            })
        }
    }

    #[test]
    fn auto_speaks_only_for_voice_origin() {
        assert!(InputOrigin::Voice.auto_speaks());
        assert!(!InputOrigin::Text.auto_speaks());
        assert!(!InputOrigin::Background.auto_speaks());
    }

    #[tokio::test]
    async fn user_ends_the_session_explicitly() {
        let session = VoiceSession::new(
            VoiceSessionConfig {
                idle_timeout: Duration::from_secs(60),
            },
            InputOrigin::Voice,
            InputPath::ExplicitVoiceSession,
        );
        let mut io = ScriptedIo {
            voice: vec![None; 100],
            processed: 0,
            exit_on: 3,
            capture_index: 0,
        };
        let outcome = session.run(&mut io).await;
        assert_eq!(outcome, SessionOutcome::UserExited);
        assert_eq!(io.processed, 0);
    }

    #[tokio::test]
    async fn multiple_voice_turns_continue_in_one_session() {
        let session = VoiceSession::new(
            VoiceSessionConfig {
                idle_timeout: Duration::from_millis(300),
            },
            InputOrigin::Voice,
            InputPath::ExplicitVoiceSession,
        );
        let mut io = ScriptedIo {
            voice: vec![Some("one".into()), Some("two".into())],
            processed: 0,
            exit_on: usize::MAX,
            capture_index: 0,
        };
        // Both utterances process before the short idle deadline fires.
        let outcome = session.run(&mut io).await;
        assert_eq!(outcome, SessionOutcome::IdleTimeout);
        assert_eq!(io.processed, 2);
    }

    #[tokio::test]
    async fn idle_timeout_ends_the_session() {
        // idle_timeout is elided by selecting capture; with an empty capture
        // completing instantly the loop would spin forever, so give the
        // capture a delay against a short idle timeout.
        struct SlowIo;
        #[async_trait::async_trait]
        impl VoiceSessionIo for SlowIo {
            async fn capture(
                &mut self,
                _o: InputOrigin,
            ) -> Result<Option<String>, VoiceSessionError> {
                tokio::time::sleep(Duration::from_millis(50)).await;
                Ok(None)
            }
            async fn process(
                &mut self,
                _t: &str,
                _o: InputOrigin,
                _p: InputPath,
            ) -> Result<ProcessedTurn, VoiceSessionError> {
                unreachable!()
            }
        }
        let session = VoiceSession::new(
            VoiceSessionConfig {
                idle_timeout: Duration::from_millis(120),
            },
            InputOrigin::Voice,
            InputPath::ExplicitVoiceSession,
        );
        let mut io = SlowIo;
        let outcome = session.run(&mut io).await;
        assert_eq!(outcome, SessionOutcome::IdleTimeout);
    }
}
