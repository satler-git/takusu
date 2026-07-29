use std::collections::{HashMap, VecDeque};
use std::sync::Arc;

use rmcp::{
    ErrorData as McpError, ServerHandler, ServiceExt,
    model::{
        CallToolRequestMethod, CallToolRequestParams, CallToolResult, Content, Implementation,
        ListToolsResult, PaginatedRequestParams, ProtocolVersion, ServerCapabilities, ServerInfo,
        Tool,
    },
    service::{RequestContext, RoleServer},
    transport::stdio,
};
use serde_json::{Value, json};
use takusu_agent::{AgentConfig, AgentError, AgentSession, InvalidArgsError};
use takusu_client::Client;
use takusu_local_lib::app::TakusuApp;
use takusu_local_lib::error::AppError;
use tokio::sync::Mutex;

use crate::server::{LocalServer, start_in_process};

const MAX_SESSIONS: usize = 64;

pub async fn run(app: Arc<TakusuApp>) -> Result<(), AppError> {
    let local_server = start_in_process(app).await?;
    let mut config = AgentConfig::load()
        .map_err(|e| AppError::Internal(format!("failed to load agent config: {e}")))?;
    config.server.url = local_server.url.clone();
    config.server.token = local_server.token.clone();

    let client = Client::new(&config.server.url, &config.server.token);
    let server = McpServer {
        config,
        client,
        sessions: Mutex::new(SessionStore::new()),
        _local_server: local_server,
    };

    let service = server
        .serve(stdio())
        .await
        .map_err(|e| AppError::Internal(format!("MCP server init error: {e}")))?;
    service
        .waiting()
        .await
        .map_err(|e| AppError::Internal(format!("MCP server error: {e}")))?;
    Ok(())
}

struct SessionStore<V> {
    map: HashMap<String, Arc<V>>,
    order: VecDeque<String>,
}

impl<V> SessionStore<V> {
    fn new() -> Self {
        Self {
            map: HashMap::new(),
            order: VecDeque::new(),
        }
    }

    fn get(&mut self, id: &str) -> Option<Arc<V>> {
        let session = self.map.get(id).cloned()?;
        self.touch(id);
        Some(session)
    }

    fn insert(&mut self, id: String, session: Arc<V>) -> Arc<V> {
        if self.map.len() >= MAX_SESSIONS && !self.map.contains_key(&id) {
            while let Some(oldest) = self.order.pop_front() {
                if self.map.remove(&oldest).is_some() {
                    break;
                }
            }
        }
        self.map.insert(id.clone(), session.clone());
        self.touch(&id);
        session
    }

    fn remove(&mut self, id: &str) -> bool {
        if self.map.remove(id).is_some() {
            self.order.retain(|k| k != id);
            true
        } else {
            false
        }
    }

    fn touch(&mut self, id: &str) {
        self.order.retain(|k| k != id);
        self.order.push_back(id.to_string());
    }
}

struct McpServer {
    config: AgentConfig,
    client: Client,
    sessions: Mutex<SessionStore<AgentSession>>,
    _local_server: LocalServer,
}

impl McpServer {
    async fn create_session(&self) -> Result<(String, Arc<AgentSession>), AgentError> {
        let session = takusu_agent::runner::build_session(&self.config, self.client.clone())?;
        let session = Arc::new(session);
        let id = session.session_id().to_string();
        let mut sessions = self.sessions.lock().await;
        let session = sessions.insert(id.clone(), session);
        Ok((id, session))
    }

    async fn get_session(&self, id: &str) -> Result<Arc<AgentSession>, AgentError> {
        self.sessions.lock().await.get(id).ok_or_else(|| {
            AgentError::Tool(takusu_agent::ToolError::InvalidArgs(InvalidArgsError::new(
                "session_id",
                format!("not found: {id}"),
            )))
        })
    }

    async fn remove_session(&self, id: &str) -> bool {
        self.sessions.lock().await.remove(id)
    }
}

impl ServerHandler for McpServer {
    fn get_info(&self) -> ServerInfo {
        ServerInfo::new(ServerCapabilities::builder().enable_tools().build())
            .with_protocol_version(ProtocolVersion::V_2024_11_05)
            .with_server_info(Implementation::from_build_env())
            .with_instructions(
                "Takusu agent MCP server. Tools: takusu_create_session, takusu_agent_run, \
             takusu_get_approval, takusu_resolve_approval, takusu_delete_session",
            )
    }

    async fn list_tools(
        &self,
        _request: Option<PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> Result<ListToolsResult, McpError> {
        let tools = vec![
            tool(
                "takusu_create_session",
                "Create a new agent session and return its ID.",
                json!({"type": "object", "properties": {}, "required": []}),
            ),
            tool(
                "takusu_agent_run",
                "Run one agent turn. Creates a new session if session_id is omitted.",
                json!({
                    "type": "object",
                    "properties": {
                        "session_id": {"type": "string", "description": "Existing session ID"},
                        "text": {"type": "string", "description": "User message"}
                    },
                    "required": ["text"]
                }),
            ),
            tool(
                "takusu_get_approval",
                "Get the pending approval in a session, if any.",
                json!({
                    "type": "object",
                    "properties": {"session_id": {"type": "string"}},
                    "required": ["session_id"]
                }),
            ),
            tool(
                "takusu_resolve_approval",
                "Approve or deny a pending approval in a session.",
                json!({
                    "type": "object",
                    "properties": {
                        "session_id": {"type": "string"},
                        "approval_id": {"type": "string"},
                        "approve": {"type": "boolean"}
                    },
                    "required": ["session_id", "approval_id", "approve"]
                }),
            ),
            tool(
                "takusu_delete_session",
                "Delete an agent session.",
                json!({
                    "type": "object",
                    "properties": {"session_id": {"type": "string"}},
                    "required": ["session_id"]
                }),
            ),
        ];
        Ok(ListToolsResult {
            tools,
            next_cursor: None,
            meta: None,
        })
    }

    async fn call_tool(
        &self,
        request: CallToolRequestParams,
        _context: RequestContext<RoleServer>,
    ) -> Result<CallToolResult, McpError> {
        let args = request.arguments.unwrap_or_default();
        let args = serde_json::Value::Object(args);

        match request.name.as_ref() {
            "takusu_create_session" => match self.create_session().await {
                Ok((id, _)) => Ok(json_result(json!({"session_id": id}))),
                Err(e) => Err(map_agent_error(e)),
            },

            "takusu_agent_run" => {
                let text = required_string(&args, "text")?;
                let session_id = optional_string(&args, "session_id");

                let (session_id, session) = if let Some(id) = session_id {
                    let session = self.get_session(&id).await.map_err(map_agent_error)?;
                    (id, session)
                } else {
                    self.create_session().await.map_err(map_agent_error)?
                };

                match session.run_turn(&text).await {
                    Ok(result) => {
                        let value = json!({
                            "session_id": session_id,
                            "text": result.text,
                            "approval_request": result.approval_request,
                            "changes": result.changes,
                            "schedule_dirty": result.schedule_dirty,
                        });
                        Ok(json_result(value))
                    }
                    Err(e) => Err(map_agent_error(e)),
                }
            }

            "takusu_get_approval" => {
                let session_id = required_string(&args, "session_id")?;
                let session = self
                    .get_session(&session_id)
                    .await
                    .map_err(map_agent_error)?;
                match session.pending_approval() {
                    Some(approval) => Ok(json_result(json!({"approval": approval}))),
                    None => Ok(json_result(json!({"approval": null}))),
                }
            }

            "takusu_resolve_approval" => {
                let session_id = required_string(&args, "session_id")?;
                let approval_id = required_string(&args, "approval_id")?;
                let approve = required_bool(&args, "approve")?;
                let session = self
                    .get_session(&session_id)
                    .await
                    .map_err(map_agent_error)?;
                match session.resolve_approval(&approval_id, approve).await {
                    Ok(result) => Ok(json_result(json!({
                        "approved": result.approved,
                        "changes": result.changes,
                        "schedule_dirty": result.schedule_dirty,
                    }))),
                    Err(e) => Err(map_agent_error(e)),
                }
            }

            "takusu_delete_session" => {
                let session_id = required_string(&args, "session_id")?;
                let found = self.remove_session(&session_id).await;
                Ok(json_result(json!({"deleted": found})))
            }

            _ => Err(McpError::method_not_found::<CallToolRequestMethod>()),
        }
    }
}

fn tool(name: &'static str, description: &'static str, schema: Value) -> Tool {
    Tool::new(name, description, Arc::new(rmcp::model::object(schema)))
}

fn json_result(value: Value) -> CallToolResult {
    CallToolResult::success(vec![Content::text(
        serde_json::to_string(&value).unwrap_or_else(|_| "{}".to_string()),
    )])
}

fn required_string(value: &Value, key: &str) -> Result<String, McpError> {
    value
        .get(key)
        .and_then(|v| v.as_str())
        .map(|s| s.to_owned())
        .ok_or_else(|| McpError::invalid_params(format!("missing or invalid {key}"), None))
}

fn optional_string(value: &Value, key: &str) -> Option<String> {
    value
        .get(key)
        .and_then(|v| v.as_str())
        .map(|s| s.to_owned())
}

fn required_bool(value: &Value, key: &str) -> Result<bool, McpError> {
    value
        .get(key)
        .and_then(|v| v.as_bool())
        .ok_or_else(|| McpError::invalid_params(format!("missing or invalid {key}"), None))
}

fn map_agent_error(e: AgentError) -> McpError {
    match e {
        AgentError::Tool(t) if t.is_recoverable() => McpError::invalid_params(t.to_string(), None),
        AgentError::Tool(t) => McpError::internal_error(t.to_string(), None),
        AgentError::Client(e) => McpError::internal_error(e.to_string(), None),
        AgentError::Llm(e) => McpError::internal_error(e.to_string(), None),
        AgentError::TooManyToolCalls => {
            McpError::internal_error("too many tool calls".to_string(), None)
        }
        #[cfg(feature = "audio-device")]
        AgentError::Audio(e) => McpError::internal_error(e.to_string(), None),
        AgentError::Lock(e) => McpError::internal_error(e, None),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn session_store_get_and_remove() {
        let mut store = SessionStore::<()>::new();
        assert!(store.get("missing").is_none());
        let s = Arc::new(());
        store.insert("a".to_string(), s.clone());
        assert!(store.get("a").is_some());
        assert!(store.remove("a"));
        assert!(!store.remove("a"));
    }

    #[test]
    fn session_store_evicts_oldest_when_at_capacity() {
        let mut store = SessionStore::<()>::new();
        for i in 0..MAX_SESSIONS {
            store.insert(format!("{i}"), Arc::new(()));
        }
        assert!(store.get("0").is_some());

        store.insert("new".to_string(), Arc::new(()));
        assert!(store.get("0").is_some());
        assert!(store.get("1").is_none());
        assert!(store.get("new").is_some());
    }

    #[test]
    fn session_store_insert_existing_updates_lru_order() {
        let mut store = SessionStore::<()>::new();
        for i in 0..MAX_SESSIONS {
            store.insert(format!("{i}"), Arc::new(()));
        }
        assert!(store.get("0").is_some());
        store.insert("1".to_string(), Arc::new(()));
        assert!(store.get("2").is_some());

        store.insert("new".to_string(), Arc::new(()));
        assert!(store.get("0").is_some());
        assert!(store.get("1").is_some());
        assert!(store.get("2").is_some());
        assert!(store.get("3").is_none());
        assert!(store.get("new").is_some());
    }

    #[test]
    fn required_string_parses_valid_and_rejects_missing() {
        let value = json!({"text": "hello"});
        assert_eq!(required_string(&value, "text").unwrap(), "hello");
        assert!(required_string(&value, "missing").is_err());
    }

    #[test]
    fn required_bool_rejects_non_boolean() {
        let value = json!({"approve": "yes"});
        assert!(required_bool(&value, "approve").is_err());
    }

    #[test]
    fn map_agent_error_classifies_recoverable_tool_error() {
        let e = AgentError::Tool(takusu_agent::ToolError::InvalidArgs(
            takusu_agent::InvalidArgsError::no_field("bad"),
        ));
        let err = map_agent_error(e);
        assert_eq!(err.code, rmcp::model::ErrorCode::INVALID_PARAMS);
    }

    #[test]
    fn map_agent_error_classifies_internal_error() {
        let e = AgentError::TooManyToolCalls;
        let err = map_agent_error(e);
        assert_eq!(err.code, rmcp::model::ErrorCode::INTERNAL_ERROR);
    }
}
