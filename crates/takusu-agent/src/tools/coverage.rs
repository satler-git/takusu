use async_trait::async_trait;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use takusu_client::Client;
use takusu_types::Timestamp;

use crate::tools::{ToolContext, ToolModule, client_error};
use crate::{
    InferredField, InvalidArgsError, ToolError, ToolExposure, ToolName, ToolOutput, ToolRegistry,
    TypedTool, deserialize_trimmed_optional, deserialize_trimmed_required, inferred_fields_schema,
};

pub struct CoverageModule;

impl ToolModule for CoverageModule {
    fn register(&self, registry: &mut ToolRegistry, ctx: &ToolContext) {
        registry.register(Box::new(crate::tool::Typed(CoverageConfirm {
            client: ctx.client.clone(),
        })));
    }
}

static COVERAGE_MODULE: &dyn ToolModule = &CoverageModule;

inventory::submit!(COVERAGE_MODULE);

/// Record a coverage confirmation after the user has stated what happened
/// during a local time interval (intake, target period, or system check).
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct CoverageConfirmArgs {
    /// Start of the covered interval as an RFC 3339 timestamp.
    #[serde(deserialize_with = "deserialize_trimmed_required")]
    #[schemars(with = "String")]
    start_at: String,
    /// End of the covered interval as an RFC 3339 timestamp.
    #[serde(deserialize_with = "deserialize_trimmed_required")]
    #[schemars(with = "String")]
    end_at: String,
    #[serde(deserialize_with = "deserialize_trimmed_required")]
    #[schemars(with = "String")]
    timezone: String,
    #[serde(default, deserialize_with = "deserialize_trimmed_optional")]
    #[schemars(with = "Option<String>")]
    source: Option<String>,
    #[serde(default, deserialize_with = "deserialize_trimmed_optional")]
    #[schemars(with = "Option<String>")]
    calendar_health: Option<String>,
    #[serde(default, deserialize_with = "deserialize_trimmed_optional")]
    #[schemars(with = "Option<String>")]
    why: Option<String>,
    #[serde(default)]
    inferred_fields: Vec<InferredField>,
    #[serde(default)]
    warnings: Vec<String>,
}

struct CoverageConfirm {
    client: Client,
}

#[async_trait]
impl TypedTool for CoverageConfirm {
    type Params = CoverageConfirmArgs;

    fn name(&self) -> &'static str {
        ToolName::CoverageConfirm.into()
    }

    fn description(&self) -> &'static str {
        "Record a coverage confirmation for a local time interval. Use this after an intake or capture flow when the user has confirmed what happened during a period. Does not require approval; it is an immediate write."
    }

    fn exposure(&self) -> ToolExposure {
        ToolExposure::Direct
    }

    fn parameters_schema(&self) -> serde_json::Value {
        let mut schema = self.default_parameters_schema();
        if let Some(props) = schema
            .get_mut("properties")
            .and_then(serde_json::Value::as_object_mut)
        {
            props.insert(
                "inferred_fields".into(),
                inferred_fields_schema("Fields inferred from context."),
            );
            if let Some(timezone) = props.get_mut("timezone").and_then(Value::as_object_mut) {
                timezone.insert(
                    "description".into(),
                    "IANA timezone for the interval, e.g. Asia/Tokyo".into(),
                );
            }
            if let Some(source) = props.get_mut("source").and_then(Value::as_object_mut) {
                source.insert(
                    "description".into(),
                    "Why the confirmation is being recorded.".into(),
                );
                source.insert(
                    "enum".into(),
                    json!(["intake_complete", "target_period", "system"]),
                );
            }
            if let Some(health) = props
                .get_mut("calendar_health")
                .and_then(Value::as_object_mut)
            {
                health.insert(
                    "description".into(),
                    "Health of the external calendar this confirmation was drawn from.".into(),
                );
                health.insert("enum".into(), json!(["ok", "stale", "error"]));
            }
        }
        schema
    }

    fn validate_args(&self, args: &Self::Params) -> Result<(), InvalidArgsError> {
        if args.timezone.is_empty() {
            return Err(InvalidArgsError::new("timezone", "timezone is required"));
        }
        let health = args.calendar_health.as_deref().unwrap_or("ok");
        if !matches!(health, "ok" | "stale" | "error") {
            return Err(InvalidArgsError::new(
                "calendar_health",
                "calendar_health must be 'ok', 'stale', or 'error'",
            ));
        }
        Ok(())
    }

    async fn call_typed(&self, args: Self::Params) -> Result<ToolOutput, ToolError> {
        let start_at: Timestamp = args.start_at.parse().map_err(|_| {
            ToolError::InvalidArgs(InvalidArgsError::new("start_at", "invalid timestamp"))
        })?;
        let end_at: Timestamp = args.end_at.parse().map_err(|_| {
            ToolError::InvalidArgs(InvalidArgsError::new("end_at", "invalid timestamp"))
        })?;
        if start_at > end_at {
            return Err(ToolError::InvalidArgs(InvalidArgsError::new(
                "end_at",
                "end_at must not be before start_at",
            )));
        }
        let schedule_revision = self
            .client
            .get_schedule_revision()
            .await
            .map_err(client_error)?;
        let body = takusu_client::CreateCoverageConfirmation {
            start_at,
            end_at,
            timezone: args.timezone,
            source: args.source.unwrap_or_else(|| "intake_complete".into()),
            schedule_revision,
            calendar_health: args.calendar_health.unwrap_or_else(|| "ok".into()),
            operation_id: None,
        };
        let row = self
            .client
            .create_coverage_confirmation(&body)
            .await
            .map_err(client_error)?;
        Ok(ToolOutput {
            content: serde_json::to_string(&row).unwrap_or_default(),
            why: args.why,
            warnings: args.warnings,
            proposed_changes: Vec::new(),
            inferred_fields: args.inferred_fields,
            schedule_dirty: false,
            ..Default::default()
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn coverage_confirm_name_matches_tool_name() {
        let tool = CoverageConfirm {
            client: Client::new("http://localhost", ""),
        };
        assert_eq!(tool.name(), "coverage_confirm");
    }

    #[test]
    fn schema_does_not_expose_schedule_revision() {
        let tool = CoverageConfirm {
            client: Client::new("http://localhost", ""),
        };
        let schema = tool.parameters_schema();
        let props = schema
            .get("properties")
            .and_then(|v| v.as_object())
            .unwrap();
        assert!(!props.contains_key("schedule_revision"));
    }

    #[test]
    fn validate_args_rejects_invalid_calendar_health() {
        let tool = CoverageConfirm {
            client: Client::new("http://localhost", ""),
        };
        let mut args = CoverageConfirmArgs {
            start_at: "2026-01-01T00:00:00Z".into(),
            end_at: "2026-01-01T01:00:00Z".into(),
            timezone: "Asia/Tokyo".into(),
            source: None,
            calendar_health: Some("broken".into()),
            why: None,
            inferred_fields: Vec::new(),
            warnings: Vec::new(),
        };
        assert!(tool.validate_args(&args).is_err());
        args.calendar_health = Some("ok".into());
        assert!(tool.validate_args(&args).is_ok());
        args.calendar_health = Some("stale".into());
        assert!(tool.validate_args(&args).is_ok());
        args.calendar_health = Some("error".into());
        assert!(tool.validate_args(&args).is_ok());
        args.calendar_health = None;
        assert!(tool.validate_args(&args).is_ok());
    }

    #[tokio::test]
    async fn call_typed_rejects_swapped_interval() {
        let tool = CoverageConfirm {
            client: Client::new("http://localhost", ""),
        };
        let args = CoverageConfirmArgs {
            start_at: "2026-01-01T02:00:00Z".into(),
            end_at: "2026-01-01T01:00:00Z".into(),
            timezone: "Asia/Tokyo".into(),
            source: None,
            calendar_health: None,
            why: None,
            inferred_fields: Vec::new(),
            warnings: Vec::new(),
        };
        assert!(matches!(
            tool.call_typed(args).await,
            Err(ToolError::InvalidArgs(_))
        ));
    }
}
