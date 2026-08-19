//! Compact popover window.
//!
//! WI-7 specifies a real window rendered with GPUI. This module exposes a
//! stable interface so the tray and notification code can request it; the
//! current implementation falls back to the StatusNotifierItem menu (see
//! `tray.rs`) because the build environment does not yet provide the GUI
//! dependencies required for winit/egui/GPUI.
//!
//! A GPUI or winit+egui implementation can be swapped in here without touching
//! the daemon event loop.

use crate::state::DesktopState;

/// A request to display the compact panel.
#[derive(Debug, Clone)]
pub struct PopoverRequest {
    pub title: String,
    pub detail: Option<String>,
}

/// Current popover backend.
#[derive(Debug, Clone, Default)]
pub enum Popover {
    /// Placeholder: the panel is shown via the tray menu fallback.
    #[default]
    MenuFallback,
}

impl Popover {
    pub fn new() -> Self {
        Self::default()
    }

    /// Show or update the compact panel.
    pub fn show(&self, _state: &DesktopState, request: PopoverRequest) {
        tracing::info!(title=%request.title, detail=?request.detail, "popover requested (menu fallback)");
    }

    /// Hide the panel.
    pub fn hide(&self) {
        tracing::info!("popover hidden (menu fallback)");
    }
}
