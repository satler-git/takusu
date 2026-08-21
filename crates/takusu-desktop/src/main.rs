//! takusu resident desktop daemon.
//!
//! Runs the StatusNotifierItem tray, desktop notifications, and compact panel
//! for the Linux resident surface. Planner events are evaluated by the local
//! host and replayed here through the event ledger.

use std::sync::Arc;
use std::time::Duration;

use futures_util::StreamExt;
use takusu_agent::capability::CapabilityRequest;
use takusu_agent::presentation::ActionKind;
use takusu_agent::{Presentation, SurfaceEvent, SurfaceSnapshot};
use takusu_contracts::EventDeliveryState;
use tracing_subscriber::{EnvFilter, fmt};

use takusu_desktop::config::Config;
use takusu_desktop::notify::{self, NotificationState};
use takusu_desktop::popover::{Popover, PopoverRequest};
use takusu_desktop::state::{DesktopError, DesktopState};
use takusu_desktop::transport::{DesktopTransport, HttpTransport, MockTransport};
use takusu_desktop::tray;

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

    // Register this host as the desktop device (WI-11). Re-registering is
    // idempotent, so a daemon restart updates the name without clearing state.
    transport.register_device("desktop").await?;

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

    let initial_next_eval_at =
        replay_events(transport.as_ref(), &notification_state, &proxy).await?;
    let replay_transport = Arc::clone(&transport);
    let replay_state = Arc::clone(&notification_state);
    let replay_proxy = proxy.clone();
    tokio::spawn(async move {
        let mut next_eval_at = initial_next_eval_at;
        loop {
            tokio::time::sleep(eval_sleep_duration(next_eval_at)).await;
            match replay_events(replay_transport.as_ref(), &replay_state, &replay_proxy).await {
                Ok(next) => next_eval_at = next,
                Err(error) => {
                    tracing::warn!(error = %error, "planner event replay failed");
                    next_eval_at = None;
                }
            }
        }
    });

    // Background heartbeat: the desktop remains the resident authority even
    // between long planner sleeps, while the daemon is alive. `HttpTransport`
    // refreshes with a 120s TTL, so a 60s interval leaves a comfortable margin.
    let heartbeat_transport = Arc::clone(&transport);
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(Duration::from_secs(60));
        interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        loop {
            interval.tick().await;
            if let Err(error) = heartbeat_transport
                .refresh_evaluator_heartbeat("desktop")
                .await
            {
                tracing::warn!(error = %error, "desktop heartbeat failed");
            }
        }
    });

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

fn eval_sleep_duration(next_eval_at: Option<jiff::Timestamp>) -> Duration {
    const DEFAULT_SLEEP: Duration = Duration::from_secs(60);
    const MAX_SLEEP: Duration = Duration::from_secs(60 * 60);

    if let Some(next) = next_eval_at {
        let now = jiff::Timestamp::now().as_second();
        let next = next.as_second();
        if next > now {
            let seconds = (next - now).min(MAX_SLEEP.as_secs() as i64) as u64;
            return Duration::from_secs(seconds);
        }
    }
    DEFAULT_SLEEP
}

async fn replay_events(
    transport: &dyn DesktopTransport,
    notification_state: &std::sync::Mutex<NotificationState>,
    proxy: &notify::NotificationsProxy<'static>,
) -> Result<Option<jiff::Timestamp>, DesktopError> {
    // Keep the desktop evaluator heartbeat alive before each evaluation. The
    // server resolves resident authority from the priority list and heartbeat
    // TTL; only the resident device may commit events.
    transport.refresh_evaluator_heartbeat("desktop").await?;
    let result = transport.evaluate_planner_events("desktop").await?;
    for event in transport.list_planner_events("desktop").await? {
        if !matches!(
            event.delivery_state,
            EventDeliveryState::PendingDelivery | EventDeliveryState::DeferredQuietHours
        ) {
            continue;
        }
        if !transport.claim_planner_event(&event.id, "desktop").await? {
            continue;
        }
        let presentation: Presentation =
            serde_json::from_str(&event.presentation).map_err(|error| {
                DesktopError::Transport(format!("invalid event presentation: {error}"))
            })?;
        let mut actions = Vec::new();
        if let Presentation::CheckIn(card) = &presentation
            && let Some(task_id) = event.task_id.as_deref()
        {
            if matches!(
                event.kind.as_str(),
                "task_start_time_reached" | "task_non_start_continued"
            ) {
                let capability = transport
                    .mint_action_capability(&CapabilityRequest {
                        task_id: task_id.into(),
                        action: "start".into(),
                        device_id: "desktop".into(),
                        event_id: Some(event.id.clone()),
                        ..Default::default()
                    })
                    .await?;
                if let Some(action) = card.act.actions.as_slice().first() {
                    actions.push(notify::NotificationAction {
                        key: action.id.clone(),
                        label: action.label.clone(),
                        capability: Some(capability),
                    });
                }
            }

            if let Some(action) = card
                .shift
                .actions
                .as_slice()
                .iter()
                .find(|a| a.kind == ActionKind::Immediate)
            {
                let delay_capability = transport
                    .mint_action_capability(&CapabilityRequest {
                        task_id: task_id.into(),
                        action: "delay".into(),
                        device_id: "desktop".into(),
                        event_id: Some(event.id.clone()),
                        snooze_minutes: Some(10),
                        ..Default::default()
                    })
                    .await?;
                actions.push(notify::NotificationAction {
                    key: action.id.clone(),
                    label: action.label.clone(),
                    capability: Some(delay_capability),
                });
            }
        }
        let notification = notify::DesktopNotification {
            id: 0,
            title: "takusu".into(),
            body: presentation.voice_template(),
            actions,
        };

        match notify::show(proxy, notification_state, transport, &notification).await {
            Ok(_) => {
                if let Err(error) = transport
                    .update_planner_event_state(&event.id, EventDeliveryState::Delivered)
                    .await
                {
                    if let Err(rollback_err) = transport
                        .update_planner_event_state(&event.id, EventDeliveryState::PendingDelivery)
                        .await
                    {
                        tracing::warn!(
                            error = %rollback_err,
                            event_id = %event.id,
                            "failed to rollback planner event state"
                        );
                    }
                    tracing::warn!(
                        error = %error,
                        event_id = %event.id,
                        "failed to mark planner event as delivered"
                    );
                    continue;
                }
            }
            Err(error) => {
                if let Err(rollback_err) = transport
                    .update_planner_event_state(&event.id, EventDeliveryState::PendingDelivery)
                    .await
                {
                    tracing::warn!(
                        error = %rollback_err,
                        event_id = %event.id,
                        "failed to rollback planner event state"
                    );
                }
                tracing::warn!(
                    error = %error,
                    event_id = %event.id,
                    "failed to show planner event notification"
                );
                continue;
            }
        }
    }
    Ok(result.next_eval_at.map(|ts| ts.to_jiff()))
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
            tracing::info!(notification_id = id, "showed notification");
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
