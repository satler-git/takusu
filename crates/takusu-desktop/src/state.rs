//! Shared desktop state.
//!
//! The state is a small, interior-mutable snapshot of what the tray icon,
//! notification, and popover should display. It is updated from an SSE stream
//! fed by the local agent transport.

use std::sync::{Arc, RwLock};

use takusu_agent::{Presentation, SurfaceEvent, SurfaceSnapshot, SurfaceState};
use takusu_agent::capability::ActionCapability;

use crate::config::Theme;

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
    pub presentation: Option<Presentation>,
    pub theme: Theme,
}

impl ViewModel {
    pub fn state(&self) -> SurfaceState {
        self.snapshot.state
    }

    /// Human-readable title for the tray tooltip / notification header.
    pub fn title(&self) -> String {
        match &self.presentation {
            Some(Presentation::CurrentTask(card)) => format!("今: {}", card.title),
            Some(Presentation::CheckIn(card)) => card.question.clone(),
            Some(Presentation::ScheduleAlert(alert)) => alert.message.clone(),
            _ => format!("takusu — {}", state_label(self.snapshot.state)),
        }
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

    pub fn set_presentation(&self, presentation: Option<Presentation>) {
        if let Ok(mut guard) = self.inner.write() {
            guard.presentation = presentation;
        }
    }

    pub fn set_theme(&self, theme: Theme) {
        if let Ok(mut guard) = self.inner.write() {
            guard.theme = theme;
        }
    }

    /// Extract quick actions from the current presentation, if any.
    pub fn quick_actions(&self) -> Vec<(String, Option<ActionCapability>)> {
        let guard = match self.inner.read() {
            Ok(g) => g,
            Err(e) => e.into_inner(),
        };
        let mut out = Vec::new();
        if let Some(Presentation::CurrentTask(_card)) = &guard.presentation {
            // For the mock, the current task card can expose start/delay/progress/complete.
            // Real quick-action capabilities come from the transport's `mint_capability`.
            out.push(("開始".into(), None));
            out.push(("10分ずらす".into(), None));
            out.push(("進捗".into(), None));
            out.push(("完了".into(), None));
        }
        if let Some(Presentation::CheckIn(card)) = &guard.presentation {
            for g in [&card.act, &card.shift] {
                for a in g.actions.as_slice() {
                    out.push((a.label.clone(), a.capability.clone()));
                }
            }
        }
        out
    }
}
