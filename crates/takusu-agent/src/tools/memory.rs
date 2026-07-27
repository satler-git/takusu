use async_trait::async_trait;
use serde_json::{Value, json};
use takusu_client::{Client, CreateMemory, MemoryQuery, MemoryRow, SimilarTaskQuery, UpdateMemory};
use takusu_util::{MemoryKind, SubjectType};

use crate::tools::takusu::required_i64;
use crate::{
    ChangeOperation, InvalidArgsError, ProposedChange, Target, TargetKind, Tool, ToolError,
    ToolExposure, ToolOutput,
};

fn object(args: Value) -> Result<serde_json::Map<String, Value>, ToolError> {
    args.as_object()
        .cloned()
        .ok_or_else(|| ToolError::InvalidArgs(InvalidArgsError::new("args", "must be an object")))
}

fn required_string(args: &serde_json::Map<String, Value>, name: &str) -> Result<String, ToolError> {
    args.get(name)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
        .ok_or_else(|| ToolError::InvalidArgs(InvalidArgsError::new(name, "missing or empty")))
}

fn optional_string(
    args: &serde_json::Map<String, Value>,
    name: &str,
) -> Result<Option<String>, ToolError> {
    match args.get(name) {
        None | Some(Value::Null) => Ok(None),
        Some(value) => value
            .as_str()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(|value| Some(value.to_owned()))
            .ok_or_else(|| ToolError::InvalidArgs(InvalidArgsError::new(name, "must be a string"))),
    }
}

fn optional_i64(
    args: &serde_json::Map<String, Value>,
    name: &str,
) -> Result<Option<i64>, ToolError> {
    match args.get(name) {
        None | Some(Value::Null) => Ok(None),
        Some(value) => value.as_i64().map(Some).ok_or_else(|| {
            ToolError::InvalidArgs(InvalidArgsError::new(name, "must be an integer"))
        }),
    }
}

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

fn memory_json(row: &MemoryRow) -> Value {
    json!({
        "id": row.id,
        "kind": row.kind,
        "key": row.key,
        "content": row.content,
        "subject_type": row.subject_type,
        "subject_id": row.subject_id,
        "source": row.source,
        "revision": row.revision,
        "created_at": row.created_at,
        "updated_at": row.updated_at,
        "last_used_at": row.last_used_at,
    })
}

#[allow(clippy::too_many_arguments)]
fn make_proposal(
    operation: ChangeOperation,
    target: &Target,
    description: &str,
    before: Option<Value>,
    after: Option<Value>,
    execution_args: serde_json::Map<String, Value>,
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
        arguments: Some(Value::Object(execution_args)),
        observed_updated_at,
    };
    ToolOutput {
        content: serde_json::to_string(&json!({
            "approval_required": true,
            "target": proposal.target.to_string(),
        }))
        .unwrap(),
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

pub fn register_tools(registry: &mut crate::ToolRegistry, client: Client) {
    registry.register(Box::new(MemorySearch {
        client: client.clone(),
    }));
    registry.register(Box::new(SimilarTasks {
        client: client.clone(),
    }));
    registry.register(Box::new(MemorySave));
    registry.register(Box::new(MemoryUpdate {
        client: client.clone(),
    }));
    registry.register(Box::new(MemoryDelete { client }));
}

#[derive(Clone)]
struct MemorySearch {
    client: Client,
}

#[async_trait]
impl Tool for MemorySearch {
    fn name(&self) -> &'static str {
        "memory_search"
    }
    fn description(&self) -> &'static str {
        "Search saved memory by key or content. Returns a list of matching memory entries."
    }
    fn exposure(&self) -> ToolExposure {
        ToolExposure::Deferred
    }
    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "q": {"type": "string", "description": "Search query. Multiple keywords are ANDed. * is a wildcard matching any sequence of characters. Example: 研究室 大学, 研究*大学."},
                "kind": {"type": "string", "description": "Filter by kind. Values: proper_noun, fact, task_note."},
                "subject_type": {"type": "string"},
                "subject_id": {"type": "string"},
                "limit": {"type": "integer", "description": "Maximum results (default 10, max 50)."},
            },
            "required": ["q"],
            "additionalProperties": false,
        })
    }
    async fn call(&self, args: Value) -> Result<ToolOutput, ToolError> {
        let args = object(args)?;
        let query = MemoryQuery {
            q: required_string(&args, "q")?,
            kind: optional_string(&args, "kind")?
                .map(|s| s.parse::<MemoryKind>())
                .transpose()?,
            subject_type: optional_string(&args, "subject_type")?
                .map(|s| s.parse::<SubjectType>())
                .transpose()?,
            subject_id: optional_string(&args, "subject_id")?,
            limit: optional_i64(&args, "limit")?,
        };
        let rows = self
            .client
            .search_memory(&query)
            .await
            .map_err(client_error)?;
        let content: Vec<Value> = rows.iter().map(memory_json).collect();
        Ok(ToolOutput {
            content: serde_json::to_string(&json!({"results": content})).unwrap(),
            ..Default::default()
        })
    }
}

#[derive(Clone)]
struct SimilarTasks {
    client: Client,
}

#[async_trait]
impl Tool for SimilarTasks {
    fn name(&self) -> &'static str {
        "similar_tasks"
    }
    fn description(&self) -> &'static str {
        "Find completed tasks with titles similar to the given title. Useful for estimating durations before creating a task."
    }
    fn exposure(&self) -> ToolExposure {
        ToolExposure::Deferred
    }
    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "title": {"type": "string", "description": "Title to compare against completed tasks."},
                "limit": {"type": "integer", "description": "Maximum results (default 10, max 50)."},
            },
            "required": ["title"],
            "additionalProperties": false,
        })
    }
    async fn call(&self, args: Value) -> Result<ToolOutput, ToolError> {
        let args = object(args)?;
        let query = SimilarTaskQuery {
            title: required_string(&args, "title")?,
            limit: optional_i64(&args, "limit")?,
        };
        let rows = self
            .client
            .find_similar_tasks(&query)
            .await
            .map_err(client_error)?;
        Ok(ToolOutput {
            content: serde_json::to_string(&json!({"results": rows})).unwrap(),
            ..Default::default()
        })
    }
}

struct MemorySave;

#[async_trait]
impl Tool for MemorySave {
    fn name(&self) -> &'static str {
        "memory_save"
    }
    fn description(&self) -> &'static str {
        "Propose saving a memory (proper noun, fact, or task note). Generates an approval request; does not write immediately."
    }
    fn exposure(&self) -> ToolExposure {
        ToolExposure::Deferred
    }
    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "kind": {"type": "string", "description": "proper_noun, fact, or task_note."},
                "key": {"type": "string", "description": "Short identifier or term."},
                "content": {"type": "string", "description": "Detailed content."},
                "subject_type": {"type": "string", "description": "Optional. For task_note set to 'task'."},
                "subject_id": {"type": "string", "description": "Optional task ID when subject_type is 'task'."},
                "why": {"type": "string", "description": "Short user-facing reason."},
                "warnings": {"type": "array", "items": {"type": "string"}},
                "inferred_fields": crate::inferred_fields_schema("Fields inferred from user input."),
            },
            "required": ["kind", "key", "content"],
            "additionalProperties": false,
        })
    }
    async fn call(&self, args: Value) -> Result<ToolOutput, ToolError> {
        let args = object(args)?;
        let kind: MemoryKind = required_string(&args, "kind")?.parse()?;
        let key = required_string(&args, "key")?;

        let subject_type = optional_string(&args, "subject_type")?
            .map(|s| s.parse::<SubjectType>())
            .transpose()?;
        let subject_id = optional_string(&args, "subject_id")?;
        if kind == MemoryKind::TaskNote {
            if subject_type != Some(SubjectType::Task) {
                return Err(ToolError::InvalidArgs(InvalidArgsError::new(
                    "subject_type",
                    "task_note requires subject_type='task'",
                )));
            }
            if subject_id.is_none() {
                return Err(ToolError::InvalidArgs(InvalidArgsError::new(
                    "subject_id",
                    "task_note requires subject_id",
                )));
            }
        }

        let mut execution_args = args.clone();
        let create = CreateMemory {
            kind,
            key: key.clone(),
            content: required_string(&args, "content")?,
            subject_type,
            subject_id,
            upsert: false,
        };
        let mut body = serde_json::to_value(&create).map_err(|e| ToolError::Other(Box::new(e)))?;
        if let Value::Object(ref mut map) = body {
            map.remove("upsert");
        }
        if let Value::Object(map) = body {
            execution_args.extend(map);
        }

        let description = format!("save {kind} memory \"{key}\"");
        let why = optional_string(&args, "why")?;
        let warnings = args
            .get("warnings")
            .and_then(Value::as_array)
            .map(|values| {
                values
                    .iter()
                    .filter_map(Value::as_str)
                    .map(ToOwned::to_owned)
                    .collect()
            })
            .unwrap_or_default();
        let inferred_fields = args
            .get("inferred_fields")
            .cloned()
            .unwrap_or_else(|| json!([]));
        let inferred_fields = serde_json::from_value::<Vec<crate::InferredField>>(inferred_fields)
            .map_err(|e| {
                ToolError::InvalidArgs(InvalidArgsError::new(
                    "inferred_fields",
                    format!("invalid: {e}"),
                ))
            })?;

        let after = json!({
            "id": Value::Null,
            "kind": kind,
            "key": key,
            "content": create.content,
            "subject_type": create.subject_type,
            "subject_id": create.subject_id,
            "source": "user_confirmed",
            "revision": 1,
            "created_at": Value::Null,
            "updated_at": Value::Null,
            "last_used_at": Value::Null,
        });

        Ok(make_proposal(
            ChangeOperation::Create,
            &Target::new(TargetKind::Memory, key.as_str()),
            &description,
            None,
            Some(after),
            execution_args,
            None,
            inferred_fields,
            why,
            warnings,
        ))
    }
}

#[derive(Clone)]
struct MemoryUpdate {
    client: Client,
}

#[async_trait]
impl Tool for MemoryUpdate {
    fn name(&self) -> &'static str {
        "memory_update"
    }
    fn description(&self) -> &'static str {
        "Propose updating a memory's content. Generates an approval request; does not write immediately."
    }
    fn exposure(&self) -> ToolExposure {
        ToolExposure::Deferred
    }
    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "memory_ref": {"type": "string", "description": "Memory ID (from memory_search)."},
                "observed_revision": {"type": "integer"},
                "content": {"type": "string"},
                "why": {"type": "string"},
                "warnings": {"type": "array", "items": {"type": "string"}},
                "inferred_fields": crate::inferred_fields_schema("Fields inferred from user input."),
            },
            "required": ["memory_ref", "observed_revision", "content"],
            "additionalProperties": false,
        })
    }
    async fn call(&self, args: Value) -> Result<ToolOutput, ToolError> {
        let args = object(args)?;
        let memory_ref = required_string(&args, "memory_ref")?;

        let current = self
            .client
            .get_memory(&memory_ref)
            .await
            .map_err(client_error)?;

        let mut execution_args = args.clone();
        let update = UpdateMemory {
            observed_revision: required_i64(&args, "observed_revision")?,
            content: Some(required_string(&args, "content")?),
        };
        let body = serde_json::to_value(&update).map_err(|e| ToolError::Other(Box::new(e)))?;
        if let Value::Object(map) = body {
            execution_args.extend(map);
        }
        execution_args.insert("memory_ref".into(), Value::String(memory_ref.clone()));

        let description = format!("update memory \"{}\"", current.key);
        let why = optional_string(&args, "why")?;
        let warnings = args
            .get("warnings")
            .and_then(Value::as_array)
            .map(|values| {
                values
                    .iter()
                    .filter_map(Value::as_str)
                    .map(ToOwned::to_owned)
                    .collect()
            })
            .unwrap_or_default();
        let inferred_fields = args
            .get("inferred_fields")
            .cloned()
            .unwrap_or_else(|| json!([]));
        let inferred_fields = serde_json::from_value::<Vec<crate::InferredField>>(inferred_fields)
            .map_err(|e| {
                ToolError::InvalidArgs(InvalidArgsError::new(
                    "inferred_fields",
                    format!("invalid: {e}"),
                ))
            })?;

        let mut after = memory_json(&current);
        if let Value::Object(ref mut map) = after {
            map.insert(
                "content".into(),
                Value::String(update.content.clone().unwrap_or_default()),
            );
            map.insert(
                "revision".into(),
                Value::Number(serde_json::Number::from(current.revision + 1)),
            );
        }

        Ok(make_proposal(
            ChangeOperation::Update,
            &Target::new(TargetKind::Memory, memory_ref.as_str()),
            &description,
            Some(memory_json(&current)),
            Some(after),
            execution_args,
            Some(current.updated_at),
            inferred_fields,
            why,
            warnings,
        ))
    }
}

#[derive(Clone)]
struct MemoryDelete {
    client: Client,
}

#[async_trait]
impl Tool for MemoryDelete {
    fn name(&self) -> &'static str {
        "memory_delete"
    }
    fn description(&self) -> &'static str {
        "Propose deleting a memory. Generates an approval request; does not write immediately."
    }
    fn exposure(&self) -> ToolExposure {
        ToolExposure::Deferred
    }
    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "memory_ref": {"type": "string", "description": "Memory ID (from memory_search)."},
                "observed_revision": {"type": "integer"},
                "why": {"type": "string"},
                "warnings": {"type": "array", "items": {"type": "string"}},
                "inferred_fields": crate::inferred_fields_schema("Fields inferred from user input."),
            },
            "required": ["memory_ref", "observed_revision"],
            "additionalProperties": false,
        })
    }
    async fn call(&self, args: Value) -> Result<ToolOutput, ToolError> {
        let args = object(args)?;
        let memory_ref = required_string(&args, "memory_ref")?;

        let current = self
            .client
            .get_memory(&memory_ref)
            .await
            .map_err(client_error)?;

        let mut execution_args = args.clone();
        execution_args.insert("memory_ref".into(), Value::String(memory_ref.clone()));

        let description = format!("delete memory \"{}\"", current.key);
        let why = optional_string(&args, "why")?;
        let warnings = args
            .get("warnings")
            .and_then(Value::as_array)
            .map(|values| {
                values
                    .iter()
                    .filter_map(Value::as_str)
                    .map(ToOwned::to_owned)
                    .collect()
            })
            .unwrap_or_default();
        let inferred_fields = args
            .get("inferred_fields")
            .cloned()
            .unwrap_or_else(|| json!([]));
        let inferred_fields = serde_json::from_value::<Vec<crate::InferredField>>(inferred_fields)
            .map_err(|e| {
                ToolError::InvalidArgs(InvalidArgsError::new(
                    "inferred_fields",
                    format!("invalid: {e}"),
                ))
            })?;

        Ok(make_proposal(
            ChangeOperation::Delete,
            &Target::new(TargetKind::Memory, memory_ref.as_str()),
            &description,
            Some(memory_json(&current)),
            None,
            execution_args,
            Some(current.updated_at),
            inferred_fields,
            why,
            warnings,
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
            content: "大学".into(),
            subject_type: SubjectType::Empty,
            subject_id: "".into(),
            source: "user_confirmed".into(),
            revision: 1,
            created_at: "2025-01-01T00:00:00Z".into(),
            updated_at: "2025-01-01T00:00:00Z".into(),
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
        let save = MemorySave;
        let schema = save.parameters_schema();
        let props = schema["properties"].as_object().unwrap();
        assert!(!props.contains_key("upsert"));
    }
}
