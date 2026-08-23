//! HTTP transport for the desktop daemon talking to `takusu-local`.
//!
//! Targets the agent routes mounted under `/api/agent/v1/`:
//! - `GET /surface` → snapshot
//! - `GET /surface/events` → SSE stream
//! - `POST /surface/commands` → command
//! - `POST /actions` → capability authorization

use std::future::Future;
use std::pin::Pin;
use std::time::Duration;

use eventsource_stream::{Event as SseEvent, Eventsource};
use futures_util::{StreamExt, TryStreamExt};
use reqwest::header::{self, HeaderValue};
use serde::Deserialize;
use takusu_agent::capability::{ActionCapability, CapabilityRequest};
use takusu_agent::events::EvaluationResult;
use takusu_agent::transport::{API_VERSION, SurfaceCommandRequest, Versioned};
use takusu_agent::{
    DeliveryMode, Presentation, SurfaceCommand, SurfaceCommandResponse, SurfaceEvent,
    SurfaceSnapshot,
};
use takusu_contracts::{DeviceRow, EventDeliveryState, EventLedgerRow};

use crate::state::DesktopError;
use crate::transport::{BoxStream, DesktopTransport};

/// HTTP client that talks to the local agent API.
#[derive(Debug, Clone)]
pub struct HttpTransport {
    client: reqwest::Client,
    sse_client: reqwest::Client,
    base_url: String,
    token: String,
}

#[derive(Debug, Deserialize)]
struct DeliveryModeResponse {
    mode: DeliveryMode,
}

impl HttpTransport {
    pub fn new(base_url: impl Into<String>, token: impl Into<String>) -> Self {
        // Bound the HTTP client so a hung local host cannot stall the daemon
        // indefinitely. The SSE long poll is not affected because it is a
        // streaming response, but each request/response body must be received
        // within a reasonable window.
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(30))
            .connect_timeout(Duration::from_secs(10))
            .build()
            .unwrap_or_else(|_| reqwest::Client::new());
        // The SSE stream is a long-lived connection; a separate client with no
        // request timeout keeps it open while the standard client still bounds
        // regular calls.
        let sse_client = reqwest::Client::builder()
            .connect_timeout(Duration::from_secs(10))
            .build()
            .unwrap_or_else(|_| reqwest::Client::new());
        Self {
            client,
            sse_client,
            base_url: base_url.into().trim_end_matches('/').to_string(),
            token: token.into(),
        }
    }

    fn bearer(&self) -> Result<HeaderValue, DesktopError> {
        HeaderValue::from_str(&format!("Bearer {}", self.token))
            .map_err(|e| DesktopError::Transport(format!("invalid bearer token: {e}")))
    }
}

impl DesktopTransport for HttpTransport {
    fn surface_snapshot(
        &self,
    ) -> Pin<Box<dyn Future<Output = Result<SurfaceSnapshot, DesktopError>> + Send + '_>> {
        let this = self.clone();
        Box::pin(async move {
            let response = this
                .client
                .get(format!("{}/api/agent/v1/surface", this.base_url))
                .header(header::AUTHORIZATION, this.bearer()?)
                .send()
                .await
                .map_err(|e| DesktopError::Transport(e.to_string()))?;
            if !response.status().is_success() {
                return Err(DesktopError::Transport(format!(
                    "surface snapshot failed: {}",
                    response.status()
                )));
            }
            response
                .json::<Versioned<SurfaceSnapshot>>()
                .await
                .map(|v| v.value)
                .map_err(|e| DesktopError::Transport(e.to_string()))
        })
    }

    fn surface_events(
        &self,
    ) -> Pin<
        Box<
            dyn Future<Output = Result<BoxStream<'static, SurfaceEvent>, DesktopError>> + Send + '_,
        >,
    > {
        let this = self.clone();
        Box::pin(async move {
            let response = this
                .sse_client
                .get(format!("{}/api/agent/v1/surface/events", this.base_url))
                .header(header::AUTHORIZATION, this.bearer()?)
                .send()
                .await
                .map_err(|e| DesktopError::Transport(e.to_string()))?;
            if !response.status().is_success() {
                return Err(DesktopError::Transport(format!(
                    "surface events failed: {}",
                    response.status()
                )));
            }

            let stream = response
                .bytes_stream()
                .eventsource()
                .map_ok(|event: SseEvent| {
                    (event.id, serde_json::from_str::<SurfaceEvent>(&event.data))
                });

            let parsed = stream.filter_map(|item| async move {
                match item {
                    Ok((_, Ok(event))) => Some(event),
                    Ok((_, Err(e))) => {
                        tracing::warn!(error=%e, "failed to deserialize surface event");
                        None
                    }
                    Err(e) => {
                        tracing::warn!(error=%e, "surface event stream error");
                        None
                    }
                }
            });

            Ok(Box::pin(parsed) as BoxStream<'static, SurfaceEvent>)
        })
    }

    fn send_command(
        &self,
        command: SurfaceCommand,
    ) -> Pin<Box<dyn Future<Output = Result<SurfaceCommandResponse, DesktopError>> + Send + '_>>
    {
        let this = self.clone();
        Box::pin(async move {
            let body = Versioned {
                version: API_VERSION,
                value: SurfaceCommandRequest {
                    command,
                    operation_id: None,
                },
            };
            let response = this
                .client
                .post(format!("{}/api/agent/v1/surface/commands", this.base_url))
                .header(header::AUTHORIZATION, this.bearer()?)
                .json(&body)
                .send()
                .await
                .map_err(|e| DesktopError::Transport(e.to_string()))?;
            if !response.status().is_success() {
                return Err(DesktopError::Transport(format!(
                    "surface command failed: {}",
                    response.status()
                )));
            }
            response
                .json::<Versioned<SurfaceCommandResponse>>()
                .await
                .map(|v| v.value)
                .map_err(|e| DesktopError::Transport(e.to_string()))
        })
    }

    fn authorize_action(
        &self,
        capability: &ActionCapability,
    ) -> Pin<Box<dyn Future<Output = Result<Presentation, DesktopError>> + Send + '_>> {
        let this = self.clone();
        let capability = capability.clone();
        Box::pin(async move {
            let body = Versioned {
                version: API_VERSION,
                value: capability,
            };
            let response = this
                .client
                .post(format!("{}/api/agent/v1/actions", this.base_url))
                .header(header::AUTHORIZATION, this.bearer()?)
                .json(&body)
                .send()
                .await
                .map_err(|e| DesktopError::Transport(e.to_string()))?;
            if !response.status().is_success() {
                return Err(DesktopError::Transport(format!(
                    "authorize action failed: {}",
                    response.status()
                )));
            }
            response
                .json::<Versioned<Presentation>>()
                .await
                .map(|v| v.value)
                .map_err(|e| DesktopError::Transport(e.to_string()))
        })
    }

    fn register_device(
        &self,
        device_id: &str,
    ) -> Pin<Box<dyn Future<Output = Result<(), DesktopError>> + Send + '_>> {
        let this = self.clone();
        let device_id = device_id.to_string();
        Box::pin(async move {
            let body = serde_json::json!({
                "id": device_id,
                "name": "desktop",
                "platform": "desktop",
            });
            let response = this
                .client
                .post(format!("{}/api/devices", this.base_url))
                .header(header::AUTHORIZATION, this.bearer()?)
                .json(&body)
                .send()
                .await
                .map_err(|e| DesktopError::Transport(e.to_string()))?;
            if !response.status().is_success() {
                return Err(DesktopError::Transport(format!(
                    "register device failed: {}",
                    response.status()
                )));
            }
            Ok(())
        })
    }

    fn refresh_evaluator_heartbeat(
        &self,
        device_id: &str,
    ) -> Pin<Box<dyn Future<Output = Result<(), DesktopError>> + Send + '_>> {
        let this = self.clone();
        let device_id = device_id.to_string();
        Box::pin(async move {
            let now = jiff::Timestamp::now();
            // The desktop daemon also sends a background heartbeat every 60s;
            // use a 120s TTL so transient scheduling jitter does not demote
            // resident authority between refreshes.
            let until = jiff::Timestamp::from_second(now.as_second() + 120).unwrap_or(now);
            let body = serde_json::json!({
                "device_id": device_id,
                "until": until.to_string(),
            });
            let response = this
                .client
                .post(format!(
                    "{}/api/devices/{}/heartbeat",
                    this.base_url,
                    takusu_types::url_encode(&device_id)
                ))
                .header(header::AUTHORIZATION, this.bearer()?)
                .json(&body)
                .send()
                .await
                .map_err(|e| DesktopError::Transport(e.to_string()))?;
            if !response.status().is_success() {
                return Err(DesktopError::Transport(format!(
                    "refresh heartbeat failed: {}",
                    response.status()
                )));
            }
            Ok(())
        })
    }

    fn evaluate_planner_events(
        &self,
        device_id: &str,
    ) -> Pin<Box<dyn Future<Output = Result<EvaluationResult, DesktopError>> + Send + '_>> {
        let this = self.clone();
        let device_id = device_id.to_string();
        Box::pin(async move {
            let response = this
                .client
                .post(format!("{}/api/events/evaluate", this.base_url))
                .header(header::AUTHORIZATION, this.bearer()?)
                .json(&serde_json::json!({ "device_id": device_id }))
                .send()
                .await
                .map_err(|e| DesktopError::Transport(e.to_string()))?;
            response
                .error_for_status()
                .map_err(|e| DesktopError::Transport(e.to_string()))?
                .json::<EvaluationResult>()
                .await
                .map_err(|e| DesktopError::Transport(e.to_string()))
        })
    }

    fn list_planner_events(
        &self,
        device_id: &str,
    ) -> Pin<Box<dyn Future<Output = Result<Vec<EventLedgerRow>, DesktopError>> + Send + '_>> {
        let this = self.clone();
        let device_id = device_id.to_string();
        Box::pin(async move {
            let response = this
                .client
                .get(format!("{}/api/events", this.base_url))
                .query(&[("device_id", device_id)])
                .header(header::AUTHORIZATION, this.bearer()?)
                .send()
                .await
                .map_err(|e| DesktopError::Transport(e.to_string()))?;
            response
                .error_for_status()
                .map_err(|e| DesktopError::Transport(e.to_string()))?
                .json::<Vec<EventLedgerRow>>()
                .await
                .map_err(|e| DesktopError::Transport(e.to_string()))
        })
    }

    fn claim_planner_event(
        &self,
        event_id: &str,
        device_id: &str,
    ) -> Pin<Box<dyn Future<Output = Result<bool, DesktopError>> + Send + '_>> {
        let this = self.clone();
        let path = format!(
            "{}/api/events/{}/claim",
            this.base_url,
            takusu_types::url_encode(event_id)
        );
        let device_id = device_id.to_string();
        Box::pin(async move {
            let response = this
                .client
                .post(path)
                .header(header::AUTHORIZATION, this.bearer()?)
                .json(&serde_json::json!({ "device_id": device_id }))
                .send()
                .await
                .map_err(|e| DesktopError::Transport(e.to_string()))?;
            response
                .error_for_status()
                .map_err(|e| DesktopError::Transport(e.to_string()))?
                .json::<serde_json::Value>()
                .await
                .map(|value| {
                    value
                        .get("claimed")
                        .and_then(serde_json::Value::as_bool)
                        .unwrap_or(false)
                })
                .map_err(|e| DesktopError::Transport(e.to_string()))
        })
    }

    fn event_delivery_mode(
        &self,
        event_id: &str,
        device_id: &str,
    ) -> Pin<Box<dyn Future<Output = Result<DeliveryMode, DesktopError>> + Send + '_>> {
        let this = self.clone();
        let path = format!(
            "{}/api/events/{}/delivery",
            this.base_url,
            takusu_types::url_encode(event_id)
        );
        let device_id = device_id.to_string();
        Box::pin(async move {
            let response = this
                .client
                .get(path)
                .query(&[("device_id", device_id)])
                .header(header::AUTHORIZATION, this.bearer()?)
                .send()
                .await
                .map_err(|e| DesktopError::Transport(e.to_string()))?;
            response
                .error_for_status()
                .map_err(|e| DesktopError::Transport(e.to_string()))?
                .json::<DeliveryModeResponse>()
                .await
                .map(|value| value.mode)
                .map_err(|e| DesktopError::Transport(e.to_string()))
        })
    }

    fn update_planner_event_state(
        &self,
        event_id: &str,
        state: EventDeliveryState,
    ) -> Pin<Box<dyn Future<Output = Result<EventLedgerRow, DesktopError>> + Send + '_>> {
        let this = self.clone();
        let path = format!(
            "{}/api/events/{}/state",
            this.base_url,
            takusu_types::url_encode(event_id)
        );
        Box::pin(async move {
            let response = this
                .client
                .put(path)
                .header(header::AUTHORIZATION, this.bearer()?)
                .json(&state)
                .send()
                .await
                .map_err(|e| DesktopError::Transport(e.to_string()))?;
            response
                .error_for_status()
                .map_err(|e| DesktopError::Transport(e.to_string()))?
                .json::<EventLedgerRow>()
                .await
                .map_err(|e| DesktopError::Transport(e.to_string()))
        })
    }

    fn suppress_device(
        &self,
        device_id: &str,
        minutes: i64,
    ) -> Pin<Box<dyn Future<Output = Result<DeviceRow, DesktopError>> + Send + '_>> {
        let this = self.clone();
        let path = format!(
            "{}/api/devices/{}/suppress",
            this.base_url,
            takusu_types::url_encode(device_id)
        );
        Box::pin(async move {
            let response = this
                .client
                .post(path)
                .header(header::AUTHORIZATION, this.bearer()?)
                .json(&takusu_contracts::SuppressDevice { minutes })
                .send()
                .await
                .map_err(|e| DesktopError::Transport(e.to_string()))?;
            response
                .error_for_status()
                .map_err(|e| DesktopError::Transport(e.to_string()))?
                .json::<DeviceRow>()
                .await
                .map_err(|e| DesktopError::Transport(e.to_string()))
        })
    }

    fn mint_action_capability(
        &self,
        request: &CapabilityRequest,
    ) -> Pin<Box<dyn Future<Output = Result<ActionCapability, DesktopError>> + Send + '_>> {
        let this = self.clone();
        let body = Versioned {
            version: API_VERSION,
            value: request.clone(),
        };
        Box::pin(async move {
            let response = this
                .client
                .post(format!("{}/api/agent/v1/capabilities", this.base_url))
                .header(header::AUTHORIZATION, this.bearer()?)
                .json(&body)
                .send()
                .await
                .map_err(|e| DesktopError::Transport(e.to_string()))?;
            response
                .error_for_status()
                .map_err(|e| DesktopError::Transport(e.to_string()))?
                .json::<Versioned<ActionCapability>>()
                .await
                .map(|value| value.value)
                .map_err(|e| DesktopError::Transport(e.to_string()))
        })
    }
}
