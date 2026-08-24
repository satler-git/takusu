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
//!
//! WI-19: conversation polish adds:
//! - barge-in detection while the assistant is speaking,
//! - a per-turn timeout so a stuck turn does not hang the session,
//! - per-turn error recovery so recoverable failures return to listening instead
//!   of killing the session.

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

/// Recoverable failures while capturing or processing an utterance.
#[derive(Debug, Clone, Error)]
pub enum VoiceSessionError {
    #[error("capture failed: {0}")]
    Capture(String),
    #[error("agent turn failed: {0}")]
    Agent(String),
    /// The turn exceeded its time budget. The session should recover and keep
    /// listening.
    #[error("turn timed out")]
    Timeout,
    #[error("io terminated by user")]
    UserCancelled,
}

impl VoiceSessionError {
    /// Whether the error is recoverable: the session can keep listening instead
    /// of exiting.
    pub fn is_recoverable(&self) -> bool {
        matches!(self, Self::Timeout)
    }
}

impl From<AgentError> for VoiceSessionError {
    fn from(e: AgentError) -> Self {
        Self::Agent(e.to_string())
    }
}

/// A single processed turn with the information the loop needs to continue.
#[derive(Debug, Clone)]
pub struct ProcessedTurn {
    pub result: TurnResult,
    /// If the user barged in while the assistant was speaking, this contains
    /// the captured interruption text. The session loop routes it as the next
    /// user input instead of returning to open listening.
    pub barge_in: Option<String>,
}

/// The concrete capture/process backend driving one utterance.
///
/// `capture` records until VAD endpointing (or an explicit stop) and returns
/// `None` when no speech was detected. `process` runs the agent turn and
/// speaks the reply only when `origin.auto_speaks()`. `process` may also
/// detect a barge-in and return the interruption text in
/// [`ProcessedTurn::barge_in`].
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
    /// No activity for more than [`VoiceSessionConfig::idle_timeout`].
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
    /// Maximum time a single turn (capture or process) may take before the
    /// session treats it as a timeout and recovers.
    pub turn_timeout: Duration,
    /// How many consecutive recoverable errors are allowed before the session
    /// gives up and returns [`SessionOutcome::Failed`].
    pub max_consecutive_errors: usize,
}

impl Default for VoiceSessionConfig {
    fn default() -> Self {
        Self {
            idle_timeout: Duration::from_secs(30),
            turn_timeout: Duration::from_secs(60),
            max_consecutive_errors: 3,
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
    ///
    /// A `process` that returns a [`ProcessedTurn::barge_in`] immediately routes
    /// the barge-in text into the next `process` instead of waiting for a new
    /// utterance. Recoverable errors are counted; after
    /// `max_consecutive_errors` the session exits with [`SessionOutcome::Failed`].
    pub async fn run<I>(self, io: &mut I) -> SessionOutcome
    where
        I: VoiceSessionIo,
    {
        let deadline = Instant::now() + self.config.idle_timeout;
        let idle = tokio::time::sleep_until(tokio::time::Instant::from_std(deadline));
        tokio::pin!(idle);

        let turn_timeout = self.config.turn_timeout;
        let mut consecutive_errors = 0usize;
        let mut barge_in: Option<String> = None;

        loop {
            let turn = async {
                let text = match barge_in.take() {
                    Some(text) => text,
                    None => match io.capture(self.origin).await {
                        Ok(Some(text)) => text,
                        Ok(None) => return Ok(None),
                        Err(VoiceSessionError::UserCancelled) => {
                            return Err(VoiceSessionError::UserCancelled);
                        }
                        Err(e) => return Err(e),
                    },
                };

                io.process(&text, self.origin, self.input_path)
                    .await
                    .map(Some)
            };

            tokio::select! {
                _ = &mut idle => {
                    return SessionOutcome::IdleTimeout;
                }
                timed = tokio::time::timeout(turn_timeout, turn) => {
                    let result = match timed {
                        Ok(inner) => inner,
                        Err(_) => Err(VoiceSessionError::Timeout),
                    };
                    match result {
                        Ok(None) => {
                            // No speech this attempt; yield so the idle timer
                            // advances instead of busy-spinning on an always-
                            // ready capture future, then keep listening until
                            // the idle deadline fires.
                            tokio::task::yield_now().await;
                        }
                        Ok(Some(ProcessedTurn { result: _, barge_in: Some(text) })) => {
                            consecutive_errors = 0;
                            // Real activity resets the idle deadline.
                            idle.as_mut().reset(tokio::time::Instant::now() + self.config.idle_timeout);
                            barge_in = Some(text);
                        }
                        Ok(Some(ProcessedTurn { result: _, barge_in: None })) => {
                            consecutive_errors = 0;
                            // Real activity resets the idle deadline so the
                            // session survives long conversations.
                            idle.as_mut().reset(tokio::time::Instant::now() + self.config.idle_timeout);
                        }
                        Err(VoiceSessionError::UserCancelled) => {
                            return SessionOutcome::UserExited;
                        }
                        Err(e) if e.is_recoverable() => {
                            consecutive_errors += 1;
                            if consecutive_errors >= self.config.max_consecutive_errors {
                                return SessionOutcome::Failed;
                            }
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
        processed: Vec<String>,
        /// When set, subsequent captures cancel the session.
        exit_on: usize,
        capture_index: usize,
        /// Sequence of process results, cycled if longer than the test needs.
        process_results: Vec<Result<ProcessedTurn, VoiceSessionError>>,
    }

    impl ScriptedIo {
        fn with_voice(voice: Vec<Option<String>>) -> Self {
            Self {
                voice,
                processed: Vec::new(),
                exit_on: usize::MAX,
                capture_index: 0,
                process_results: vec![Ok(ProcessedTurn {
                    result: TurnResult {
                        text: String::new(),
                        changes: Vec::new(),
                        schedule_dirty: false,
                        approval_request: None,
                        presentation: None,
                        intake_state: None,
                    },
                    barge_in: None,
                })],
            }
        }
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
            text: &str,
            _origin: InputOrigin,
            _input_path: InputPath,
        ) -> Result<ProcessedTurn, VoiceSessionError> {
            self.processed.push(text.to_string());
            let index = self.processed.len() - 1;
            self.process_results.get(index).cloned().unwrap_or_else(|| {
                Ok(ProcessedTurn {
                    result: TurnResult {
                        text: String::new(),
                        changes: Vec::new(),
                        schedule_dirty: false,
                        approval_request: None,
                        presentation: None,
                        intake_state: None,
                    },
                    barge_in: None,
                })
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
                ..Default::default()
            },
            InputOrigin::Voice,
            InputPath::ExplicitVoiceSession,
        );
        let mut io = ScriptedIo::with_voice(vec![None; 100]);
        io.exit_on = 3;
        let outcome = session.run(&mut io).await;
        assert_eq!(outcome, SessionOutcome::UserExited);
        assert!(io.processed.is_empty());
    }

    #[tokio::test]
    async fn multiple_voice_turns_continue_in_one_session() {
        let session = VoiceSession::new(
            VoiceSessionConfig {
                idle_timeout: Duration::from_millis(300),
                ..Default::default()
            },
            InputOrigin::Voice,
            InputPath::ExplicitVoiceSession,
        );
        let mut io = ScriptedIo::with_voice(vec![Some("one".into()), Some("two".into())]);
        let outcome = session.run(&mut io).await;
        assert_eq!(outcome, SessionOutcome::IdleTimeout);
        assert_eq!(io.processed, vec!["one", "two"]);
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
                ..Default::default()
            },
            InputOrigin::Voice,
            InputPath::ExplicitVoiceSession,
        );
        let mut io = SlowIo;
        let outcome = session.run(&mut io).await;
        assert_eq!(outcome, SessionOutcome::IdleTimeout);
    }

    #[tokio::test]
    async fn barge_in_reroutes_into_immediate_next_process() {
        let session = VoiceSession::new(
            VoiceSessionConfig {
                idle_timeout: Duration::from_millis(100),
                ..Default::default()
            },
            InputOrigin::Voice,
            InputPath::ExplicitVoiceSession,
        );
        let mut io = ScriptedIo::with_voice(vec![Some("first".into())]);
        io.process_results = vec![
            Ok(ProcessedTurn {
                result: TurnResult {
                    text: "reply".into(),
                    changes: Vec::new(),
                    schedule_dirty: false,
                    approval_request: None,
                    presentation: None,
                    intake_state: None,
                },
                barge_in: Some("wait".into()),
            }),
            Ok(ProcessedTurn {
                result: TurnResult {
                    text: "ok".into(),
                    changes: Vec::new(),
                    schedule_dirty: false,
                    approval_request: None,
                    presentation: None,
                    intake_state: None,
                },
                barge_in: None,
            }),
        ];
        let outcome = session.run(&mut io).await;
        assert_eq!(outcome, SessionOutcome::IdleTimeout);
        assert_eq!(io.processed, vec!["first", "wait"]);
    }

    #[tokio::test]
    async fn recoverable_errors_allow_session_to_continue() {
        let session = VoiceSession::new(
            VoiceSessionConfig {
                idle_timeout: Duration::from_millis(500),
                max_consecutive_errors: 2,
                ..Default::default()
            },
            InputOrigin::Voice,
            InputPath::ExplicitVoiceSession,
        );
        let mut io = ScriptedIo::with_voice(vec![Some("one".into()), Some("two".into())]);
        io.process_results = vec![
            Err(VoiceSessionError::Timeout),
            Ok(ProcessedTurn {
                result: TurnResult {
                    text: "ok".into(),
                    changes: Vec::new(),
                    schedule_dirty: false,
                    approval_request: None,
                    presentation: None,
                    intake_state: None,
                },
                barge_in: None,
            }),
        ];
        let outcome = session.run(&mut io).await;
        assert_eq!(outcome, SessionOutcome::IdleTimeout);
        assert_eq!(io.processed, vec!["one", "two"]);
    }

    #[tokio::test]
    async fn terminal_error_ends_session_immediately() {
        let session = VoiceSession::new(
            VoiceSessionConfig {
                idle_timeout: Duration::from_secs(60),
                ..Default::default()
            },
            InputOrigin::Voice,
            InputPath::ExplicitVoiceSession,
        );
        let mut io = ScriptedIo::with_voice(vec![Some("one".into())]);
        io.process_results = vec![Err(VoiceSessionError::Capture("mic".into()))];
        let outcome = session.run(&mut io).await;
        assert_eq!(outcome, SessionOutcome::Failed);
    }

    #[tokio::test]
    async fn turn_timeout_is_recoverable() {
        let session = VoiceSession::new(
            VoiceSessionConfig {
                idle_timeout: Duration::from_millis(180),
                turn_timeout: Duration::from_millis(100),
                max_consecutive_errors: 5,
            },
            InputOrigin::Voice,
            InputPath::ExplicitVoiceSession,
        );
        struct SlowProcessIo;
        #[async_trait::async_trait]
        impl VoiceSessionIo for SlowProcessIo {
            async fn capture(
                &mut self,
                _o: InputOrigin,
            ) -> Result<Option<String>, VoiceSessionError> {
                Ok(Some("hello".into()))
            }
            async fn process(
                &mut self,
                _t: &str,
                _o: InputOrigin,
                _p: InputPath,
            ) -> Result<ProcessedTurn, VoiceSessionError> {
                tokio::time::sleep(Duration::from_secs(10)).await;
                Ok(ProcessedTurn {
                    result: TurnResult {
                        text: String::new(),
                        changes: Vec::new(),
                        schedule_dirty: false,
                        approval_request: None,
                        presentation: None,
                        intake_state: None,
                    },
                    barge_in: None,
                })
            }
        }
        let mut io = SlowProcessIo;
        let outcome = session.run(&mut io).await;
        assert_eq!(outcome, SessionOutcome::IdleTimeout);
    }
}
