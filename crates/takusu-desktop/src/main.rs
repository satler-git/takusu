//! takusu resident desktop daemon.
//!
//! Runs the StatusNotifierItem tray, desktop notifications, and compact panel
//! for the Linux resident surface. It does not evaluate planner events; it
//! subscribes to the local agent's surface state stream.

use std::sync::Arc;
use std::time::Duration;

use futures_util::StreamExt;
use takusu_agent::{Presentation, SurfaceEvent, SurfaceSnapshot};
use tracing_subscriber::{EnvFilter, fmt};

use takusu_desktop::config::Config;
use takusu_desktop::notify::{self, NotificationState};
use takusu_desktop::popover::{Popover, PopoverRequest};
use takusu_desktop::state::{DesktopError, DesktopState};
use takusu_desktop::tray;
use takusu_desktop::transport::{DesktopTransport, HttpTransport, MockTransport};

#[tokio::main]
async fn main() {
    let filter = EnvFilter::from_default_env()
        .add_directive("takusu_desktop=info".parse().expect("valid directive"));
    fmt().with_env_filter(filter).init();

    if let Err(e) = run().await {
        tracing::error!(error=%e, "daemon failed");
        std::process::exit(1);
    }
}

async fn run() -> Result<(), DesktopError> {
    let config = Config::load().map_err(|e| DesktopError::Transport(e.to_string()))?;
    if config.token.is_empty() {
        tracing::warn!("no bearer token configured; agent routes may fail");
    }
    let state = DesktopState::new();
    state.set_theme(config.theme);

    // Real transport: HTTP to takusu-local agent routes.
    let transport: Arc<dyn DesktopTransport + Send + Sync> =
        if std::env::var("TAKUSU_DESKTOP_MOCK").is_ok() {
            Arc::new(MockTransport::new(
                takusu_agent::surface::SurfaceStateMachine::new().snapshot(),
            ))
        } else {
            Arc::new(HttpTransport::new(&config.local_url, &config.token))
        };

    // Start tray.
    let _tray = tray::spawn(state.clone(), Arc::clone(&transport)).await?;

    // Start notification service.
    let (_connection, proxy) = notify::connect().await?;
    let notification_state = Arc::new(std::sync::Mutex::new(NotificationState::default()));
    let listener_proxy = proxy.clone();
    let listener_state = Arc::clone(&notification_state);
    let listener_transport = Arc::clone(&transport);
    tokio::spawn(async move {
        if let Err(e) =
            notify::run_action_listener(&listener_proxy, listener_state, listener_transport).await
        {
            tracing::error!(error=%e, "notification action listener failed");
        }
    });

    let popover = Popover::new();

    // Initial snapshot.
    let snapshot = transport.surface_snapshot().await?;
    apply_surface_event(
        &state,
        &popover,
        SurfaceEvent::Snapshot(snapshot),
        &notification_state,
        &proxy,
        transport.as_ref(),
    )
    .await?;

    // Subscribe to surface events and reconnect if the stream drops.
    loop {
        let mut events = transport.surface_events().await?;
        while let Some(event) = events.next().await {
            apply_surface_event(
                &state,
                &popover,
                event,
                &notification_state,
                &proxy,
                transport.as_ref(),
            )
            .await?;
        }
        tracing::warn!("surface event stream ended; reconnecting in 5s");
        tokio::time::sleep(Duration::from_secs(5)).await;
    }
}

async fn apply_surface_event(
    state: &DesktopState,
    popover: &Popover,
    event: SurfaceEvent,
    notification_state: &std::sync::Mutex<NotificationState>,
    proxy: &notify::NotificationsProxy<'static>,
    transport: &dyn DesktopTransport,
) -> Result<(), DesktopError> {
    // StateChanged carries a fresh snapshot and may trigger notifications.
    let changed = match &event {
        SurfaceEvent::StateChanged(s) => Some(s.clone()),
        SurfaceEvent::Snapshot(_) => None,
    };

    state.update_surface(event);

    if let Some(snapshot) = changed {
        let presentation = build_presentation(&snapshot);
        state.set_presentation(presentation.clone());

        // Surface-driven notification for check-ins and alerts.
        if let Some(notification) = presentation_to_notification(&presentation) {
            let id = notify::show(proxy, notification_state, transport, &notification).await?;
            tracing::info!(notification_id=id, "showed notification");
        }

        // Open the panel when the state asks for it.
        if matches!(snapshot.state, takusu_agent::SurfaceState::Thinking) {
            popover.show(
                state,
                PopoverRequest {
                    title: state
                        .snapshot()
                        .map(|v| v.title())
                        .unwrap_or_else(|| "takusu".into()),
                    detail: None,
                },
            );
        }
    }

    Ok(())
}

/// Convert a surface snapshot into a presentation (placeholder).
fn build_presentation(_snapshot: &SurfaceSnapshot) -> Option<Presentation> {
    // In the full implementation this fetches `/api/agent/v1/surface` and the
    // last turn result. For the scaffold, the presentation is empty.
    None
}

/// Build a desktop notification from a presentation.
fn presentation_to_notification(
    _presentation: &Option<Presentation>,
) -> Option<notify::DesktopNotification> {
    None
}
