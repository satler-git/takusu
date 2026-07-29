use async_trait::async_trait;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use takusu_client::{Client, CreateMemory, MemoryQuery, MemoryRow, SimilarTaskQuery, UpdateMemory};
use takusu_types::{MemoryKind, MemorySource, SubjectType};

use crate::tools::{ToolContext, ToolModule};
use crate::{
    ChangeOperation, InferredField, InvalidArgsError, ProposalContent, ProposedChange, Target,
    TargetKind, ToolError, ToolExposure, ToolName, ToolOutput, ToolRegistry, TypedTool,
    deserialize_trimmed_optional, deserialize_trimmed_required, inferred_fields_schema,
};

pub fn client_error(error: takusu_client::ClientError) -> ToolError {
    match error {
        takusu_client::ClientError::Api { status: 400, body } => {
            ToolError::InvalidArgs(InvalidArgsError::no_field(body))
        }
        takusu_client::ClientError::Api { status: 404, body } => ToolError::NotFound(body),
        takusu_client::ClientError::Api { status: 409, body } => ToolError::Conflict(body),
        takusu_client::ClientError::Api {
            status: status @ 401..=499,
            body,
        } => ToolError::Other(Box::new(takusu_client::ClientError::Api { status, body })),
        error => ToolError::Other(Box::new(error)),
    }
}

/// Serialized form of a memory row returned by memory tools.
#[derive(Debug, Serialize)]
struct MemoryResponse<'a> {
    id: &'a str,
    kind: &'a MemoryKind,
    key: &'a str,
    content: &'a str,
    subject_type: &'a SubjectType,
    subject_id: &'a str,
    source: &'a MemorySource,
    revision: i64,
    created_at: &'a takusu_types::Timestamp,
    updated_at: &'a takusu_types::Timestamp,
    last_used_at: Option<&'a takusu_types::Timestamp>,
}

impl<'a> From<&'a MemoryRow> for MemoryResponse<'a> {
    fn from(row: &'a MemoryRow) -> Self {
        Self {
            id: &row.id,
            kind: &row.kind,
            key: &row.key,
            content: &row.content,
            subject_type: &row.subject_type,
            subject_id: &row.subject_id,
            source: &row.source,
            revision: row.revision,
            created_at: &row.created_at,
            updated_at: &row.updated_at,
            last_used_at: row.last_used_at.as_ref(),
        }
    }
}

/// Wrapper for tool content that returns `{"results": [...]}`.
#[derive(Debug, Serialize)]
struct ResultsContent<T> {
    results: Vec<T>,
}

fn memory_json(row: &MemoryRow) -> Value {
    serde_json::to_value(MemoryResponse::from(row)).unwrap()
}

#[allow(clippy::too_many_arguments)]
fn make_proposal(
    operation: ChangeOperation,
    target: &Target,
    description: &str,
    before: Option<Value>,
    after: Option<Value>,
    execution_args: Value,
    observed_updated_at: Option<String>,
    inferred_fields: Vec<crate::InferredField>,
    why: Option<String>,
    warnings: Vec<String>,
) -> ToolOutput {
    let proposal = ProposedChange {
        operation,
        target: target.clone(),
        description: description.to_owned(),
        before,
        after,
        arguments: Some(execution_args),
        observed_updated_at,
    };
    ToolOutput {
        content: ProposalContent::new(&proposal.target).to_json_string(),
        why,
        warnings: warnings.clone(),
        proposed_changes: vec![proposal],
        inferred_fields,
        changes: Vec::new(),
        discovered_tools: Vec::new(),
        schedule_dirty: false,
        is_error: false,
    }
}

struct MemoryModule;

impl ToolModule for MemoryModule {
    fn register(&self, registry: &mut ToolRegistry, ctx: &ToolContext) {
        registry.register(Box::new(crate::tool::Typed(MemorySearch {
            client: ctx.client.clone(),
        })));
        registry.register(Box::new(crate::tool::Typed(SimilarTasks {
            client: ctx.client.clone(),
        })));
        registry.register(Box::new(crate::tool::Typed(MemorySave)));
        registry.register(Box::new(crate::tool::Typed(MemoryUpdate {
            client: ctx.client.clone(),
        })));
        registry.register(Box::new(crate::tool::Typed(MemoryDelete {
            client: ctx.client.clone(),
        })));
    }
}

static MEMORY_MODULE: &dyn ToolModule = &MemoryModule;

inventory::submit!(MEMORY_MODULE);

#[derive(Clone)]
struct MemorySearch {
    client: Client,
}

/// Arguments for [`MemorySearch`].
#[derive(Debug, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct MemorySearchArgs {
    /// Search query. Multiple keywords are ANDed. * is a wildcard matching any sequence of characters. Example: 研究室 大学, 研究*大学.
    #[serde(deserialize_with = "deserialize_trimmed_required")]
    #[schemars(with = "String")]
    q: String,
    /// Filter by kind. Values: proper_noun, fact, task_note.
    #[serde(default, deserialize_with = "deserialize_trimmed_optional")]
    #[schemars(with = "Option<String>")]
    kind: Option<String>,
    #[serde(default, deserialize_with = "deserialize_trimmed_optional")]
    #[schemars(with = "Option<String>")]
    subject_type: Option<String>,
    #[serde(default, deserialize_with = "deserialize_trimmed_optional")]
    #[schemars(with = "Option<String>")]
    subject_id: Option<String>,
    /// Maximum results (default 10, max 50).
    #[serde(default)]
    limit: Option<i64>,
}

#[async_trait]
impl TypedTool for MemorySearch {
    type Params = MemorySearchArgs;

    fn name(&self) -> &'static str {
        ToolName::MemorySearch.into()
    }
    fn description(&self) -> &'static str {
        "Search saved memory by key or content. Returns a list of matching memory entries."
    }
    fn exposure(&self) -> ToolExposure {
        ToolExposure::Deferred
    }
    async fn call_typed(&self, args: Self::Params) -> Result<ToolOutput, ToolError> {
        let query = MemoryQuery {
            q: args.q,
            kind: args.kind.map(|s| s.parse::<MemoryKind>()).transpose()?,
            subject_type: args
                .subject_type
                .map(|s| s.parse::<SubjectType>())
                .transpose()?,
            subject_id: args.subject_id,
            limit: args.limit,
        };
        let rows = self
            .client
            .search_memory(&query)
            .await
            .map_err(client_error)?;
        let content: Vec<Value> = rows.iter().map(memory_json).collect();
        Ok(ToolOutput {
            content: serde_json::to_string(&ResultsContent { results: content }).unwrap(),
            ..Default::default()
        })
    }
}

#[derive(Clone)]
struct SimilarTasks {
    client: Client,
}

/// Arguments for [`SimilarTasks`].
#[derive(Debug, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct SimilarTasksArgs {
    /// Title to compare against completed tasks.
    #[serde(deserialize_with = "deserialize_trimmed_required")]
    #[schemars(with = "String")]
    title: String,
    /// Maximum results (default 10, max 50).
    #[serde(default)]
    limit: Option<i64>,
}

#[async_trait]
impl TypedTool for SimilarTasks {
    type Params = SimilarTasksArgs;

    fn name(&self) -> &'static str {
        ToolName::SimilarTasks.into()
    }
    fn description(&self) -> &'static str {
        "Find completed tasks with titles similar to the given title. Useful for estimating durations before creating a task."
    }
    fn exposure(&self) -> ToolExposure {
        ToolExposure::Deferred
    }
    async fn call_typed(&self, args: Self::Params) -> Result<ToolOutput, ToolError> {
        let query = SimilarTaskQuery {
            title: args.title,
            limit: args.limit,
        };
        let rows = self
            .client
            .find_similar_tasks(&query)
            .await
            .map_err(client_error)?;
        Ok(ToolOutput {
            content: serde_json::to_string(&ResultsContent { results: rows }).unwrap(),
            ..Default::default()
        })
    }
}

struct MemorySave;

/// Arguments for [`MemorySave`].
#[derive(Debug, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct MemorySaveArgs {
    /// proper_noun, fact, or task_note.
    #[serde(deserialize_with = "deserialize_trimmed_required")]
    #[schemars(with = "String")]
    kind: String,
    /// Short identifier or term.
    #[serde(deserialize_with = "deserialize_trimmed_required")]
    #[schemars(with = "String")]
    key: String,
    /// Detailed content.
    #[serde(deserialize_with = "deserialize_trimmed_required")]
    #[schemars(with = "String")]
    content: String,
    /// Optional. For task_note set to 'task'.
    #[serde(default, deserialize_with = "deserialize_trimmed_optional")]
    #[schemars(with = "Option<String>")]
    subject_type: Option<String>,
    /// Optional task ID when subject_type is 'task'.
    #[serde(default, deserialize_with = "deserialize_trimmed_optional")]
    #[schemars(with = "Option<String>")]
    subject_id: Option<String>,
    /// Short user-facing reason.
    #[serde(default, deserialize_with = "deserialize_trimmed_optional")]
    #[schemars(with = "Option<String>")]
    why: Option<String>,
    #[serde(default)]
    warnings: Vec<String>,
    #[serde(default)]
    inferred_fields: Vec<InferredField>,
}

#[async_trait]
impl TypedTool for MemorySave {
    type Params = MemorySaveArgs;

    fn name(&self) -> &'static str {
        ToolName::MemorySave.into()
    }
    fn description(&self) -> &'static str {
        "Propose saving a memory (proper noun, fact, or task note). Generates an approval request; does not write immediately."
    }
    fn exposure(&self) -> ToolExposure {
        ToolExposure::Deferred
    }

    fn parameters_schema(&self) -> Value {
        let mut schema = self.default_parameters_schema();
        if let Some(props) = schema.get_mut("properties").and_then(Value::as_object_mut) {
            props.insert(
                "inferred_fields".into(),
                inferred_fields_schema("Fields inferred from user input."),
            );
        }
        schema
    }

    fn validate_args(&self, args: &Self::Params) -> Result<(), InvalidArgsError> {
        let kind: MemoryKind = args.kind.parse().map_err(|e: takusu_types::UnknownLabel| {
            InvalidArgsError::new("kind", e.to_string())
        })?;
        if kind == MemoryKind::TaskNote {
            if args.subject_type.as_deref() != Some("task") {
                return Err(InvalidArgsError::new(
                    "subject_type",
                    "task_note requires subject_type='task'",
                ));
            }
            if args.subject_id.is_none() {
                return Err(InvalidArgsError::new(
                    "subject_id",
                    "task_note requires subject_id",
                ));
            }
        } else if let Some(st) = &args.subject_type {
            st.parse::<SubjectType>()
                .map_err(|e: takusu_types::UnknownLabel| {
                    InvalidArgsError::new("subject_type", e.to_string())
                })?;
        }
        Ok(())
    }

    async fn call_typed(&self, args: Self::Params) -> Result<ToolOutput, ToolError> {
        let kind: MemoryKind = args.kind.parse()?;
        let create = CreateMemory {
            kind,
            key: args.key.clone(),
            content: args.content.clone(),
            subject_type: args
                .subject_type
                .as_ref()
                .map(|s| s.parse::<SubjectType>())
                .transpose()?,
            subject_id: args.subject_id.clone(),
            upsert: false,
        };

        // Build execution_args: serialize the typed args (which includes
        // kind, key, content, subject_type, subject_id) so that
        // execute_proposed_change can deserialize them into CreateMemory.
        let mut execution_args = serde_json::to_value(&args).unwrap_or_default();
        if let Value::Object(ref mut map) = execution_args {
            // Remove fields not needed by CreateMemory.
            map.remove("why");
            map.remove("warnings");
            map.remove("inferred_fields");
        }

        let description = format!("save {kind} memory \"{}\"", args.key);
        let after = json!({
            "id": Value::Null,
            "kind": kind,
            "key": args.key,
            "content": args.content,
            "subject_type": create.subject_type,
            "subject_id": create.subject_id,
            "source": MemorySource::UserConfirmed,
            "revision": 1,
            "created_at": Value::Null,
            "updated_at": Value::Null,
            "last_used_at": Value::Null,
        });

        Ok(make_proposal(
            ChangeOperation::Create,
            &Target::new(TargetKind::Memory, &args.key),
            &description,
            None,
            Some(after),
            execution_args,
            None,
            args.inferred_fields,
            args.why,
            args.warnings,
        ))
    }
}

#[derive(Clone)]
struct MemoryUpdate {
    client: Client,
}

/// Arguments for [`MemoryUpdate`].
#[derive(Debug, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct MemoryUpdateArgs {
    /// Memory ID (from memory_search).
    #[serde(deserialize_with = "deserialize_trimmed_required")]
    #[schemars(with = "String")]
    memory_ref: String,
    observed_revision: i64,
    #[serde(deserialize_with = "deserialize_trimmed_required")]
    #[schemars(with = "String")]
    content: String,
    #[serde(default, deserialize_with = "deserialize_trimmed_optional")]
    #[schemars(with = "Option<String>")]
    why: Option<String>,
    #[serde(default)]
    warnings: Vec<String>,
    #[serde(default)]
    inferred_fields: Vec<InferredField>,
}

#[async_trait]
impl TypedTool for MemoryUpdate {
    type Params = MemoryUpdateArgs;

    fn name(&self) -> &'static str {
        ToolName::MemoryUpdate.into()
    }
    fn description(&self) -> &'static str {
        "Propose updating a memory's content. Generates an approval request; does not write immediately."
    }
    fn exposure(&self) -> ToolExposure {
        ToolExposure::Deferred
    }

    fn parameters_schema(&self) -> Value {
        let mut schema = self.default_parameters_schema();
        if let Some(props) = schema.get_mut("properties").and_then(Value::as_object_mut) {
            props.insert(
                "inferred_fields".into(),
                inferred_fields_schema("Fields inferred from user input."),
            );
        }
        schema
    }

    async fn call_typed(&self, args: Self::Params) -> Result<ToolOutput, ToolError> {
        let current = self
            .client
            .get_memory(&args.memory_ref)
            .await
            .map_err(client_error)?;

        let update = UpdateMemory {
            observed_revision: args.observed_revision,
            content: Some(args.content.clone()),
        };
        let body = serde_json::to_value(&update).map_err(|e| ToolError::Other(Box::new(e)))?;

        // Build execution_args: merge the serialized UpdateMemory body with
        // memory_ref so execute_proposed_change has everything it needs.
        let mut execution_args = serde_json::Map::new();
        if let Value::Object(map) = body {
            execution_args.extend(map);
        }
        execution_args.insert("memory_ref".into(), Value::String(args.memory_ref.clone()));

        let description = format!("update memory \"{}\"", current.key);
        let mut after = memory_json(&current);
        if let Value::Object(ref mut map) = after {
            map.insert("content".into(), Value::String(args.content.clone()));
            map.insert(
                "revision".into(),
                Value::Number(serde_json::Number::from(current.revision + 1)),
            );
        }

        Ok(make_proposal(
            ChangeOperation::Update,
            &Target::new(TargetKind::Memory, &args.memory_ref),
            &description,
            Some(memory_json(&current)),
            Some(after),
            Value::Object(execution_args),
            Some(current.updated_at.to_string()),
            args.inferred_fields,
            args.why,
            args.warnings,
        ))
    }
}

#[derive(Clone)]
struct MemoryDelete {
    client: Client,
}

/// Arguments for [`MemoryDelete`].
#[derive(Debug, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct MemoryDeleteArgs {
    /// Memory ID (from memory_search).
    #[serde(deserialize_with = "deserialize_trimmed_required")]
    #[schemars(with = "String")]
    memory_ref: String,
    observed_revision: i64,
    #[serde(default, deserialize_with = "deserialize_trimmed_optional")]
    #[schemars(with = "Option<String>")]
    why: Option<String>,
    #[serde(default)]
    warnings: Vec<String>,
    #[serde(default)]
    inferred_fields: Vec<InferredField>,
}

#[async_trait]
impl TypedTool for MemoryDelete {
    type Params = MemoryDeleteArgs;

    fn name(&self) -> &'static str {
        ToolName::MemoryDelete.into()
    }
    fn description(&self) -> &'static str {
        "Propose deleting a memory. Generates an approval request; does not write immediately."
    }
    fn exposure(&self) -> ToolExposure {
        ToolExposure::Deferred
    }

    fn parameters_schema(&self) -> Value {
        let mut schema = self.default_parameters_schema();
        if let Some(props) = schema.get_mut("properties").and_then(Value::as_object_mut) {
            props.insert(
                "inferred_fields".into(),
                inferred_fields_schema("Fields inferred from user input."),
            );
        }
        schema
    }

    async fn call_typed(&self, args: Self::Params) -> Result<ToolOutput, ToolError> {
        let current = self
            .client
            .get_memory(&args.memory_ref)
            .await
            .map_err(client_error)?;

        let mut execution_args = serde_json::Map::new();
        execution_args.insert("memory_ref".into(), Value::String(args.memory_ref.clone()));
        execution_args.insert(
            "observed_revision".into(),
            Value::Number(serde_json::Number::from(args.observed_revision)),
        );

        let description = format!("delete memory \"{}\"", current.key);

        Ok(make_proposal(
            ChangeOperation::Delete,
            &Target::new(TargetKind::Memory, &args.memory_ref),
            &description,
            Some(memory_json(&current)),
            None,
            Value::Object(execution_args),
            Some(current.updated_at.to_string()),
            args.inferred_fields,
            args.why,
            args.warnings,
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn memory_json_excludes_internal_normalized_fields() {
        let row = MemoryRow {
            id: "m1".into(),
            kind: MemoryKind::ProperNoun,
            key: "研究室".into(),
            normalized_key: "けんきゅうしつ".into(),
            content: "大学".into(),
            normalized_content: "だいがく".into(),
            subject_type: SubjectType::Empty,
            subject_id: "".into(),
            source: MemorySource::UserConfirmed,
            revision: 1,
            created_at: "2025-01-01T00:00:00Z".parse().unwrap(),
            updated_at: "2025-01-01T00:00:00Z".parse().unwrap(),
            last_used_at: None,
        };
        let value = memory_json(&row);
        assert_eq!(value["id"], "m1");
        assert_eq!(value["key"], "研究室");
        assert!(value.get("normalized_key").is_none());
        assert!(value.get("normalized_content").is_none());
    }

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

    #[test]
    fn memory_save_schema_has_no_upsert() {
        use crate::tool::Tool;
        let save = MemorySave;
        let schema = crate::tool::Typed(save).parameters_schema();
        let props = schema["properties"].as_object().unwrap();
        assert!(!props.contains_key("upsert"));
    }
}
