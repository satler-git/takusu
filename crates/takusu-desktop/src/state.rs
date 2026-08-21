//! Shared desktop state.
//!
//! The state is a small, interior-mutable snapshot of what the tray icon,
//! notification, and compact panel should display. It is updated from an SSE
//! stream fed by the local agent transport and from the planner event replay
//! loop.

use std::sync::{Arc, RwLock};

use takusu_agent::{Presentation, SurfaceEvent, SurfaceSnapshot, SurfaceState};

use crate::config::Theme;
use crate::presentation::{DesktopAction, DesktopPresentation};

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
    pub theme: Theme,
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
#[derive(Debug, Clone)]
pub struct DesktopState {
    inner: Arc<RwLock<ViewModel>>,
}

impl Default for DesktopState {
    fn default() -> Self {
        Self::new()
    }
}

impl DesktopState {
    pub fn new() -> Self {
        Self {
            inner: Arc::new(RwLock::new(ViewModel::default())),
        }
    }

    pub fn set(&self, model: ViewModel) {
        if let Ok(mut guard) = self.inner.write() {
            *guard = model;
        }
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
    }

    pub fn set_surface_presentation(&self, presentation: Option<Presentation>) {
        if let Ok(mut guard) = self.inner.write() {
            guard.surface_presentation = presentation;
        }
    }

    pub fn set_current_presentation(&self, presentation: Option<DesktopPresentation>) {
        if let Ok(mut guard) = self.inner.write() {
            guard.current_presentation = presentation;
        }
    }

    pub fn current_presentation(&self) -> Option<DesktopPresentation> {
        self.inner
            .read()
            .ok()
            .and_then(|g| g.current_presentation.clone())
    }

    pub fn set_theme(&self, theme: Theme) {
        if let Ok(mut guard) = self.inner.write() {
            guard.theme = theme;
        }
    }

    /// Return the currently configured theme, falling back to the default.
    pub fn theme(&self) -> Theme {
        self.snapshot().map(|view| view.theme).unwrap_or_default()
    }

    pub fn set_do_not_disturb(&self, enabled: bool) {
        if let Ok(mut guard) = self.inner.write() {
            guard.do_not_disturb = enabled;
        }
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
}
