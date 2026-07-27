use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use takusu_client::Client;

use crate::{
    InferredField, InvalidArgsError, ProposedChange, Tool, ToolError, ToolExposure, ToolOutput,
    ToolRegistry,
};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Skill {
    pub slug: String,
    pub name: String,
    pub description: String,
    pub body: String,
    pub built_in: bool,
}

impl Skill {
    fn to_create_skill(&self) -> takusu_client::CreateSkill {
        takusu_client::CreateSkill {
            slug: self.slug.clone(),
            name: self.name.clone(),
            description: self.description.clone(),
            body: self.body.clone(),
            built_in: Some(self.built_in),
        }
    }
}

/// Parse a skill markdown file with TOML front matter.
fn parse_skill_content(slug: &str, content: &str) -> Option<Skill> {
    let body = content.trim_start();
    if !body.starts_with("+++") {
        return None;
    }
    let end = body[3..].find("+++")?;
    let front = &body[3..3 + end];
    let meta: SkillFrontMatter = toml::from_str(front).ok()?;
    let instruction = body[3 + end + 3..].trim().to_string();
    Some(Skill {
        slug: slug.to_string(),
        name: meta.name,
        description: meta.description,
        body: instruction,
        built_in: true,
    })
}

#[derive(Debug, Clone, Deserialize)]
struct SkillFrontMatter {
    name: String,
    description: String,
}

/// Returns built-in skills parsed from the bundled markdown files.
pub fn built_in_skills() -> Vec<Skill> {
    crate::bundled_skills::built_in_skill_contents()
        .iter()
        .filter_map(|(slug, content)| parse_skill_content(slug, content))
        .collect()
}

pub const SKILL_INDEX_HEADER: &str =
    "必要なスキルの詳細は `skills_read` ツールで slug を指定して読み出してください。";

/// Build a fallback skills index from bundled skills.
pub fn built_in_skills_index() -> String {
    let skills = built_in_skills();
    if skills.is_empty() {
        return "（スキルはまだ登録されていません）".into();
    }
    let mut lines = vec![SKILL_INDEX_HEADER.to_string()];
    for s in &skills {
        lines.push(format!(
            "- {} ({}) [built-in]: {}",
            s.name, s.slug, s.description
        ));
    }
    lines.join("\n")
}

/// Synchronize built-in skills into storage so they are synced across devices.
pub async fn sync_built_in_skills(client: &Client) -> Result<(), takusu_client::ClientError> {
    for skill in built_in_skills() {
        let body = skill.to_create_skill();
        // Ignore conflicts: built-in skills may already be present.
        if let Err(e) = client.create_skill(&body).await {
            if matches!(e, takusu_client::ClientError::Api { status: 409, .. }) {
                continue;
            }
            return Err(e);
        }
    }
    Ok(())
}

pub fn register_tools(registry: &mut ToolRegistry, client: Client) {
    registry.register(Box::new(SkillsList {
        client: client.clone(),
    }));
    registry.register(Box::new(SkillsRead {
        client: client.clone(),
    }));
    registry.register(Box::new(SkillsProposeAdd {
        client: client.clone(),
    }));
    registry.register(Box::new(SkillsProposeEdit { client }));
}

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

fn client_error(error: takusu_client::ClientError) -> ToolError {
    match error {
        takusu_client::ClientError::Api {
            status: 400..=499,
            body,
        } => {
            if body.contains("not found") || body.contains("Not found") {
                ToolError::NotFound(body)
            } else {
                ToolError::InvalidArgs(InvalidArgsError::no_field(body))
            }
        }
        error => ToolError::Other(Box::new(error)),
    }
}

fn validate_slug(slug: &str) -> Result<(), ToolError> {
    if slug.is_empty() || slug.len() > 64 {
        return Err(ToolError::InvalidArgs(InvalidArgsError::new(
            "slug",
            "must be 1..64 characters",
        )));
    }
    if slug.starts_with('.') || slug.contains('/') || slug.contains("..") {
        return Err(ToolError::InvalidArgs(InvalidArgsError::new(
            "slug",
            "must not contain path components",
        )));
    }
    if !slug
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
    {
        return Err(ToolError::InvalidArgs(InvalidArgsError::new(
            "slug",
            "must contain only ASCII letters, digits, '-', '_'",
        )));
    }
    Ok(())
}

fn validate_skill_input(
    slug: &str,
    name: Option<&str>,
    description: Option<&str>,
    body: Option<&str>,
    is_create: bool,
) -> Result<(), ToolError> {
    validate_slug(slug)?;
    if let Some(name) = name {
        if name.is_empty() || name.len() > 100 {
            return Err(ToolError::InvalidArgs(InvalidArgsError::new(
                "name",
                "must be 1..100 characters",
            )));
        }
    } else if is_create {
        return Err(ToolError::InvalidArgs(InvalidArgsError::new(
            "name", "missing",
        )));
    }
    if let Some(description) = description
        && description.len() > 500
    {
        return Err(ToolError::InvalidArgs(InvalidArgsError::new(
            "description",
            "must be at most 500 characters",
        )));
    }
    if let Some(body) = body {
        if body.is_empty() || body.len() > 64 * 1024 {
            return Err(ToolError::InvalidArgs(InvalidArgsError::new(
                "body",
                "must be 1..65536 characters",
            )));
        }
    } else if is_create {
        return Err(ToolError::InvalidArgs(InvalidArgsError::new(
            "body", "missing",
        )));
    }
    Ok(())
}

fn skill_json(skill: &takusu_client::SkillRow) -> Value {
    json!({
        "slug": skill.slug,
        "name": skill.name,
        "description": skill.description,
        "built_in": skill.built_in,
        "created_at": skill.created_at,
        "updated_at": skill.updated_at,
    })
}

struct SkillsList {
    client: Client,
}

#[async_trait]
impl Tool for SkillsList {
    fn name(&self) -> &'static str {
        "skills_list"
    }

    fn description(&self) -> &'static str {
        "List all available skills (built-in and user-defined)."
    }

    fn exposure(&self) -> ToolExposure {
        ToolExposure::Deferred
    }

    fn parameters_schema(&self) -> Value {
        json!({"type":"object","properties":{},"additionalProperties":false})
    }

    async fn call(&self, args: Value) -> Result<ToolOutput, ToolError> {
        let _ = object(args)?;
        let skills = self.client.list_skills().await.map_err(client_error)?;
        let content = skills.iter().map(skill_json).collect::<Vec<_>>();
        Ok(ToolOutput {
            content: serde_json::to_string(&content).unwrap(),
            ..Default::default()
        })
    }
}

struct SkillsRead {
    client: Client,
}

#[async_trait]
impl Tool for SkillsRead {
    fn name(&self) -> &'static str {
        "skills_read"
    }

    fn description(&self) -> &'static str {
        "Read a skill by slug, including its full body."
    }

    fn exposure(&self) -> ToolExposure {
        ToolExposure::Deferred
    }

    fn parameters_schema(&self) -> Value {
        json!({"type":"object","properties":{"slug":{"type":"string"}},"required":["slug"],"additionalProperties":false})
    }

    async fn call(&self, args: Value) -> Result<ToolOutput, ToolError> {
        let slug = required_string(&object(args)?, "slug")?;
        validate_slug(&slug)?;
        let skill = self.client.get_skill(&slug).await.map_err(client_error)?;
        Ok(ToolOutput {
            content: serde_json::to_string(&skill).unwrap(),
            ..Default::default()
        })
    }
}

struct SkillsProposeAdd {
    client: Client,
}

#[async_trait]
impl Tool for SkillsProposeAdd {
    fn name(&self) -> &'static str {
        "skills_propose_add"
    }

    fn description(&self) -> &'static str {
        "Propose adding a new skill. Requires user approval before it is written."
    }

    fn exposure(&self) -> ToolExposure {
        ToolExposure::Deferred
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type":"object",
            "properties":{
                "slug":{"type":"string","description":"URL-safe identifier"},
                "name":{"type":"string"},
                "description":{"type":"string"},
                "body":{"type":"string","description":"Skill instructions (markdown)"},
                "why":{"type":"string"},
                "warnings":{"type":"array","items":{"type":"string"}},
                "inferred_fields": crate::inferred_fields_schema("Fields inferred from user input.")
            },
            "required":["slug","name","description","body"],
            "additionalProperties":false
        })
    }

    async fn call(&self, args: Value) -> Result<ToolOutput, ToolError> {
        let args = object(args)?;
        let slug = required_string(&args, "slug")?;
        let name = required_string(&args, "name")?;
        let description = required_string(&args, "description")?;
        let body = required_string(&args, "body")?;
        validate_skill_input(&slug, Some(&name), Some(&description), Some(&body), true)?;

        match self.client.get_skill(&slug).await {
            Err(takusu_client::ClientError::Api { status: 404, .. }) => {}
            Ok(_) => {
                return Err(ToolError::InvalidArgs(InvalidArgsError::new(
                    "slug",
                    format!("skill {slug} already exists"),
                )));
            }
            Err(e) => return Err(ToolError::Other(Box::new(e))),
        }

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

        let inferred = args
            .get("inferred_fields")
            .cloned()
            .unwrap_or_else(|| json!([]));
        let inferred_fields: Vec<InferredField> =
            serde_json::from_value(inferred).map_err(|error| {
                ToolError::InvalidArgs(InvalidArgsError::new(
                    "inferred_fields",
                    format!("invalid: {error}"),
                ))
            })?;

        let after = json!({
            "slug": slug,
            "name": name,
            "description": description,
            "body": body,
        });
        let proposal = ProposedChange {
            operation: "create".to_owned(),
            target_label: format!("skill {slug}"),
            description: format!("Create skill {slug}: {name}"),
            before: None,
            after: Some(after),
            arguments: Some(Value::Object(args)),
            observed_updated_at: None,
        };

        Ok(ToolOutput {
            content: serde_json::to_string(&json!({
                "approval_required": true,
                "target": proposal.target_label,
            }))
            .unwrap(),
            why,
            warnings,
            proposed_changes: vec![proposal],
            inferred_fields,
            schedule_dirty: false,
            ..Default::default()
        })
    }
}

struct SkillsProposeEdit {
    client: Client,
}

#[async_trait]
impl Tool for SkillsProposeEdit {
    fn name(&self) -> &'static str {
        "skills_propose_edit"
    }

    fn description(&self) -> &'static str {
        "Propose editing an existing skill. Requires user approval before it is written."
    }

    fn exposure(&self) -> ToolExposure {
        ToolExposure::Deferred
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type":"object",
            "properties":{
                "slug":{"type":"string"},
                "name":{"type":"string"},
                "description":{"type":"string"},
                "body":{"type":"string"},
                "why":{"type":"string"},
                "warnings":{"type":"array","items":{"type":"string"}},
                "inferred_fields": crate::inferred_fields_schema("Fields inferred from user input.")
            },
            "required":["slug"],
            "additionalProperties":false
        })
    }

    async fn call(&self, args: Value) -> Result<ToolOutput, ToolError> {
        let args = object(args)?;
        let slug = required_string(&args, "slug")?;
        validate_slug(&slug)?;

        let existing = self.client.get_skill(&slug).await.map_err(client_error)?;
        if existing.built_in {
            return Err(ToolError::InvalidArgs(InvalidArgsError::new(
                "slug",
                format!("built-in skill {slug} cannot be edited"),
            )));
        }

        let name = optional_string(&args, "name")?;
        let description = optional_string(&args, "description")?;
        let body = optional_string(&args, "body")?;
        if name.is_none() && description.is_none() && body.is_none() {
            return Err(ToolError::InvalidArgs(InvalidArgsError::no_field(
                "at least one of name, description, or body is required",
            )));
        }
        validate_skill_input(
            &slug,
            name.as_deref(),
            description.as_deref(),
            body.as_deref(),
            false,
        )?;

        let mut before =
            serde_json::to_value(&existing).map_err(|e| ToolError::Other(Box::new(e)))?;
        if let Value::Object(ref mut map) = before {
            map.remove("created_at");
            map.remove("updated_at");
            map.remove("built_in");
        }
        let mut after = before.clone();
        if let Value::Object(ref mut map) = after {
            if let Some(name) = name {
                map.insert("name".to_owned(), Value::String(name));
            }
            if let Some(description) = description {
                map.insert("description".to_owned(), Value::String(description));
            }
            if let Some(body) = body {
                map.insert("body".to_owned(), Value::String(body));
            }
        }
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

        let inferred = args
            .get("inferred_fields")
            .cloned()
            .unwrap_or_else(|| json!([]));
        let inferred_fields: Vec<InferredField> =
            serde_json::from_value(inferred).map_err(|error| {
                ToolError::InvalidArgs(InvalidArgsError::new(
                    "inferred_fields",
                    format!("invalid: {error}"),
                ))
            })?;

        let proposal = ProposedChange {
            operation: "update".to_owned(),
            target_label: format!("skill {slug}"),
            description: format!("Update skill {slug}"),
            before: Some(before),
            after: Some(after),
            arguments: Some(Value::Object(args)),
            observed_updated_at: Some(existing.updated_at),
        };

        Ok(ToolOutput {
            content: serde_json::to_string(&json!({
                "approval_required": true,
                "target": proposal.target_label,
            }))
            .unwrap(),
            why,
            warnings,
            proposed_changes: vec![proposal],
            inferred_fields,
            schedule_dirty: false,
            ..Default::default()
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_skill_content_reads_front_matter() {
        let content = "+++\nname = \"weekly-review\"\ndescription = \"Run the weekly review\"\n+++\n\nfree-form\n";
        let skill = parse_skill_content("weekly-review", content).unwrap();
        assert_eq!(skill.name, "weekly-review");
        assert_eq!(skill.description, "Run the weekly review");
        assert_eq!(skill.body, "free-form");
        assert!(skill.built_in);
    }
}
