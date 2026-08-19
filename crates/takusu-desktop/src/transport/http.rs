//! HTTP transport for the desktop daemon talking to `takusu-local`.
//!
//! Targets the agent routes mounted under `/api/agent/v1/`:
//! - `GET /surface` → snapshot
//! - `GET /surface/events` → SSE stream
//! - `POST /surface/commands` → command
//! - `POST /actions` → capability authorization

use std::future::Future;
use std::pin::Pin;

use eventsource_stream::{Event as SseEvent, Eventsource};
use futures_util::{StreamExt, TryStreamExt};
use reqwest::header::{self, HeaderValue};
use takusu_agent::capability::ActionCapability;
use takusu_agent::transport::{SurfaceCommandRequest, Versioned, API_VERSION};
use takusu_agent::{
    SurfaceCommand, SurfaceCommandResponse, SurfaceEvent, SurfaceSnapshot,
};

use crate::state::DesktopError;
use crate::transport::{BoxStream, DesktopTransport};

/// HTTP client that talks to the local agent API.
#[derive(Debug, Clone)]
pub struct HttpTransport {
    client: reqwest::Client,
    base_url: String,
    token: String,
}

impl HttpTransport {
    pub fn new(base_url: impl Into<String>, token: impl Into<String>) -> Self {
        Self {
            client: reqwest::Client::new(),
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
    ) -> Pin<Box<dyn Future<Output = Result<BoxStream<'static, SurfaceEvent>, DesktopError>> + Send + '_>>
    {
        let this = self.clone();
        Box::pin(async move {
            let response = this
                .client
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
                .map_ok(|event: SseEvent| (event.id, serde_json::from_str::<SurfaceEvent>(&event.data)));

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
    ) -> Pin<Box<dyn Future<Output = Result<SurfaceCommandResponse, DesktopError>> + Send + '_>> {
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
    ) -> Pin<Box<dyn Future<Output = Result<(), DesktopError>> + Send + '_>> {
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
            Ok(())
        })
    }
}
