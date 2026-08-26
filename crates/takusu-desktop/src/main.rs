//! takusu resident desktop daemon.
//!
//! Runs the StatusNotifierItem tray, desktop notifications, and compact panel
//! for the Linux resident surface. Planner events are evaluated by the local
//! host and replayed here through the event ledger.

use std::sync::Arc;
use std::time::Duration;

use futures_util::StreamExt;
use takusu_agent::{DeliveryMode, SurfaceCommand, SurfaceEvent, SurfaceState};
use takusu_contracts::EventDeliveryState;

#[cfg(feature = "audio-device")]
use takusu_desktop::audio::speak_cue;
#[cfg(feature = "audio-device")]
use takusu_desktop::audio::speak_presentation;
use tracing_subscriber::{EnvFilter, fmt};

use takusu_desktop::config::Config;
use takusu_desktop::local;
use takusu_desktop::notify::{self, NotificationState};
use takusu_desktop::popover::{Popover, PopoverRequest};
use takusu_desktop::presentation::{build_presentation, presentation_to_notification};
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
    let mut config = Config::load().map_err(|e| DesktopError::Transport(e.to_string()))?;

    let use_mock = std::env::var("TAKUSU_DESKTOP_MOCK").is_ok();

    // If no local URL is configured and we are not in mock mode, start an
    // in-process `takusu-local` server so the desktop daemon is self-contained.
    if !use_mock && config.local_url.is_empty() {
        local::start(&mut config).await.map_err(|e| {
            tracing::error!(error = %e, "failed to start embedded takusu-local");
            e
        })?;
    }

    if !use_mock && config.token.is_empty() {
        tracing::warn!("no bearer token configured; agent routes may fail");
    }

    let state = DesktopState::new(config.clone());

    // Real transport: HTTP to takusu-local agent routes.
    let transport: Arc<dyn DesktopTransport + Send + Sync> = if use_mock {
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
    let listener_desktop_state = state.clone();
    tokio::spawn(async move {
        if let Err(e) = notify::run_action_listener(
            &listener_proxy,
            listener_state,
            listener_transport,
            Some(listener_desktop_state),
        )
        .await
        {
            tracing::error!(error=%e, "notification action listener failed");
        }
    });

    let popover = Popover::new_with_transport(Arc::clone(&transport));

    let state_for_popover = state.clone();
    let popover_for_callback = popover.clone();
    state.set_on_change(move || {
        show_popover_for_state(&state_for_popover, &popover_for_callback);
    });

    #[cfg(feature = "audio-device")]
    {
        // Announce listening start/end with the configured TTS cues.
        let cue_state = state.clone();
        state.set_on_cue(move |cue| {
            let cue_state = cue_state.clone();
            tokio::spawn(async move {
                if cue_state.do_not_disturb() {
                    return;
                }
                if let Err(error) = speak_cue(cue).await {
                    tracing::warn!(?cue, error=%error, "listening cue failed");
                }
            });
        });
    }

    let initial_next_eval_at =
        replay_events(transport.as_ref(), &state, &notification_state, &proxy).await?;
    let replay_transport = Arc::clone(&transport);
    let replay_state = state.clone();
    let replay_notification_state = Arc::clone(&notification_state);
    let replay_proxy = proxy.clone();
    tokio::spawn(async move {
        let mut next_eval_at = initial_next_eval_at;
        loop {
            tokio::time::sleep(eval_sleep_duration(next_eval_at)).await;
            match replay_events(
                replay_transport.as_ref(),
                &replay_state,
                &replay_notification_state,
                &replay_proxy,
            )
            .await
            {
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
    apply_surface_event(&state, SurfaceEvent::Snapshot(snapshot), transport.as_ref()).await?;

    // Subscribe to surface events and reconnect if the stream drops.
    loop {
        let mut events = transport.surface_events().await?;
        while let Some(event) = events.next().await {
            apply_surface_event(&state, event, transport.as_ref()).await?;
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
    state: &DesktopState,
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

        let delivery_mode = transport.event_delivery_mode(&event.id, "desktop").await?;
        match delivery_mode {
            DeliveryMode::Suppress => {
                tracing::info!(event_id = %event.id, "suppressed planner event delivery");
                if let Err(error) = transport
                    .update_planner_event_state(&event.id, EventDeliveryState::Ignored)
                    .await
                {
                    tracing::warn!(
                        error = %error,
                        event_id = %event.id,
                        "failed to mark suppressed planner event as ignored"
                    );
                }
                continue;
            }
            DeliveryMode::DeferQuietHours => {
                if let Err(error) = transport
                    .update_planner_event_state(&event.id, EventDeliveryState::DeferredQuietHours)
                    .await
                {
                    tracing::warn!(
                        error = %error,
                        event_id = %event.id,
                        "failed to defer planner event to quiet hours"
                    );
                }
                continue;
            }
            _ => {}
        }

        let desktop_presentation = build_presentation(transport, &event, "desktop").await?;

        // Surface the current planner event in the tray and compact panel.
        state.set_current_presentation(Some(desktop_presentation.clone()));

        // do_not_disturb suppresses audible output and system notifications, but
        // the tray / compact panel may already show the latest presentation. The
        // next delivery attempt will re-evaluate delivery mode and can speak or
        // notify once do_not_disturb is off.
        if state.do_not_disturb() {
            tracing::info!(event_id = %event.id, "suppressed notification in do-not-disturb");
            continue;
        }

        let spoke = match delivery_mode {
            DeliveryMode::Speak => {
                #[cfg(feature = "audio-device")]
                {
                    match speak_presentation(&desktop_presentation.presentation).await {
                        Ok(()) => {
                            tracing::info!(event_id = %event.id, "spoke planner event");
                            true
                        }
                        Err(error) => {
                            tracing::warn!(
                                error = %error,
                                event_id = %event.id,
                                "failed to speak planner event; falling back to notification"
                            );
                            false
                        }
                    }
                }
                #[cfg(not(feature = "audio-device"))]
                {
                    tracing::warn!(
                        event_id = %event.id,
                        "audio-device feature disabled; falling back to notification"
                    );
                    false
                }
            }
            DeliveryMode::Notify => false,
            _ => unreachable!("suppress and defer quiet hours handled above"),
        };

        if !spoke {
            let notification = presentation_to_notification(&desktop_presentation);
            if let Err(error) =
                notify::show(proxy, notification_state, transport, &notification).await
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
                    "failed to show planner event notification"
                );
                continue;
            }
        }

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
    Ok(result.next_eval_at.map(|ts| ts.to_jiff()))
}

async fn apply_surface_event(
    state: &DesktopState,
    event: SurfaceEvent,
    transport: &dyn DesktopTransport,
) -> Result<(), DesktopError> {
    // StateChanged carries a fresh snapshot and may trigger commands or UI.
    let changed = match &event {
        SurfaceEvent::StateChanged(s) => Some(s.clone()),
        SurfaceEvent::Snapshot(_) => None,
    };

    state.update_surface(event);

    if let Some(snapshot) = changed {
        let command = match snapshot.state {
            SurfaceState::Thinking => Some(SurfaceCommand::OpenPanel),
            SurfaceState::WaitingForApproval => Some(SurfaceCommand::OpenApproval),
            SurfaceState::Error => Some(SurfaceCommand::ShowRecovery),
            _ => None,
        };

        if let Some(command) = command {
            match transport.send_command(command).await {
                Ok(response) => {
                    if !response.accepted {
                        tracing::warn!(
                            command = ?command,
                            reason = ?response.reason,
                            "surface command not accepted"
                        );
                    }
                }
                Err(error) => {
                    tracing::warn!(
                        command = ?command,
                        error = %error,
                        "failed to send surface command"
                    );
                }
            }
        }
    }

    Ok(())
}

fn show_popover_for_state(state: &DesktopState, popover: &Popover) {
    let view = state.snapshot();
    let snapshot = view
        .as_ref()
        .map(|v| v.snapshot.clone())
        .unwrap_or_default();
    let title = view
        .as_ref()
        .map(|v| v.title())
        .unwrap_or_else(|| "takusu".into());
    let detail = view.as_ref().and_then(|v| v.detail());
    let active = state.voice_session_active();
    let invited = state.consume_voice_invite();
    let panel_open = state.panel_open();

    let show_voice_button = cfg!(feature = "audio-device") && (active || invited);

    if active
        || invited
        || panel_open
        || matches!(
            snapshot.state,
            SurfaceState::Thinking
                | SurfaceState::WaitingForApproval
                | SurfaceState::Error
                | SurfaceState::WaitingForUser
        )
    {
        popover.show(
            state,
            PopoverRequest {
                title,
                detail: detail.or_else(|| snapshot.error.clone()),
                actions: state.quick_actions(),
                voice_button: show_voice_button,
            },
        );
    } else {
        popover.hide();
    }
}
