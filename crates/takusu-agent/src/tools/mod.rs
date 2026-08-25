pub mod capture;
pub mod comments;
pub mod coverage;
pub mod day_details;
pub mod intake_state;
pub mod memory;
pub mod progress;
pub mod rrule;
pub mod skills;
pub mod takusu;
pub mod tool_search;
pub mod user_input;

use std::sync::{Arc, Weak};

use takusu_client::Client;

use crate::tools::takusu::TimeZoneCache;
use crate::{InvalidArgsError, ToolError, ToolRegistry, UserInputProvider};

pub(crate) fn other_error(msg: impl Into<String>) -> ToolError {
    ToolError::Other(Box::new(std::io::Error::other(msg.into())))
}

pub(crate) fn client_error(error: takusu_client::ClientError) -> ToolError {
    match error {
        takusu_client::ClientError::Api { status: 400, body } => {
            ToolError::InvalidArgs(InvalidArgsError::no_field(body))
        }
        takusu_client::ClientError::Api { status: 404, body } => ToolError::NotFound(body),
        takusu_client::ClientError::Api { status: 409, body } => ToolError::Conflict(body),
        takusu_client::ClientError::MultipleOpenWorkSessions(task_id) => {
            ToolError::InvalidArgs(InvalidArgsError::new(
                "task_ref",
                format!("multiple open work sessions for task {task_id}"),
            ))
        }
        error => ToolError::Other(Box::new(error)),
    }
}

/// Shared dependencies passed to every [`ToolModule::register`] call.
///
/// Each field is cheap to clone (`Arc`-backed or `Client` which is itself
/// `Arc`-backed), so modules can freely clone the fields they need.
pub struct ToolContext {
    pub client: Client,
    pub tz_cache: TimeZoneCache,
    pub user_input_provider: Arc<dyn UserInputProvider>,
    pub registry_ref: Weak<ToolRegistry>,
}

/// A self-registering collection of related tools.
///
/// A module that defines one or more tools implements this trait and submits
/// itself via `inventory::submit!`. The central `register_tools` function
/// (in `tools/takusu/mod.rs`) iterates all registered modules with
/// `inventory::iter` and calls `register` on each, so adding a new tool
/// module only requires creating the module and submitting it — no central
/// list to edit.
pub trait ToolModule: Send + Sync + 'static {
    /// Register this module's tools into `registry`, using `ctx` for shared
    /// dependencies.
    fn register(&self, registry: &mut ToolRegistry, ctx: &ToolContext);
}

inventory::collect!(&'static dyn ToolModule);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn client_error_maps_status_to_tool_error() {
        let err400 = takusu_client::ClientError::Api {
            status: 400,
            body: "bad".into(),
        };
        assert!(matches!(client_error(err400), ToolError::InvalidArgs(_)));

        let err404 = takusu_client::ClientError::Api {
            status: 404,
            body: "gone".into(),
        };
        assert!(matches!(client_error(err404), ToolError::NotFound(_)));

        let err409 = takusu_client::ClientError::Api {
            status: 409,
            body: "conflict".into(),
        };
        assert!(matches!(client_error(err409), ToolError::Conflict(_)));

        let err418 = takusu_client::ClientError::Api {
            status: 418,
            body: "teapot".into(),
        };
        assert!(matches!(client_error(err418), ToolError::Other(_)));
    }
}
