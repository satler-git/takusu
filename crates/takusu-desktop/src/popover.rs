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

use std::fmt;
use std::sync::Arc;

use crate::presentation::{DesktopAction, DesktopPresentation, execute_quick_action};
use crate::state::DesktopState;
use crate::transport::DesktopTransport;
#[cfg(target_os = "linux")]
use takusu_agent::Presentation;

#[cfg(target_os = "linux")]
use gpui::prelude::*;

/// A request to display the compact panel.
#[derive(Debug, Clone, Default)]
pub struct PopoverRequest {
    pub title: String,
    pub detail: Option<String>,
    pub actions: Vec<DesktopAction>,
}

impl PopoverRequest {
    /// Build a compact panel request from a desktop presentation.
    pub fn from_presentation(presentation: &DesktopPresentation) -> Self {
        Self {
            title: presentation.title.clone(),
            detail: Some(presentation.body.clone()),
            actions: presentation.actions.clone(),
        }
    }
}

/// Current popover backend.
#[derive(Clone, Default)]
pub enum Popover {
    /// Placeholder: the panel is shown via the tray menu fallback.
    #[default]
    MenuFallback,
    /// A real compact window rendered with GPUI.
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
    /// Create a new popover. On Linux this attempts to start a GPUI-backed
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
                    tracing::warn!(error = %err, "GPUI popover unavailable; using menu fallback");
                    Self::MenuFallback
                }
            }
        }

        #[cfg(not(target_os = "linux"))]
        {
            tracing::warn!("GPUI popover is only available on Linux; using menu fallback");
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
                    tracing::warn!(error = %err, "GPUI popover unavailable; using menu fallback");
                    Self::MenuFallback
                }
            }
        }

        #[cfg(not(target_os = "linux"))]
        {
            tracing::warn!("GPUI popover is only available on Linux; using menu fallback");
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
        Err("GPUI popover is only available on Linux".to_string())
    }

    fn show(&self, _state: &DesktopState, _request: PopoverRequest) {}

    fn hide(&self) {}
}

#[cfg(target_os = "linux")]
#[derive(Clone)]
#[doc(hidden)]
pub struct WindowController {
    sender: std::sync::Arc<std::sync::Mutex<futures_channel::mpsc::UnboundedSender<WindowCommand>>>,
}

#[cfg(target_os = "linux")]
impl fmt::Debug for WindowController {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("WindowController").finish_non_exhaustive()
    }
}

#[cfg(target_os = "linux")]
#[derive(Clone, Debug)]
enum WindowCommand {
    Show {
        title: String,
        detail: Option<String>,
        theme: crate::config::Theme,
        actions: Vec<DesktopAction>,
    },
    Hide,
}

#[cfg(target_os = "linux")]
struct PopoverView {
    title: gpui::SharedString,
    detail: Option<gpui::SharedString>,
    background: gpui::Hsla,
    text_color: gpui::Hsla,
    actions: Vec<DesktopAction>,
    transport: Arc<dyn DesktopTransport + Send + Sync>,
    runtime: tokio::runtime::Handle,
}

#[cfg(target_os = "linux")]
impl gpui::Render for PopoverView {
    fn render(
        &mut self,
        _window: &mut gpui::Window,
        _cx: &mut gpui::Context<Self>,
    ) -> impl gpui::IntoElement {
        let mut action_children = Vec::new();
        for action in &self.actions {
            let runtime = self.runtime.clone();
            let transport = self.transport.clone();
            let action = action.clone();
            let label: gpui::SharedString = action.label.clone().into();
            action_children.push(
                gpui::div()
                    .child(label)
                    .px_3()
                    .py_1()
                    .rounded_md()
                    .border_1()
                    .border_color(self.text_color)
                    .cursor_pointer()
                    .id(gpui::ElementId::Name(action.id.clone().into()))
                    .on_click(move |_event, _window, _cx| {
                        let transport = transport.clone();
                        let action = action.clone();
                        runtime.spawn(async move {
                            match execute_quick_action(transport.as_ref(), &action).await {
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
                                Err(e) => {
                                    tracing::warn!(
                                        error = %e,
                                        action_id = %action.id,
                                        "popover quick action failed"
                                    );
                                }
                            }
                        });
                    }),
            );
        }

        gpui::div()
            .flex()
            .flex_col()
            .justify_between()
            .p_4()
            .gap_2()
            .bg(self.background)
            .text_color(self.text_color)
            .size_full()
            .child(
                gpui::div()
                    .flex()
                    .flex_col()
                    .gap_2()
                    .child(
                        gpui::div()
                            .child(self.title.clone())
                            .text_xl()
                            .font_weight(gpui::FontWeight::BOLD),
                    )
                    .when_some(self.detail.clone(), |this, detail| {
                        this.child(gpui::div().child(detail).text_sm())
                    }),
            )
            .child(
                gpui::div()
                    .flex()
                    .flex_row()
                    .gap_2()
                    .children(action_children)
                    .child(
                        gpui::div()
                            .child("閉じる")
                            .px_3()
                            .py_1()
                            .rounded_md()
                            .border_1()
                            .border_color(self.text_color)
                            .cursor_pointer()
                            .id("close")
                            .on_click(|_event, window, _cx| window.remove_window()),
                    ),
            )
    }
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
            mpsc::channel::<futures_channel::mpsc::UnboundedSender<WindowCommand>>();

        let thread_transport = Arc::clone(&transport);
        let thread_runtime = runtime.clone();

        thread::spawn(move || {
            let app = gpui::Application::new();
            app.run(move |cx: &mut gpui::App| {
                let (cmd_tx, mut cmd_rx) = futures_channel::mpsc::unbounded::<WindowCommand>();

                if init_tx.send(cmd_tx).is_err() {
                    return;
                }

                cx.spawn(async move |cx: &mut gpui::AsyncApp| {
                    use futures_util::StreamExt;

                    let mut current: Option<gpui::WindowHandle<PopoverView>> = None;

                    while let Some(cmd) = StreamExt::next(&mut cmd_rx).await {
                        match cmd {
                            WindowCommand::Hide => {
                                if let Some(handle) = current.take() {
                                    let _ = handle.update(cx, |_view, window, _cx| {
                                        window.remove_window();
                                    });
                                }
                            }
                            WindowCommand::Show {
                                title,
                                detail,
                                theme,
                                actions,
                            } => {
                                if let Some(handle) = current.take() {
                                    let _ = handle.update(cx, |_view, window, _cx| {
                                        window.remove_window();
                                    });
                                }

                                let title_ss: gpui::SharedString = title.clone().into();
                                let detail_ss: Option<gpui::SharedString> = detail.map(Into::into);
                                let (background, text_color) = theme_colors(theme);

                                let options = gpui::WindowOptions {
                                    window_bounds: Some(gpui::WindowBounds::Windowed(
                                        gpui::Bounds {
                                            origin: gpui::point(gpui::px(100.0), gpui::px(100.0)),
                                            size: gpui::size(gpui::px(360.0), gpui::px(220.0)),
                                        },
                                    )),
                                    titlebar: Some(gpui::TitlebarOptions {
                                        title: Some(title_ss.clone()),
                                        ..Default::default()
                                    }),
                                    focus: true,
                                    show: true,
                                    kind: gpui::WindowKind::PopUp,
                                    is_movable: false,
                                    is_resizable: false,
                                    is_minimizable: false,
                                    ..Default::default()
                                };

                                match cx.open_window(options, |_window, app_cx| {
                                    app_cx.new(|_cx| PopoverView {
                                        title: title_ss,
                                        detail: detail_ss,
                                        background,
                                        text_color,
                                        actions,
                                        transport: Arc::clone(&thread_transport),
                                        runtime: thread_runtime.clone(),
                                    })
                                }) {
                                    Ok(handle) => current = Some(handle),
                                    Err(err) => {
                                        tracing::warn!(
                                            error = %err,
                                            "failed to open GPUI popover window"
                                        );
                                    }
                                }
                            }
                        }
                    }
                })
                .detach();
            });
        });

        match init_rx.recv_timeout(Duration::from_secs(2)) {
            Ok(sender) => Ok(Self {
                sender: std::sync::Arc::new(std::sync::Mutex::new(sender)),
            }),
            Err(RecvTimeoutError::Timeout) => {
                Err("GPUI popover did not initialize within 2 seconds".to_string())
            }
            Err(RecvTimeoutError::Disconnected) => {
                Err("GPUI popover thread disconnected".to_string())
            }
        }
    }

    fn show(&self, state: &DesktopState, request: PopoverRequest) {
        let command = WindowCommand::Show {
            title: request.title,
            detail: request.detail,
            theme: state.theme(),
            actions: request.actions,
        };

        if let Ok(guard) = self.sender.lock()
            && let Err(err) = guard.unbounded_send(command)
        {
            tracing::warn!(error = %err, "failed to send popover show command");
        }
    }

    fn hide(&self) {
        if let Ok(guard) = self.sender.lock() {
            let _ = guard.unbounded_send(WindowCommand::Hide);
        }
    }
}

#[cfg(target_os = "linux")]
fn theme_colors(theme: crate::config::Theme) -> (gpui::Hsla, gpui::Hsla) {
    let (bg_hex, text_hex) = match theme {
        crate::config::Theme::Light => (0xffffff, 0x000000),
        crate::config::Theme::Dark => (0x1e1e2e, 0xcdd6f4),
        crate::config::Theme::Catppuccin => (0x303446, 0xc6d0f5),
        crate::config::Theme::AuraSoftDark => (0x1f1b29, 0xe6e0f5),
    };

    let background: gpui::Hsla = gpui::rgb(bg_hex).into();
    let text: gpui::Hsla = gpui::rgb(text_hex).into();
    (background, text)
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

        let state = DesktopState::new();
        popover.show(
            &state,
            PopoverRequest {
                title: "takusu".into(),
                detail: Some("detail text".into()),
                actions: Vec::new(),
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
