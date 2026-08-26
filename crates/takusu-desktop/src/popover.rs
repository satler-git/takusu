//! Compact popover window.
//!
//! WI-7 specifies a real window rendered with an immediate-mode UI. This
//! module exposes a stable interface so the tray and notification code can
//! request it. The current implementation uses `eframe` (winit + egui + wgpu)
//! and falls back to the StatusNotifierItem menu (see `tray.rs`) when the
//! window cannot be initialized.
//!
//! A real window or a menu fallback can be selected at runtime by setting
//! `TAKUSU_DESKTOP_POPOVER=menu`.

use std::fmt;
use std::sync::Arc;

use crate::presentation::{DesktopAction, DesktopPresentation, execute_quick_action};
use crate::state::DesktopState;
use crate::transport::DesktopTransport;
#[cfg(target_os = "linux")]
use takusu_agent::Presentation;

/// A request to display the compact panel.
#[derive(Debug, Clone, Default)]
pub struct PopoverRequest {
    pub title: String,
    pub detail: Option<String>,
    pub actions: Vec<DesktopAction>,
    /// Whether to include a start/stop voice session button.
    pub voice_button: bool,
}

impl PopoverRequest {
    /// Build a compact panel request from a desktop presentation.
    pub fn from_presentation(presentation: &DesktopPresentation) -> Self {
        Self {
            title: presentation.title.clone(),
            detail: Some(presentation.body.clone()),
            actions: presentation.actions.clone(),
            voice_button: false,
        }
    }
}

/// Current popover backend.
#[derive(Clone, Default)]
pub enum Popover {
    /// Placeholder: the panel is shown via the tray menu fallback.
    #[default]
    MenuFallback,
    /// A real compact window rendered with eframe.
    Window(WindowController),
}

impl fmt::Debug for Popover {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::MenuFallback => f.write_str("Popover::MenuFallback"),
            Self::Window(_) => f.write_str("Popover::Window"),
        }
    }
}

impl Popover {
    /// Create a new popover. On Linux this attempts to start an eframe-backed
    /// window unless `TAKUSU_DESKTOP_POPOVER=menu` is set.
    pub fn new() -> Self {
        if std::env::var("TAKUSU_DESKTOP_POPOVER").as_deref() == Ok("menu") {
            return Self::MenuFallback;
        }

        #[cfg(target_os = "linux")]
        {
            let transport = Arc::new(crate::transport::MockTransport::new(
                takusu_agent::surface::SurfaceStateMachine::new().snapshot(),
            )) as Arc<dyn DesktopTransport + Send + Sync>;
            let runtime = tokio::runtime::Handle::current();
            match WindowController::new(transport, runtime) {
                Ok(ctrl) => Self::Window(ctrl),
                Err(err) => {
                    tracing::warn!(error = %err, "eframe popover unavailable; using menu fallback");
                    Self::MenuFallback
                }
            }
        }

        #[cfg(not(target_os = "linux"))]
        {
            tracing::warn!("eframe popover is only available on Linux; using menu fallback");
            Self::MenuFallback
        }
    }

    /// Create a popover wired to a transport so quick actions can be authorized.
    pub fn new_with_transport(transport: Arc<dyn DesktopTransport + Send + Sync>) -> Self {
        if std::env::var("TAKUSU_DESKTOP_POPOVER").as_deref() == Ok("menu") {
            return Self::MenuFallback;
        }

        #[cfg(target_os = "linux")]
        {
            let runtime = tokio::runtime::Handle::current();
            match WindowController::new(transport, runtime) {
                Ok(ctrl) => Self::Window(ctrl),
                Err(err) => {
                    tracing::warn!(error = %err, "eframe popover unavailable; using menu fallback");
                    Self::MenuFallback
                }
            }
        }

        #[cfg(not(target_os = "linux"))]
        {
            tracing::warn!("eframe popover is only available on Linux; using menu fallback");
            Self::MenuFallback
        }
    }

    /// Show or update the compact panel.
    pub fn show(&self, state: &DesktopState, request: PopoverRequest) {
        match self {
            Self::MenuFallback => {
                tracing::info!(
                    title = %request.title,
                    detail = ?request.detail,
                    actions = request.actions.len(),
                    "popover requested (menu fallback)"
                );
            }
            Self::Window(ctrl) => ctrl.show(state, request),
        }
    }

    /// Hide the panel.
    pub fn hide(&self) {
        match self {
            Self::MenuFallback => {
                tracing::info!("popover hidden (menu fallback)");
            }
            Self::Window(ctrl) => ctrl.hide(),
        }
    }
}

#[cfg(not(target_os = "linux"))]
#[derive(Clone)]
#[doc(hidden)]
pub struct WindowController;

#[cfg(not(target_os = "linux"))]
impl fmt::Debug for WindowController {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("WindowController").finish_non_exhaustive()
    }
}

#[cfg(not(target_os = "linux"))]
impl WindowController {
    #[allow(clippy::unnecessary_wraps)]
    fn new(_transport: Arc<dyn DesktopTransport + Send + Sync>) -> Result<Self, String> {
        Err("eframe popover is only available on Linux".to_string())
    }

    fn show(&self, _state: &DesktopState, _request: PopoverRequest) {}

    fn hide(&self) {}
}

#[cfg(target_os = "linux")]
#[derive(Clone)]
pub struct WindowController {
    sender: Arc<std::sync::Mutex<std::sync::mpsc::Sender<WindowCommand>>>,
    ctx: eframe::egui::Context,
}

#[cfg(target_os = "linux")]
impl fmt::Debug for WindowController {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("WindowController").finish_non_exhaustive()
    }
}

#[cfg(target_os = "linux")]
enum WindowCommand {
    Show {
        state: Box<DesktopState>,
        request: PopoverRequest,
    },
    Hide,
}

#[cfg(target_os = "linux")]
impl WindowController {
    fn new(
        transport: Arc<dyn DesktopTransport + Send + Sync>,
        runtime: tokio::runtime::Handle,
    ) -> Result<Self, String> {
        use std::sync::mpsc::{self, RecvTimeoutError};
        use std::thread;
        use std::time::Duration;

        let (init_tx, init_rx) =
            mpsc::channel::<Result<(mpsc::Sender<WindowCommand>, eframe::egui::Context), String>>();

        let thread_transport = Arc::clone(&transport);
        let thread_runtime = runtime;

        thread::spawn(move || {
            let viewport = eframe::egui::ViewportBuilder::default()
                .with_app_id("takusu.popup".to_string())
                .with_title("takusu")
                .with_inner_size(eframe::egui::vec2(360.0, 220.0))
                .with_min_inner_size(eframe::egui::vec2(360.0, 220.0))
                .with_max_inner_size(eframe::egui::vec2(360.0, 220.0))
                .with_resizable(false)
                .with_decorations(false)
                .with_visible(false)
                .with_active(false)
                .with_position(eframe::egui::Pos2::new(100.0, 100.0))
                .with_window_type(eframe::egui::viewport::X11WindowType::Notification)
                .with_override_redirect(false);

            let native_options = eframe::NativeOptions {
                viewport,
                renderer: eframe::Renderer::Wgpu,
                run_and_return: true,
                event_loop_builder: Some(Box::new(|el| {
                    use winit::platform::x11::EventLoopBuilderExtX11;
                    // Force the X11 backend so egui can set
                    // _NET_WM_WINDOW_TYPE_NOTIFICATION and avoid Wayland-specific
                    // surface/swapchain timing bugs without mutating process env.
                    let _ = el.with_x11();
                    let _ = el.with_any_thread(true);
                })),
                ..Default::default()
            };

            let result = eframe::run_native(
                "takusu popup",
                native_options,
                Box::new(|cc| {
                    let installed = egui_system_fonts::set_with_presets(
                        &cc.egui_ctx,
                        [
                            egui_system_fonts::FontPreset::Japanese,
                            egui_system_fonts::FontPreset::Latin,
                        ],
                        egui_system_fonts::FontStyle::Sans,
                    );
                    if installed.is_empty() {
                        tracing::warn!(
                            "no system fonts matched; Japanese text may show as fallback glyphs"
                        );
                    }

                    let (cmd_tx, cmd_rx) = mpsc::channel::<WindowCommand>();
                    let ctx = cc.egui_ctx.clone();
                    if init_tx.send(Ok((cmd_tx, ctx))).is_err() {
                        let err: Box<dyn std::error::Error + Send + Sync + 'static> =
                            Box::new(std::io::Error::other("popover init channel closed"));
                        return Err(err);
                    }

                    Ok(Box::new(App::new(
                        cmd_rx,
                        Arc::clone(&thread_transport),
                        thread_runtime.clone(),
                    )) as Box<dyn eframe::App>)
                }),
            );

            if let Err(err) = result {
                let _ = init_tx.send(Err(format!("{err}")));
                tracing::warn!(error = %err, "eframe event loop exited with an error");
            }
        });

        match init_rx.recv_timeout(Duration::from_secs(30)) {
            Ok(Ok((sender, ctx))) => Ok(Self {
                sender: Arc::new(std::sync::Mutex::new(sender)),
                ctx,
            }),
            Ok(Err(err)) => Err(err),
            Err(RecvTimeoutError::Timeout) => {
                Err("eframe popover did not initialize within 30 seconds".to_string())
            }
            Err(RecvTimeoutError::Disconnected) => {
                Err("eframe popover thread disconnected".to_string())
            }
        }
    }

    fn show(&self, state: &DesktopState, request: PopoverRequest) {
        let command = WindowCommand::Show {
            state: Box::new(state.clone()),
            request,
        };

        if let Ok(guard) = self.sender.lock()
            && let Err(err) = guard.send(command)
        {
            tracing::warn!(error = %err, "failed to send popover show command");
        } else {
            self.ctx.request_repaint();
        }
    }

    fn hide(&self) {
        if let Ok(guard) = self.sender.lock()
            && let Err(err) = guard.send(WindowCommand::Hide)
        {
            tracing::warn!(error = %err, "failed to send popover hide command");
        } else {
            self.ctx.request_repaint();
        }
    }
}

#[cfg(target_os = "linux")]
struct App {
    cmd_rx: std::sync::mpsc::Receiver<WindowCommand>,
    state: Option<Box<DesktopState>>,
    request: Option<PopoverRequest>,
    transport: Arc<dyn DesktopTransport + Send + Sync>,
    runtime: tokio::runtime::Handle,
    visible: bool,
    position_dirty: bool,
}

#[cfg(target_os = "linux")]
impl App {
    fn new(
        cmd_rx: std::sync::mpsc::Receiver<WindowCommand>,
        transport: Arc<dyn DesktopTransport + Send + Sync>,
        runtime: tokio::runtime::Handle,
    ) -> Self {
        Self {
            cmd_rx,
            state: None,
            request: None,
            transport,
            runtime,
            visible: false,
            position_dirty: false,
        }
    }
}

#[cfg(target_os = "linux")]
impl eframe::App for App {
    fn logic(&mut self, _ctx: &eframe::egui::Context, frame: &mut eframe::Frame) {
        while let Ok(cmd) = self.cmd_rx.try_recv() {
            match cmd {
                WindowCommand::Show { state, request } => {
                    self.state = Some(state);
                    self.request = Some(request);
                    self.visible = true;
                    self.position_dirty = true;
                }
                WindowCommand::Hide => {
                    self.visible = false;
                }
            }
        }

        if let Some(window) = frame.winit_window() {
            window.set_visible(self.visible);
            if self.visible && self.position_dirty {
                // Apply the default placement once per show instead of every
                // frame, so the user or compositor can move the panel and it
                // stays where it was put.
                window.set_outer_position(winit::dpi::PhysicalPosition::new(100, 100));
                self.position_dirty = false;
            }
        }
    }

    fn ui(&mut self, ui: &mut eframe::egui::Ui, _frame: &mut eframe::Frame) {
        if !self.visible {
            return;
        }

        let Some(request) = self.request.as_ref() else {
            return;
        };

        let theme = self
            .state
            .as_ref()
            .map_or(crate::config::Theme::Dark, |s| s.theme());
        let (background, text_color) = theme_colors(theme);

        eframe::egui::CentralPanel::default()
            .frame(eframe::egui::Frame::new().fill(background))
            .show(ui, |ui| {
                ui.visuals_mut().override_text_color = Some(text_color);

                ui.with_layout(
                    eframe::egui::Layout::bottom_up(eframe::egui::Align::Min),
                    |ui| {
                        ui.horizontal(|ui| {
                            if request.voice_button
                                && let Some(state) = &self.state
                            {
                                let voice_label = if state.voice_session_active() {
                                    "Stop voice session"
                                } else {
                                    "Start voice session"
                                };
                                if ui.button(voice_label).clicked() {
                                    let state = state.clone();
                                    let runtime = self.runtime.clone();
                                    drop(runtime.spawn(async move {
                                        if state.voice_session_active() {
                                            state.stop_voice_session();
                                        } else {
                                            state.set_voice_invite(true);
                                            state.start_voice_session();
                                        }
                                    }));
                                }
                            }

                            if ui.button("閉じる").clicked() {
                                self.visible = false;
                                if let Some(state) = &self.state {
                                    state.set_panel_open(false);
                                }
                            }
                        });

                        ui.horizontal_wrapped(|ui| {
                            for action in &request.actions {
                                let action = action.clone();
                                if ui.button(&action.label).clicked() {
                                    let state = self.state.clone();
                                    let transport = Arc::clone(&self.transport);
                                    let runtime = self.runtime.clone();
                                    drop(runtime.spawn(async move {
                                        match execute_quick_action(
                                            transport.as_ref(),
                                            state.as_deref(),
                                            &action,
                                        )
                                        .await
                                        {
                                            Ok(Presentation::Text { text }) => {
                                                tracing::info!(
                                                    text = %text,
                                                    action_id = %action.id,
                                                    "popover quick action returned"
                                                );
                                            }
                                            Ok(_) => {
                                                tracing::info!(
                                                    action_id = %action.id,
                                                    "popover quick action returned"
                                                );
                                            }
                                            Err(error) => {
                                                tracing::warn!(
                                                    error = %error,
                                                    action_id = %action.id,
                                                    "popover quick action failed"
                                                );
                                            }
                                        }
                                    }));
                                }
                            }
                        });

                        if let Some(detail) = &request.detail {
                            ui.label(detail);
                        }

                        ui.heading(&request.title);
                    },
                );
            });
    }
}

#[cfg(target_os = "linux")]
fn theme_colors(theme: crate::config::Theme) -> (eframe::egui::Color32, eframe::egui::Color32) {
    let (bg_hex, text_hex) = match theme {
        crate::config::Theme::Light => (0xffffff, 0x000000),
        crate::config::Theme::Dark => (0x1e1e2e, 0xcdd6f4),
        crate::config::Theme::Catppuccin => (0x303446, 0xc6d0f5),
        crate::config::Theme::AuraSoftDark => (0x1f1b29, 0xe6e0f5),
    };

    let to_color = |hex: u32| {
        eframe::egui::Color32::from_rgb(
            ((hex >> 16) & 0xff) as u8,
            ((hex >> 8) & 0xff) as u8,
            (hex & 0xff) as u8,
        )
    };

    (to_color(bg_hex), to_color(text_hex))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn menu_fallback_show_and_hide_do_not_panic() {
        unsafe {
            std::env::set_var("TAKUSU_DESKTOP_POPOVER", "menu");
        }

        let popover = Popover::new();
        assert!(matches!(popover, Popover::MenuFallback));

        let state = DesktopState::new(crate::config::Config::default());
        popover.show(
            &state,
            PopoverRequest {
                title: "takusu".into(),
                detail: Some("detail text".into()),
                actions: Vec::new(),
                voice_button: false,
            },
        );
        popover.hide();
    }

    #[test]
    fn popover_request_from_presentation_copies_actions() {
        let presentation = DesktopPresentation {
            title: "test".into(),
            body: "body".into(),
            actions: vec![DesktopAction {
                id: "a".into(),
                label: "start".into(),
                kind: takusu_agent::presentation::ActionKind::Immediate,
                capability: None,
                task_id: Some("t-1".into()),
                action: Some("start".into()),
                snooze_minutes: None,
                event_id: None,
            }],
            presentation: takusu_agent::Presentation::Text {
                text: "body".into(),
            },
            task_id: Some("t-1".into()),
            event_id: None,
        };
        let request = PopoverRequest::from_presentation(&presentation);
        assert_eq!(request.title, "test");
        assert_eq!(request.detail.as_deref(), Some("body"));
        assert_eq!(request.actions.len(), 1);
    }
}
