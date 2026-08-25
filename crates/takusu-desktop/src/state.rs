//! Shared desktop state.
//!
//! The state is a small, interior-mutable snapshot of what the tray icon,
//! notification, and compact panel should display. It is updated from an SSE
//! stream fed by the local agent transport and from the planner event replay
//! loop.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, RwLock};

type OnChange = Arc<Mutex<Option<Arc<dyn Fn() + Send + Sync>>>>;

#[cfg(feature = "audio-device")]
use takusu_agent::{AgentConfig, TurnEvent};
use takusu_agent::{Presentation, SurfaceEvent, SurfaceSnapshot, SurfaceState};

use crate::config::Config;
use crate::presentation::{DesktopAction, DesktopPresentation};

#[cfg(feature = "audio-device")]
use crate::audio::{
    AmbientSessionHandle, VoiceSessionHandle, spawn_ambient_session, spawn_voice_session,
};

/// Errors the daemon can surface to the user.
#[derive(Debug, thiserror::Error)]
pub enum DesktopError {
    #[error("transport error: {0}")]
    Transport(String),
    #[error("notification error: {0}")]
    Notification(String),
    #[error("tray error: {0}")]
    Tray(String),
    #[error("no active surface")]
    NoSurface,
}

/// Surface state plus the last presentation the daemon should render.
#[derive(Debug, Clone, Default)]
pub struct ViewModel {
    pub snapshot: SurfaceSnapshot,
    /// Surface-scoped presentation from a turn result (e.g. a compact panel
    /// stream or voice readback). This is not a planner event.
    pub surface_presentation: Option<Presentation>,
    /// Current desktop presentation built from the last planner event or
    /// current-task card. Carries server-issued quick-action capabilities.
    pub current_presentation: Option<DesktopPresentation>,
    pub theme: crate::config::Theme,
    /// Whether desktop notifications should be suppressed.
    pub do_not_disturb: bool,
}

impl ViewModel {
    pub fn state(&self) -> SurfaceState {
        self.snapshot.state
    }

    /// Human-readable title for the tray tooltip / notification header.
    pub fn title(&self) -> String {
        if let Some(p) = &self.current_presentation {
            return current_title(p);
        }
        match &self.surface_presentation {
            Some(Presentation::CurrentTask(card)) => {
                if let Some(settlement) = &card.settlement {
                    return format!("未確定: {}", settlement.question);
                }
                format!("今: {}", card.title)
            }
            Some(Presentation::CheckIn(card)) => card.question.clone(),
            Some(Presentation::ScheduleAlert(alert)) => alert.message.clone(),
            _ => format!("takusu — {}", state_label(self.snapshot.state)),
        }
    }

    /// Detail text for the compact panel / recovery UI.
    pub fn detail(&self) -> Option<String> {
        if let Some(p) = &self.current_presentation {
            return Some(p.body.clone());
        }
        self.snapshot.error.clone()
    }
}

fn current_title(presentation: &DesktopPresentation) -> String {
    match &presentation.presentation {
        Presentation::CurrentTask(card) => {
            if let Some(settlement) = &card.settlement {
                return format!("未確定: {}", settlement.question);
            }
            format!("今: {}", card.title)
        }
        Presentation::CheckIn(card) => card.question.clone(),
        Presentation::ScheduleAlert(alert) => alert.message.clone(),
        _ => presentation.title.clone(),
    }
}

fn state_label(state: SurfaceState) -> &'static str {
    match state {
        SurfaceState::Idle => "待機中",
        SurfaceState::Listening => "聞いています",
        SurfaceState::Transcribing => "書き起こし中",
        SurfaceState::Thinking => "考え中",
        SurfaceState::WaitingForUser => "確認待ち",
        SurfaceState::WaitingForApproval => "承認待ち",
        SurfaceState::Speaking => "話しています",
        SurfaceState::Error => "エラー",
    }
}

/// Thread-safe shared state.
#[derive(Clone)]
pub struct DesktopState {
    inner: Arc<RwLock<ViewModel>>,
    #[allow(dead_code)]
    config: Config,
    voice_invite: Arc<AtomicBool>,
    ambient_active: Arc<AtomicBool>,
    /// Wake word currently used by the ambient session, shown in notifications.
    ambient_wake_word: Arc<Mutex<String>>,
    on_change: OnChange,
    #[cfg(feature = "audio-device")]
    voice: Arc<Mutex<Option<VoiceSessionHandle>>>,
    #[cfg(feature = "audio-device")]
    ambient: Arc<Mutex<Option<AmbientSessionHandle>>>,
    #[cfg(feature = "audio-device")]
    ambient_starting: Arc<AtomicBool>,
    #[cfg(feature = "audio-device")]
    ambient_stop_requested: Arc<AtomicBool>,
}

impl DesktopState {
    pub fn new(config: Config) -> Self {
        Self {
            inner: Arc::new(RwLock::new(ViewModel {
                theme: config.theme,
                ..Default::default()
            })),
            config,
            voice_invite: Arc::new(AtomicBool::new(false)),
            ambient_active: Arc::new(AtomicBool::new(false)),
            ambient_wake_word: Arc::new(Mutex::new("たくす".into())),
            on_change: Arc::new(Mutex::new(None)),
            #[cfg(feature = "audio-device")]
            voice: Arc::new(Mutex::new(None)),
            #[cfg(feature = "audio-device")]
            ambient: Arc::new(Mutex::new(None)),
            #[cfg(feature = "audio-device")]
            ambient_starting: Arc::new(AtomicBool::new(false)),
            #[cfg(feature = "audio-device")]
            ambient_stop_requested: Arc::new(AtomicBool::new(false)),
        }
    }

    pub fn set(&self, model: ViewModel) {
        if let Ok(mut guard) = self.inner.write() {
            *guard = model;
        }
        self.notify_change();
    }

    pub fn snapshot(&self) -> Option<ViewModel> {
        self.inner.read().ok().map(|g| g.clone())
    }

    pub fn update_surface(&self, event: SurfaceEvent) {
        if let Ok(mut guard) = self.inner.write() {
            match event {
                SurfaceEvent::Snapshot(s) | SurfaceEvent::StateChanged(s) => guard.snapshot = s,
            }
        }
        self.notify_change();
    }

    pub fn set_surface_presentation(&self, presentation: Option<Presentation>) {
        if let Ok(mut guard) = self.inner.write() {
            guard.surface_presentation = presentation;
        }
        self.notify_change();
    }

    pub fn set_current_presentation(&self, presentation: Option<DesktopPresentation>) {
        if let Ok(mut guard) = self.inner.write() {
            guard.current_presentation = presentation;
        }
        self.notify_change();
    }

    pub fn current_presentation(&self) -> Option<DesktopPresentation> {
        self.inner
            .read()
            .ok()
            .and_then(|g| g.current_presentation.clone())
    }

    pub fn set_theme(&self, theme: crate::config::Theme) {
        if let Ok(mut guard) = self.inner.write() {
            guard.theme = theme;
        }
        self.notify_change();
    }

    /// Return the currently configured theme, falling back to the default.
    pub fn theme(&self) -> crate::config::Theme {
        self.snapshot().map(|view| view.theme).unwrap_or_default()
    }

    pub fn set_do_not_disturb(&self, enabled: bool) {
        if let Ok(mut guard) = self.inner.write() {
            guard.do_not_disturb = enabled;
        }
        self.notify_change();
    }

    pub fn do_not_disturb(&self) -> bool {
        self.inner
            .read()
            .ok()
            .map(|g| g.do_not_disturb)
            .unwrap_or(false)
    }

    pub fn current_task_id(&self) -> Option<String> {
        self.inner
            .read()
            .ok()
            .and_then(|g| g.current_presentation.as_ref()?.task_id.clone())
    }

    /// Extract quick actions from the current desktop presentation.
    ///
    /// Current-task cards expose start/pause/complete/dismiss. Check-ins expose
    /// their act/shift actions. Presentations without quick actions return an
    /// empty list.
    pub fn quick_actions(&self) -> Vec<DesktopAction> {
        let guard = match self.inner.read() {
            Ok(g) => g,
            Err(e) => e.into_inner(),
        };
        guard
            .current_presentation
            .as_ref()
            .map(|p| p.actions.clone())
            .unwrap_or_default()
    }

    /// Set a callback to run whenever the surface state or presentation
    /// changes. Used by `main.rs` to keep the compact panel in sync.
    pub fn set_on_change<F>(&self, f: F)
    where
        F: Fn() + Send + Sync + 'static,
    {
        if let Ok(mut guard) = self.on_change.lock() {
            *guard = Some(Arc::new(f));
        }
    }

    fn notify_change(&self) {
        if let Ok(guard) = self.on_change.lock()
            && let Some(cb) = guard.as_ref()
        {
            cb();
        }
    }

    /// Invite the compact panel to show a voice session start button.
    pub fn set_voice_invite(&self, enabled: bool) {
        self.voice_invite.store(enabled, Ordering::Relaxed);
        self.notify_change();
    }

    /// Read and reset the voice session invitation flag.
    pub fn consume_voice_invite(&self) -> bool {
        self.voice_invite.swap(false, Ordering::Relaxed)
    }

    /// Whether a voice session invitation is pending.
    pub fn voice_invite(&self) -> bool {
        self.voice_invite.load(Ordering::Relaxed)
    }

    /// Whether a voice session is currently running.
    #[cfg(feature = "audio-device")]
    pub fn voice_session_active(&self) -> bool {
        self.voice.lock().ok().is_some_and(|g| g.is_some())
    }

    /// Always false when the `audio-device` feature is disabled.
    #[cfg(not(feature = "audio-device"))]
    pub fn voice_session_active(&self) -> bool {
        false
    }

    #[cfg(feature = "audio-device")]
    pub(crate) fn set_voice_handle(&self, handle: Option<VoiceSessionHandle>) {
        if let Ok(mut guard) = self.voice.lock() {
            *guard = handle;
        }
        self.notify_change();
    }

    /// Start a voice session from the desktop surface.
    #[cfg(feature = "audio-device")]
    pub fn start_voice_session(&self) {
        if self.voice_session_active() {
            return;
        }
        let agent_config = match AgentConfig::load() {
            Ok(mut cfg) => {
                cfg.server.url = self.config.local_url.clone();
                cfg.server.token = self.config.token.clone();
                cfg
            }
            Err(error) => {
                tracing::error!(error=%error, "failed to load agent config");
                let machine = takusu_agent::SurfaceStateMachine::new();
                let snapshot =
                    machine.apply_turn_event(&TurnEvent::Error(format!("agent config: {error}")));
                self.update_surface(SurfaceEvent::StateChanged(snapshot));
                return;
            }
        };
        if let Err(error) = spawn_voice_session(self.clone(), self.config.clone(), &agent_config) {
            tracing::error!(error=%error, "failed to start voice session");
            let machine = takusu_agent::SurfaceStateMachine::new();
            let snapshot = machine.apply_turn_event(&TurnEvent::Error(error.to_string()));
            self.update_surface(SurfaceEvent::StateChanged(snapshot));
        }
    }

    /// No-op placeholder when the `audio-device` feature is disabled.
    #[cfg(not(feature = "audio-device"))]
    pub fn start_voice_session(&self) {
        tracing::warn!("audio-device feature is disabled; voice sessions are unavailable");
    }

    /// Stop a running voice session.
    #[cfg(feature = "audio-device")]
    pub fn stop_voice_session(&self) {
        if let Ok(mut guard) = self.voice.lock()
            && let Some(handle) = guard.take()
        {
            handle.stop();
        }
    }

    /// No-op placeholder when the `audio-device` feature is disabled.
    #[cfg(not(feature = "audio-device"))]
    pub fn stop_voice_session(&self) {}

    /// Whether ambient listening is currently active.
    pub fn ambient_active(&self) -> bool {
        self.ambient_active.load(Ordering::Relaxed)
    }

    /// Set the ambient active flag and notify listeners.
    pub fn set_ambient_active(&self, active: bool) {
        self.ambient_active.store(active, Ordering::Relaxed);
        self.notify_change();
    }

    /// Wake word for the current or next ambient session.
    pub fn ambient_wake_word(&self) -> String {
        self.ambient_wake_word
            .lock()
            .map(|g| g.clone())
            .unwrap_or_else(|_| "たくす".into())
    }

    #[cfg(feature = "audio-device")]
    pub(crate) fn set_ambient_wake_word(&self, word: impl Into<String>) {
        if let Ok(mut guard) = self.ambient_wake_word.lock() {
            *guard = word.into();
        }
    }

    /// Whether an ambient session is currently running.
    #[cfg(feature = "audio-device")]
    pub fn ambient_session_active(&self) -> bool {
        self.ambient.lock().ok().is_some_and(|g| g.is_some())
    }

    /// Always false when the `audio-device` feature is disabled.
    #[cfg(not(feature = "audio-device"))]
    pub fn ambient_session_active(&self) -> bool {
        false
    }

    #[cfg(feature = "audio-device")]
    pub(crate) fn set_ambient_handle(&self, handle: Option<AmbientSessionHandle>) {
        if let Ok(mut guard) = self.ambient.lock() {
            *guard = handle;
        }
        self.notify_change();
    }

    /// Whether an ambient session is in the process of starting.
    #[cfg(feature = "audio-device")]
    pub(crate) fn ambient_starting(&self) -> bool {
        self.ambient_starting.load(Ordering::Relaxed)
    }

    #[cfg(feature = "audio-device")]
    pub(crate) fn set_ambient_starting(&self, starting: bool) {
        self.ambient_starting.store(starting, Ordering::Relaxed);
    }

    /// Whether the user requested a stop while ambient was still starting.
    #[cfg(feature = "audio-device")]
    pub(crate) fn ambient_stop_requested(&self) -> bool {
        self.ambient_stop_requested.load(Ordering::Relaxed)
    }

    #[cfg(feature = "audio-device")]
    pub(crate) fn set_ambient_stop_requested(&self, requested: bool) {
        self.ambient_stop_requested
            .store(requested, Ordering::Relaxed);
    }

    /// Start an ambient listening session from the desktop surface.
    #[cfg(feature = "audio-device")]
    pub fn start_ambient_session(&self) {
        if self.ambient_session_active() || self.ambient_starting() {
            return;
        }
        self.set_ambient_starting(true);
        self.set_ambient_stop_requested(false);

        let agent_config = match AgentConfig::load() {
            Ok(mut cfg) => {
                cfg.server.url = self.config.local_url.clone();
                cfg.server.token = self.config.token.clone();
                cfg
            }
            Err(error) => {
                tracing::error!(error=%error, "failed to load agent config");
                let machine = takusu_agent::SurfaceStateMachine::new();
                let snapshot =
                    machine.apply_turn_event(&TurnEvent::Error(format!("agent config: {error}")));
                self.update_surface(SurfaceEvent::StateChanged(snapshot));
                self.set_ambient_starting(false);
                return;
            }
        };
        if let Err(error) = spawn_ambient_session(self.clone(), self.config.clone(), &agent_config)
        {
            tracing::error!(error=%error, "failed to start ambient session");
            let machine = takusu_agent::SurfaceStateMachine::new();
            let snapshot = machine.apply_turn_event(&TurnEvent::Error(error.to_string()));
            self.update_surface(SurfaceEvent::StateChanged(snapshot));
            self.set_ambient_starting(false);
            self.set_ambient_stop_requested(false);
        }
    }

    /// No-op placeholder when the `audio-device` feature is disabled.
    #[cfg(not(feature = "audio-device"))]
    pub fn start_ambient_session(&self) {
        tracing::warn!("audio-device feature is disabled; ambient listening is unavailable");
    }

    /// Stop the ambient listening session.
    #[cfg(feature = "audio-device")]
    pub fn stop_ambient_session(&self) {
        if self.ambient_starting() {
            self.set_ambient_stop_requested(true);
        }
        if let Ok(mut guard) = self.ambient.lock()
            && let Some(handle) = guard.take()
        {
            handle.stop();
        }
        self.set_ambient_active(false);
    }

    /// No-op placeholder when the `audio-device` feature is disabled.
    #[cfg(not(feature = "audio-device"))]
    pub fn stop_ambient_session(&self) {
        self.set_ambient_active(false);
    }
}
