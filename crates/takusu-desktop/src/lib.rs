//! takusu resident desktop daemon (Linux tray, notifications, compact popover).
//!
//! WI-7: the daemon owns the tray icon and desktop notifications, subscribes to
//! the local agent's surface state, and presents the compact panel. It contains
//! no planner logic; all state and actions are served by `takusu-local` once the
//! agent routes are mounted.

pub mod config;
pub mod notify;
pub mod popover;
pub mod presentation;
pub mod state;
pub mod transport;
pub mod tray;

pub use config::{Config, Theme};
pub use notify::DesktopNotification;
pub use presentation::{
    DesktopAction, DesktopPresentation, build_presentation, build_presentation_from_presentation,
    presentation_to_notification,
};
pub use state::{DesktopError, DesktopState};
pub use transport::{DesktopTransport, MockTransport};
