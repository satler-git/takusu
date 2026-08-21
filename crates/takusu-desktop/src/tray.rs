//! Linux StatusNotifierItem tray icon.
//!
//! Uses `ksni` with the tokio backend. The tray icon's name and menu are driven
//! by the shared [`DesktopState`].

#[cfg(target_os = "linux")]
use ksni::menu::{CheckmarkItem, StandardItem};
#[cfg(target_os = "linux")]
use ksni::{Icon, MenuItem, Tray, TrayMethods};
use std::sync::Arc;
use takusu_agent::SurfaceCommand;
use takusu_agent::SurfaceState;

use crate::config::Theme;
use crate::presentation::{DesktopAction, execute_quick_action};
use crate::state::DesktopState;
use crate::transport::DesktopTransport;

/// Icon name to show in the SNI host. The desktop environment's icon theme is
/// used; if missing the built-in icon mapping falls back to a simple color.
pub fn icon_name_for_state(state: SurfaceState) -> &'static str {
    match state {
        SurfaceState::Idle => "takusu-tray-idle",
        SurfaceState::Listening => "takusu-tray-listening",
        SurfaceState::Transcribing => "takusu-tray-listening",
        SurfaceState::Thinking => "takusu-tray-thinking",
        SurfaceState::WaitingForUser => "takusu-tray-thinking",
        SurfaceState::WaitingForApproval => "takusu-tray-approval",
        SurfaceState::Speaking => "takusu-tray-speaking",
        SurfaceState::Error => "takusu-tray-error",
    }
}

/// Solid ARGB icon fallback for SNI hosts that do not load icon names. The
/// 24×24 ARGB buffer is a single colored square matching the active theme.
pub fn icon_pixmap_for_state(state: SurfaceState, theme: Theme) -> Vec<u8> {
    let color = state_color(state, theme);
    // 24×24 ARGB, with A=255 and R/G/B from the theme color in network byte order.
    let (r, g, b) = hex_rgb(color);
    let mut buf = Vec::with_capacity(24 * 24 * 4);
    for _ in 0..(24 * 24) {
        buf.extend_from_slice(&[0xff, r, g, b]);
    }
    buf
}

/// Theme-appropriate color for the tray dot / fallback icon.
pub fn state_color(state: SurfaceState, theme: Theme) -> &'static str {
    let brand = match theme {
        Theme::Light => "#7261A3",
        Theme::Dark => "#9B8BC4",
        Theme::Catppuccin => "#9B8BC4",
        Theme::AuraSoftDark => "#a48bd6",
    };
    match state {
        SurfaceState::Idle => "#9B95AA",
        SurfaceState::Listening | SurfaceState::Transcribing | SurfaceState::Speaking => brand,
        SurfaceState::Thinking | SurfaceState::WaitingForUser => "#E0B040",
        SurfaceState::WaitingForApproval => "#E0B040",
        SurfaceState::Error => "#B33A3A",
    }
}

fn hex_rgb(hex: &str) -> (u8, u8, u8) {
    let hex = hex.strip_prefix('#').unwrap_or(hex);
    if hex.len() < 6 {
        return (0, 0, 0);
    }
    let r = u8::from_str_radix(&hex[0..2], 16).unwrap_or(0);
    let g = u8::from_str_radix(&hex[2..4], 16).unwrap_or(0);
    let b = u8::from_str_radix(&hex[4..6], 16).unwrap_or(0);
    (r, g, b)
}

#[cfg(target_os = "linux")]
pub struct TakusuTray {
    state: DesktopState,
    transport: Arc<dyn DesktopTransport + Send + Sync>,
}

#[cfg(target_os = "linux")]
impl TakusuTray {
    pub fn new(state: DesktopState, transport: Arc<dyn DesktopTransport + Send + Sync>) -> Self {
        Self { state, transport }
    }
}

#[cfg(target_os = "linux")]
impl Tray for TakusuTray {
    fn id(&self) -> String {
        "dev.satler.takusu.desktop".into()
    }

    fn title(&self) -> String {
        self.state
            .snapshot()
            .map(|v| v.title())
            .unwrap_or_else(|| "takusu".into())
    }

    fn icon_name(&self) -> String {
        self.state
            .snapshot()
            .map(|v| icon_name_for_state(v.state()).into())
            .unwrap_or_else(|| "takusu-tray-idle".into())
    }

    fn icon_pixmap(&self) -> Vec<Icon> {
        self.state
            .snapshot()
            .map(|v| {
                let buf = icon_pixmap_for_state(v.state(), v.theme);
                vec![Icon {
                    width: 24,
                    height: 24,
                    data: buf,
                }]
            })
            .unwrap_or_default()
    }

    #[allow(clippy::collapsible_if)]
    fn menu(&self) -> Vec<MenuItem<Self>> {
        let mut items = Vec::new();

        if let Some(view) = self.state.snapshot() {
            items.push(
                StandardItem {
                    label: view.title(),
                    enabled: false,
                    ..Default::default()
                }
                .into(),
            );

            // Quick actions from the current desktop presentation.
            for action in self.state.quick_actions() {
                let transport = Arc::clone(&self.transport);
                let label = quick_action_label(&action);
                let activate = quick_action_activate(action, transport);
                items.push(
                    StandardItem {
                        label,
                        activate,
                        ..Default::default()
                    }
                    .into(),
                );
            }

            items.push(MenuItem::Separator);
        }

        // Open the compact panel / approval UI / recovery view.
        {
            let transport = Arc::clone(&self.transport);
            let state = self.state.clone();
            let activate = Box::new(move |_this: &mut Self| {
                let state = state.clone();
                let transport = Arc::clone(&transport);
                tokio::spawn(async move {
                    let command = match state
                        .snapshot()
                        .map(|v| v.state())
                        .unwrap_or(SurfaceState::Idle)
                    {
                        SurfaceState::Listening => Some(SurfaceCommand::ConfirmRecording),
                        SurfaceState::Thinking => Some(SurfaceCommand::OpenPanel),
                        SurfaceState::Speaking => Some(SurfaceCommand::StopTts),
                        SurfaceState::WaitingForApproval => Some(SurfaceCommand::OpenApproval),
                        SurfaceState::Error => Some(SurfaceCommand::ShowRecovery),
                        _ => None,
                    };

                    if let Some(command) = command {
                        if let Err(e) = transport.send_command(command).await {
                            tracing::warn!(error=%e, command=?command, "tray open command failed");
                        }
                    }
                });
            });
            items.push(
                StandardItem {
                    label: "開く".into(),
                    activate,
                    ..Default::default()
                }
                .into(),
            );
        }

        {
            let state = self.state.clone();
            let checked = state.do_not_disturb();
            let activate = Box::new(move |_this: &mut Self| {
                let next = !state.do_not_disturb();
                state.set_do_not_disturb(next);
                tracing::info!(do_not_disturb = next, "toggled notification pause");
            });
            items.push(
                CheckmarkItem {
                    label: "通知を一時停止".into(),
                    checked,
                    activate,
                    ..Default::default()
                }
                .into(),
            );
        }

        items.push(MenuItem::Separator);

        items.push(
            StandardItem {
                label: "終了".into(),
                activate: Box::new(|_this| {
                    tracing::info!("tray quit requested");
                    std::process::exit(0);
                }),
                ..Default::default()
            }
            .into(),
        );

        items
    }
}

fn quick_action_label(action: &DesktopAction) -> String {
    if action.label.is_empty() {
        return "アクション".into();
    }
    action.label.clone()
}

#[cfg(target_os = "linux")]
fn quick_action_activate(
    action: DesktopAction,
    transport: Arc<dyn DesktopTransport + Send + Sync>,
) -> Box<dyn Fn(&mut TakusuTray) + Send> {
    Box::new(move |_this: &mut TakusuTray| {
        let transport = Arc::clone(&transport);
        let action = action.clone();
        tokio::spawn(async move {
            if let Err(e) = execute_quick_action(transport.as_ref(), &action).await {
                tracing::warn!(
                    error = %e,
                    action_id = %action.id,
                    label = %action.label,
                    "failed to execute quick action"
                );
            }
        });
    })
}

/// Opaque handle to the tray service.
#[cfg(target_os = "linux")]
pub type TrayHandle = ksni::Handle<TakusuTray>;
#[cfg(not(target_os = "linux"))]
pub type TrayHandle = ();

/// Spawn the SNI tray service. Returns a handle that keeps the service alive.
#[cfg(target_os = "linux")]
pub async fn spawn(
    state: DesktopState,
    transport: Arc<dyn DesktopTransport + Send + Sync>,
) -> Result<TrayHandle, crate::state::DesktopError> {
    let tray = TakusuTray::new(state, transport);
    let handle = tray
        .spawn()
        .await
        .map_err(|e| crate::state::DesktopError::Tray(e.to_string()))?;
    Ok(handle)
}

#[cfg(not(target_os = "linux"))]
pub async fn spawn(
    _state: DesktopState,
    _transport: Arc<dyn DesktopTransport + Send + Sync>,
) -> Result<TrayHandle, crate::state::DesktopError> {
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn icon_name_maps_each_surface_state() {
        assert_eq!(icon_name_for_state(SurfaceState::Idle), "takusu-tray-idle");
        assert_eq!(
            icon_name_for_state(SurfaceState::Listening),
            "takusu-tray-listening"
        );
        assert_eq!(
            icon_name_for_state(SurfaceState::Transcribing),
            "takusu-tray-listening"
        );
        assert_eq!(
            icon_name_for_state(SurfaceState::Thinking),
            "takusu-tray-thinking"
        );
        assert_eq!(
            icon_name_for_state(SurfaceState::WaitingForUser),
            "takusu-tray-thinking"
        );
        assert_eq!(
            icon_name_for_state(SurfaceState::WaitingForApproval),
            "takusu-tray-approval"
        );
        assert_eq!(
            icon_name_for_state(SurfaceState::Speaking),
            "takusu-tray-speaking"
        );
        assert_eq!(
            icon_name_for_state(SurfaceState::Error),
            "takusu-tray-error"
        );
    }

    #[test]
    fn state_color_uses_brand_for_active_states_and_warning_for_thinking() {
        // Listening uses the theme brand.
        assert_eq!(
            state_color(SurfaceState::Listening, Theme::Light),
            "#7261A3"
        );
        assert_eq!(
            state_color(SurfaceState::Listening, Theme::AuraSoftDark),
            "#a48bd6"
        );

        // Thinking/approval use the warning color.
        assert_eq!(state_color(SurfaceState::Thinking, Theme::Dark), "#E0B040");

        // Idle is muted.
        assert_eq!(
            state_color(SurfaceState::Idle, Theme::Catppuccin),
            "#9B95AA"
        );

        // Error is red.
        assert_eq!(state_color(SurfaceState::Error, Theme::Light), "#B33A3A");
    }

    #[test]
    fn icon_pixmap_is_24x24_argb() {
        let buf = icon_pixmap_for_state(SurfaceState::Listening, Theme::Light);
        assert_eq!(buf.len(), 24 * 24 * 4);
        // ARGB network byte order: alpha first, then RGB.
        assert_eq!(buf[0], 0xff);
        // Non-empty brand color (light theme brand = #7261A3).
        assert!(buf.iter().any(|&b| b != 0 && b != 0xff));
    }
}
