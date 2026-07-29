mod common;
mod mutation;
mod read_tools;
#[cfg(test)]
mod tests;

use takusu_client::Client;

use crate::ToolRegistry;
use crate::tools::{ToolContext, ToolModule};

// Re-export shared helpers so external modules (progress.rs, rrule.rs,
// day_details.rs, runner.rs, lib.rs, takusu-android) can access them at
// `crate::tools::takusu::*` / `takusu_agent::tools::takusu::*`.
pub(crate) use super::client_error;
pub use common::TimeZoneCache;
pub(crate) use common::{TaskContext, server_timezone, strip_leading_hash, task_json};

// Bring submodule items into scope for `register_tools`.
use mutation::*;
use read_tools::*;

/// Registers every tool module collected via `inventory`.
///
/// Each module in `tools/` implements [`ToolModule`] and submits itself with
/// `inventory::submit!`. This function creates a [`ToolContext`] from the
/// shared dependencies and iterates all registered modules, so adding a new
/// tool module does not require editing this function.
pub fn register_tools(
    registry: &mut ToolRegistry,
    client: Client,
    tz_cache: TimeZoneCache,
    user_input_provider: std::sync::Arc<dyn crate::UserInputProvider>,
    registry_ref: std::sync::Weak<ToolRegistry>,
) {
    let ctx = ToolContext {
        client,
        tz_cache,
        user_input_provider,
        registry_ref,
    };
    for module in inventory::iter::<&'static dyn ToolModule> {
        module.register(registry, &ctx);
    }
}

/// Module for the core planner read and mutation tools (tasks, habits,
/// schedule, preview, move).
struct TakusuModule;

impl ToolModule for TakusuModule {
    fn register(&self, registry: &mut ToolRegistry, ctx: &ToolContext) {
        register_read_tools(registry, ctx.client.clone(), ctx.tz_cache.clone());
        register_mutation_tools(registry, ctx.client.clone(), ctx.tz_cache.clone());
    }
}

static TAKUSU_MODULE: &dyn ToolModule = &TakusuModule;

inventory::submit!(TAKUSU_MODULE);
