use async_trait::async_trait;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use takusu_client::Client;

use crate::tools::{ToolContext, ToolModule};
use crate::{
    ChangeOperation, InferredField, InvalidArgsError, ProposalContent, ProposedChange, Target,
    TargetKind, ToolError, ToolExposure, ToolName, ToolOutput, ToolRegistry, TypedTool,
    deserialize_trimmed_optional, deserialize_trimmed_required, inferred_fields_schema,
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

struct SkillsModule;

impl ToolModule for SkillsModule {
    fn register(&self, registry: &mut ToolRegistry, ctx: &ToolContext) {
        registry.register(Box::new(crate::tool::Typed(SkillsList {
            client: ctx.client.clone(),
        })));
        registry.register(Box::new(crate::tool::Typed(SkillsRead {
            client: ctx.client.clone(),
        })));
        registry.register(Box::new(crate::tool::Typed(SkillsProposeAdd {
            client: ctx.client.clone(),
        })));
        registry.register(Box::new(crate::tool::Typed(SkillsProposeEdit {
            client: ctx.client.clone(),
        })));
    }
}

static SKILLS_MODULE: &dyn ToolModule = &SkillsModule;

inventory::submit!(SKILLS_MODULE);

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

/// Serialized form of a skill returned by `skills_list` / `skills_read`.
#[derive(Debug, Serialize)]
struct SkillResponse<'a> {
    slug: &'a str,
    name: &'a str,
    description: &'a str,
    built_in: bool,
    created_at: &'a takusu_util::Timestamp,
    updated_at: &'a takusu_util::Timestamp,
}

impl<'a> From<&'a takusu_client::SkillRow> for SkillResponse<'a> {
    fn from(skill: &'a takusu_client::SkillRow) -> Self {
        Self {
            slug: &skill.slug,
            name: &skill.name,
            description: &skill.description,
            built_in: skill.built_in,
            created_at: &skill.created_at,
            updated_at: &skill.updated_at,
        }
    }
}

fn skill_json(skill: &takusu_client::SkillRow) -> Value {
    serde_json::to_value(SkillResponse::from(skill)).unwrap()
}

struct SkillsList {
    client: Client,
}

/// Arguments for [`SkillsList`] (no parameters).
#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct SkillsListArgs {}

#[async_trait]
impl TypedTool for SkillsList {
    type Params = SkillsListArgs;

    fn name(&self) -> &'static str {
        ToolName::SkillsList.into()
    }

    fn description(&self) -> &'static str {
        "List all available skills (built-in and user-defined)."
    }

    fn exposure(&self) -> ToolExposure {
        ToolExposure::Deferred
    }

    async fn call_typed(&self, _args: Self::Params) -> Result<ToolOutput, ToolError> {
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

/// Arguments for [`SkillsRead`].
#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct SkillsReadArgs {
    #[serde(deserialize_with = "deserialize_trimmed_required")]
    #[schemars(with = "String")]
    slug: String,
}

#[async_trait]
impl TypedTool for SkillsRead {
    type Params = SkillsReadArgs;

    fn name(&self) -> &'static str {
        ToolName::SkillsRead.into()
    }

    fn description(&self) -> &'static str {
        "Read a skill by slug, including its full body."
    }

    fn exposure(&self) -> ToolExposure {
        ToolExposure::Deferred
    }

    fn validate_args(&self, args: &Self::Params) -> Result<(), InvalidArgsError> {
        validate_slug(&args.slug).map_err(ToolError::into_invalid_args)?;
        Ok(())
    }

    async fn call_typed(&self, args: Self::Params) -> Result<ToolOutput, ToolError> {
        let skill = self
            .client
            .get_skill(&args.slug)
            .await
            .map_err(client_error)?;
        Ok(ToolOutput {
            content: serde_json::to_string(&skill).unwrap(),
            ..Default::default()
        })
    }
}

struct SkillsProposeAdd {
    client: Client,
}

/// Arguments for [`SkillsProposeAdd`].
#[derive(Debug, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct SkillsProposeAddArgs {
    /// URL-safe identifier
    #[serde(deserialize_with = "deserialize_trimmed_required")]
    #[schemars(with = "String")]
    slug: String,
    #[serde(deserialize_with = "deserialize_trimmed_required")]
    #[schemars(with = "String")]
    name: String,
    #[serde(deserialize_with = "deserialize_trimmed_required")]
    #[schemars(with = "String")]
    description: String,
    /// Skill instructions (markdown)
    #[serde(deserialize_with = "deserialize_trimmed_required")]
    #[schemars(with = "String")]
    body: String,
    #[serde(default, deserialize_with = "deserialize_trimmed_optional")]
    #[schemars(with = "Option<String>")]
    why: Option<String>,
    #[serde(default)]
    warnings: Vec<String>,
    #[serde(default)]
    inferred_fields: Vec<InferredField>,
}

#[async_trait]
impl TypedTool for SkillsProposeAdd {
    type Params = SkillsProposeAddArgs;

    fn name(&self) -> &'static str {
        ToolName::SkillsProposeAdd.into()
    }

    fn description(&self) -> &'static str {
        "Propose adding a new skill. Requires user approval before it is written."
    }

    fn exposure(&self) -> ToolExposure {
        ToolExposure::Deferred
    }

    fn parameters_schema(&self) -> Value {
        let mut schema = self.default_parameters_schema();
        // Replace inferred_fields with the hand-written schema that carries
        // the custom description.
        if let Some(props) = schema.get_mut("properties").and_then(Value::as_object_mut) {
            props.insert(
                "inferred_fields".into(),
                inferred_fields_schema("Fields inferred from user input."),
            );
        }
        schema
    }

    fn validate_args(&self, args: &Self::Params) -> Result<(), InvalidArgsError> {
        validate_slug(&args.slug).map_err(ToolError::into_invalid_args)?;
        validate_skill_input(
            &args.slug,
            Some(&args.name),
            Some(&args.description),
            Some(&args.body),
            true,
        )
        .map_err(ToolError::into_invalid_args)?;
        Ok(())
    }

    async fn call_typed(&self, args: Self::Params) -> Result<ToolOutput, ToolError> {
        match self.client.get_skill(&args.slug).await {
            Err(takusu_client::ClientError::Api { status: 404, .. }) => {}
            Ok(_) => {
                return Err(ToolError::InvalidArgs(InvalidArgsError::new(
                    "slug",
                    format!("skill {} already exists", args.slug),
                )));
            }
            Err(e) => return Err(ToolError::Other(Box::new(e))),
        }

        let after = json!({
            "slug": args.slug,
            "name": args.name,
            "description": args.description,
            "body": args.body,
        });
        let arguments = serde_json::to_value(&args).unwrap_or_default();
        let proposal = ProposedChange {
            operation: ChangeOperation::Create,
            target: Target::new(TargetKind::Skill, &args.slug),
            description: format!("Create skill {}: {}", args.slug, args.name),
            before: None,
            after: Some(after),
            arguments: Some(arguments),
            observed_updated_at: None,
        };

        Ok(ToolOutput {
            content: ProposalContent::new(&proposal.target).to_json_string(),
            why: args.why,
            warnings: args.warnings,
            proposed_changes: vec![proposal],
            inferred_fields: args.inferred_fields,
            schedule_dirty: false,
            ..Default::default()
        })
    }
}

struct SkillsProposeEdit {
    client: Client,
}

/// Arguments for [`SkillsProposeEdit`].
#[derive(Debug, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct SkillsProposeEditArgs {
    #[serde(deserialize_with = "deserialize_trimmed_required")]
    #[schemars(with = "String")]
    slug: String,
    #[serde(default, deserialize_with = "deserialize_trimmed_optional")]
    #[schemars(with = "Option<String>")]
    name: Option<String>,
    #[serde(default, deserialize_with = "deserialize_trimmed_optional")]
    #[schemars(with = "Option<String>")]
    description: Option<String>,
    #[serde(default, deserialize_with = "deserialize_trimmed_optional")]
    #[schemars(with = "Option<String>")]
    body: Option<String>,
    #[serde(default, deserialize_with = "deserialize_trimmed_optional")]
    #[schemars(with = "Option<String>")]
    why: Option<String>,
    #[serde(default)]
    warnings: Vec<String>,
    #[serde(default)]
    inferred_fields: Vec<InferredField>,
}

#[async_trait]
impl TypedTool for SkillsProposeEdit {
    type Params = SkillsProposeEditArgs;

    fn name(&self) -> &'static str {
        ToolName::SkillsProposeEdit.into()
    }

    fn description(&self) -> &'static str {
        "Propose editing an existing skill. Requires user approval before it is written."
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
        validate_slug(&args.slug).map_err(ToolError::into_invalid_args)?;
        if args.name.is_none() && args.description.is_none() && args.body.is_none() {
            return Err(InvalidArgsError::no_field(
                "at least one of name, description, or body is required",
            ));
        }
        validate_skill_input(
            &args.slug,
            args.name.as_deref(),
            args.description.as_deref(),
            args.body.as_deref(),
            false,
        )
        .map_err(ToolError::into_invalid_args)?;
        Ok(())
    }

    async fn call_typed(&self, args: Self::Params) -> Result<ToolOutput, ToolError> {
        let existing = self
            .client
            .get_skill(&args.slug)
            .await
            .map_err(client_error)?;
        if existing.built_in {
            return Err(ToolError::InvalidArgs(InvalidArgsError::new(
                "slug",
                format!("built-in skill {} cannot be edited", args.slug),
            )));
        }

        let mut before =
            serde_json::to_value(&existing).map_err(|e| ToolError::Other(Box::new(e)))?;
        if let Value::Object(ref mut map) = before {
            map.remove("created_at");
            map.remove("updated_at");
            map.remove("built_in");
        }
        let mut after = before.clone();
        if let Value::Object(ref mut map) = after {
            if let Some(name) = &args.name {
                map.insert("name".to_owned(), Value::String(name.clone()));
            }
            if let Some(description) = &args.description {
                map.insert("description".to_owned(), Value::String(description.clone()));
            }
            if let Some(body) = &args.body {
                map.insert("body".to_owned(), Value::String(body.clone()));
            }
        }

        let arguments = serde_json::to_value(&args).unwrap_or_default();
        let proposal = ProposedChange {
            operation: ChangeOperation::Update,
            target: Target::new(TargetKind::Skill, &args.slug),
            description: format!("Update skill {}", args.slug),
            before: Some(before),
            after: Some(after),
            arguments: Some(arguments),
            observed_updated_at: Some(existing.updated_at.to_string()),
        };

        Ok(ToolOutput {
            content: ProposalContent::new(&proposal.target).to_json_string(),
            why: args.why,
            warnings: args.warnings,
            proposed_changes: vec![proposal],
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
    fn parse_skill_content_reads_front_matter() {
        let content = "+++\nname = \"weekly-review\"\ndescription = \"Run the weekly review\"\n+++\n\nfree-form\n";
        let skill = parse_skill_content("weekly-review", content).unwrap();
        assert_eq!(skill.name, "weekly-review");
        assert_eq!(skill.description, "Run the weekly review");
        assert_eq!(skill.body, "free-form");
        assert!(skill.built_in);
    }
}
