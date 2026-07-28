//! Golden-file snapshots for every tool's OpenAI function-calling definition.
//!
//! Each tool's `to_openai_definition()` output (name, description, and the
//! full JSON Schema for its arguments) is captured with `insta` under
//! `tests/snapshots/`.
//!
//! These snapshots are the contract between the agent and the LLM: a change in
//! a tool's schema can shift model behaviour, so any diff must be reviewed.
//!
//! To update snapshots after an intentional change:
//! ```sh
//! cargo insta accept --workspace
//! # or review interactively:
//! cargo insta review
//! ```

use std::sync::Arc;

use takusu_agent::tools::takusu::{TimeZoneCache, register_tools};
use takusu_agent::{StubUserInputProvider, ToolRegistry};
use takusu_client::Client;

fn build_registry() -> Arc<ToolRegistry> {
    let client = Client::new("http://localhost", "");
    let tz_cache = TimeZoneCache::new(client.clone());
    Arc::new_cyclic(|weak| {
        let mut registry = ToolRegistry::new();
        register_tools(
            &mut registry,
            client.clone(),
            tz_cache.clone(),
            Arc::new(StubUserInputProvider),
            weak.clone(),
        );
        registry
    })
}

#[test]
fn tool_definition_snapshots() {
    let registry = build_registry();

    insta::with_settings!({ sort_maps => true }, {
        // Only snapshot tools exposed to the model (Hidden tools are never
        // sent to the LLM, so their definitions are irrelevant).
        for name in registry.exposed_tool_names() {
            let def = registry
                .definition_for_name(&name)
                .unwrap_or_else(|| panic!("missing definition for {name}"));
            // Dynamic snapshot name: `tool_<name>.snap`.
            insta::assert_json_snapshot!(format!("tool_{name}"), def);
        }
    });
}
