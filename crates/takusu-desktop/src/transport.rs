//! Transport abstraction between the desktop daemon and the local agent.
//!
//! The default implementation talks to `takusu-local` via the agent transport
//! (`/api/agent/v1/surface/*`). The mock implementation is used in tests and
//! for state→icon / notification routing verification.

use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, Mutex};

use futures_util::Stream;
use takusu_agent::{SurfaceCommand, SurfaceCommandResponse, SurfaceEvent, SurfaceSnapshot};
use takusu_agent::capability::ActionCapability;

use crate::state::DesktopError;

pub mod http;

pub use http::HttpTransport;

pub type BoxStream<'a, T> = Pin<Box<dyn Stream<Item = T> + Send + 'a>>;

/// Minimal transport contract required by the daemon.
pub trait DesktopTransport: Send + Sync {
    /// Current surface snapshot.
    fn surface_snapshot(
        &self,
    ) -> Pin<Box<dyn Future<Output = Result<SurfaceSnapshot, DesktopError>> + Send + '_>>;

    /// Server-sent surface events.
    fn surface_events(
        &self,
    ) -> Pin<Box<dyn Future<Output = Result<BoxStream<'static, SurfaceEvent>, DesktopError>> + Send + '_>>;

    /// Forward a surface command (e.g. open panel, stop TTS).
    fn send_command(
        &self,
        command: SurfaceCommand,
    ) -> Pin<Box<dyn Future<Output = Result<SurfaceCommandResponse, DesktopError>> + Send + '_>>;

    /// Authorize an immediate action via its server-issued capability.
    fn authorize_action(
        &self,
        capability: &ActionCapability,
    ) -> Pin<Box<dyn Future<Output = Result<(), DesktopError>> + Send + '_>>;
}

/// Mock transport that replays a scripted sequence of `SurfaceEvent`s.
#[derive(Debug, Clone)]
pub struct MockTransport {
    events: Arc<Mutex<Vec<SurfaceEvent>>>,
    snapshot: Arc<Mutex<SurfaceSnapshot>>,
    commands: Arc<Mutex<Vec<SurfaceCommand>>>,
    authorized: Arc<Mutex<Vec<String>>>,
}

impl MockTransport {
    pub fn new(snapshot: SurfaceSnapshot) -> Self {
        Self {
            events: Arc::new(Mutex::new(Vec::new())),
            snapshot: Arc::new(Mutex::new(snapshot)),
            commands: Arc::new(Mutex::new(Vec::new())),
            authorized: Arc::new(Mutex::new(Vec::new())),
        }
    }

    pub fn push_event(&self, event: SurfaceEvent) {
        if let Ok(mut guard) = self.events.lock() {
            guard.push(event);
        }
    }

    pub fn set_snapshot(&self, snapshot: SurfaceSnapshot) {
        if let Ok(mut guard) = self.snapshot.lock() {
            *guard = snapshot;
        }
    }

    pub fn commands(&self) -> Vec<SurfaceCommand> {
        self.commands.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }

    pub fn authorized(&self) -> Vec<String> {
        self.authorized.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }
}

impl DesktopTransport for MockTransport {
    fn surface_snapshot(
        &self,
    ) -> Pin<Box<dyn Future<Output = Result<SurfaceSnapshot, DesktopError>> + Send + '_>> {
        let snapshot = self.snapshot.lock().unwrap_or_else(|e| e.into_inner()).clone();
        Box::pin(async move { Ok(snapshot) })
    }

    fn surface_events(
        &self,
    ) -> Pin<Box<dyn Future<Output = Result<BoxStream<'static, SurfaceEvent>, DesktopError>> + Send + '_>>
    {
        let events = self.events.lock().unwrap_or_else(|e| e.into_inner()).clone();
        Box::pin(async move {
            let stream = futures_util::stream::iter(events);
            Ok(Box::pin(stream) as BoxStream<'static, SurfaceEvent>)
        })
    }

    fn send_command(
        &self,
        command: SurfaceCommand,
    ) -> Pin<Box<dyn Future<Output = Result<SurfaceCommandResponse, DesktopError>> + Send + '_>> {
        if let Ok(mut guard) = self.commands.lock() {
            guard.push(command);
        }
        let response = SurfaceCommandResponse {
            command,
            accepted: true,
            reason: None,
            snapshot: self
                .snapshot
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .clone(),
        };
        Box::pin(async move { Ok(response) })
    }

    fn authorize_action(
        &self,
        capability: &ActionCapability,
    ) -> Pin<Box<dyn Future<Output = Result<(), DesktopError>> + Send + '_>> {
        if let Ok(mut guard) = self.authorized.lock() {
            guard.push(capability.id.clone());
        }
        Box::pin(async move { Ok(()) })
    }
}
