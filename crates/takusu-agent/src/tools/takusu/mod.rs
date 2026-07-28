mod common;
mod mutation;
mod read_tools;
#[cfg(test)]
mod tests;

use std::sync::{Arc, Weak};

use takusu_client::Client;

use crate::{ToolRegistry, UserInputProvider};

// Re-export shared helpers so external modules (progress.rs, rrule.rs,
// day_details.rs, runner.rs, lib.rs) can access them at `crate::tools::takusu::*`.
pub use common::*;

// Bring submodule items into scope for `register_tools` and for tests via
// `use super::*`.
use mutation::*;
use read_tools::*;

/// Registers planner read tools, approval-only mutation proposals, and the ASR
/// correction tool.
pub fn register_tools(
    registry: &mut ToolRegistry,
    client: Client,
    tz_cache: TimeZoneCache,
    user_input_provider: Arc<dyn UserInputProvider>,
    registry_ref: Weak<ToolRegistry>,
) {
    register_read_tools(registry, client.clone(), tz_cache.clone());
    register_mutation_tools(registry, client.clone(), tz_cache.clone());
    crate::tools::progress::register_tools(registry, client.clone(), tz_cache.clone());
    crate::tools::rrule::register_tools(registry, tz_cache.clone());
    crate::tools::day_details::register_tools(registry, client.clone(), tz_cache.clone());
    crate::tools::memory::register_tools(registry, client.clone());
    registry.register(Box::new(crate::tool::Typed(PreviewScheduleTool {
        client: client.clone(),
        tz_cache: tz_cache.clone(),
    })));
    registry.register(Box::new(MoveTaskTool {
        client: client.clone(),
        tz_cache,
    }));
    crate::tools::skills::register_tools(registry, client.clone());
    crate::tools::user_input::register_user_input_tool(registry, user_input_provider);
    registry.register(Box::new(crate::tool::Typed(
        crate::tools::tool_search::ToolSearch::from_registry(registry_ref),
    )));
}
