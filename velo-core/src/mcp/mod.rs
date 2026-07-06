pub mod client;
pub mod config;
pub mod protocol;
pub mod registry;
pub mod security;
pub mod transport;
pub mod types;

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use ractor::RpcReplyPort;
use serde_json::Value;

pub use registry::{McpRegistryActor, McpRegistryError, McpRegistryMessage};
pub use types::{
    McpContentItem, McpPrompt, McpResource, McpResourceTemplate, McpScope, McpServerConfig,
    McpServerInfo, McpServerStatus, McpTool, McpToolResult, TransportType,
};

use crate::memory::StorageBackend;
use crate::skills::{AgentSkill, SkillError};

pub struct McpRegistryHandle {
    actor_ref: ractor::ActorRef<McpRegistryMessage>,
}

impl Clone for McpRegistryHandle {
    fn clone(&self) -> Self {
        Self {
            actor_ref: self.actor_ref.clone(),
        }
    }
}

impl McpRegistryHandle {
    pub async fn new(extra_config_paths: Vec<PathBuf>) -> Result<Self, McpRegistryError> {
        let (actor_ref, _handle) = ractor::Actor::spawn(
            Some("mcp-registry".into()),
            McpRegistryActor,
            extra_config_paths,
        )
        .await
        .map_err(|e| McpRegistryError::Internal(format!("Failed to spawn MCP registry: {e}")))?;

        Ok(Self { actor_ref })
    }

    async fn call_rpc<T: Send + 'static>(
        &self,
        f: impl FnOnce(RpcReplyPort<T>) -> McpRegistryMessage + Send + 'static,
        timeout_secs: u64,
    ) -> Result<T, McpRegistryError> {
        let result = self
            .actor_ref
            .call(f, Some(Duration::from_secs(timeout_secs)))
            .await
            .map_err(|e| McpRegistryError::Internal(format!("Actor call failed: {e}")))?;

        match result {
            ractor::rpc::CallResult::Success(r) => Ok(r),
            ractor::rpc::CallResult::Timeout => {
                Err(McpRegistryError::Internal("RPC timeout".into()))
            }
            ractor::rpc::CallResult::SenderError => Err(McpRegistryError::Internal(
                "Actor sender channel error".into(),
            )),
        }
    }

    pub async fn discover_and_connect(&self) -> Result<usize, McpRegistryError> {
        self.call_rpc(
            |reply| McpRegistryMessage::DiscoverAndConnect { reply },
            120,
        )
        .await
        .and_then(|r| r)
    }

    pub async fn connect_server(&self, name: &str) -> Result<(), McpRegistryError> {
        let name = name.to_string();
        self.call_rpc(
            move |reply| McpRegistryMessage::ConnectServer {
                name: name.clone(),
                reply,
            },
            60,
        )
        .await
        .and_then(|r| r)
    }

    pub async fn disconnect_server(&self, name: &str) -> Result<(), McpRegistryError> {
        let name = name.to_string();
        self.call_rpc(
            move |reply| McpRegistryMessage::DisconnectServer {
                name: name.clone(),
                reply,
            },
            30,
        )
        .await
        .and_then(|r| r)
    }

    pub async fn reconnect_server(&self, name: &str) -> Result<(), McpRegistryError> {
        let name = name.to_string();
        self.call_rpc(
            move |reply| McpRegistryMessage::ReconnectServer {
                name: name.clone(),
                reply,
            },
            60,
        )
        .await
        .and_then(|r| r)
    }

    pub async fn call_tool(
        &self,
        server_name: &str,
        tool_name: &str,
        arguments: Option<Value>,
    ) -> Result<types::McpToolResult, McpRegistryError> {
        let server_name = server_name.to_string();
        let tool_name = tool_name.to_string();
        self.call_rpc(
            move |reply| McpRegistryMessage::CallTool {
                server_name: server_name.clone(),
                tool_name: tool_name.clone(),
                arguments,
                reply,
            },
            300,
        )
        .await
        .and_then(|r| r)
    }

    pub async fn list_servers(&self) -> Vec<McpServerInfo> {
        self.call_rpc(McpRegistryMessage::ListServers, 30)
            .await
            .unwrap_or_default()
    }

    pub async fn get_server_info(&self, name: &str) -> Option<McpServerInfo> {
        let name = name.to_string();
        self.call_rpc(
            move |reply| McpRegistryMessage::GetServerInfo {
                name: name.clone(),
                reply,
            },
            30,
        )
        .await
        .ok()
        .flatten()
    }

    pub async fn get_all_tools(&self) -> Vec<McpTool> {
        self.call_rpc(McpRegistryMessage::GetAllTools, 30)
            .await
            .unwrap_or_default()
    }

    pub async fn shutdown_all(&self) -> Result<(), McpRegistryError> {
        self.call_rpc(McpRegistryMessage::ShutdownAll, 30)
            .await
            .and_then(|r| r)
    }

    pub async fn list_resources(
        &self,
        server_name: &str,
    ) -> Result<Vec<McpResource>, McpRegistryError> {
        let server_name = server_name.to_string();
        self.call_rpc(
            move |reply| McpRegistryMessage::ListResources {
                server_name: server_name.clone(),
                reply,
            },
            60,
        )
        .await
        .and_then(|r| r)
    }

    pub async fn read_resource(
        &self,
        server_name: &str,
        uri: &str,
    ) -> Result<Vec<McpContentItem>, McpRegistryError> {
        let server_name = server_name.to_string();
        let uri = uri.to_string();
        self.call_rpc(
            move |reply| McpRegistryMessage::ReadResource {
                server_name: server_name.clone(),
                uri: uri.clone(),
                reply,
            },
            60,
        )
        .await
        .and_then(|r| r)
    }

    pub async fn list_prompts(
        &self,
        server_name: &str,
    ) -> Result<Vec<McpPrompt>, McpRegistryError> {
        let server_name = server_name.to_string();
        self.call_rpc(
            move |reply| McpRegistryMessage::ListPrompts {
                server_name: server_name.clone(),
                reply,
            },
            60,
        )
        .await
        .and_then(|r| r)
    }

    pub async fn get_prompt(
        &self,
        server_name: &str,
        name: &str,
        arguments: Option<Value>,
    ) -> Result<Vec<McpContentItem>, McpRegistryError> {
        let server_name = server_name.to_string();
        let name = name.to_string();
        self.call_rpc(
            move |reply| McpRegistryMessage::GetPrompt {
                server_name: server_name.clone(),
                name: name.clone(),
                arguments,
                reply,
            },
            60,
        )
        .await
        .and_then(|r| r)
    }

    pub async fn resolve_and_call(
        &self,
        prefixed_name: &str,
        arguments: Option<Value>,
    ) -> Result<McpToolResult, McpRegistryError> {
        let prefixed_name = prefixed_name.to_string();
        self.call_rpc(
            move |reply| McpRegistryMessage::ResolveToolCall {
                prefixed_name: prefixed_name.clone(),
                arguments,
                reply,
            },
            300,
        )
        .await
        .and_then(|r| r)
    }
}

pub struct McpToolSkill {
    pub tool: McpTool,
}

#[async_trait::async_trait]
impl AgentSkill for McpToolSkill {
    fn name(&self) -> &'static str {
        Box::leak(self.tool.prefixed_name().into_boxed_str())
    }

    fn description(&self) -> String {
        self.tool
            .description
            .clone()
            .unwrap_or_default()
            .chars()
            .take(2000)
            .collect()
    }

    fn parameters(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "properties": self.tool.input_schema.get("properties").cloned().unwrap_or_default(),
            "required": self.tool.input_schema.get("required").cloned().unwrap_or_default(),
        })
    }

    fn user_invocable(&self) -> bool {
        true
    }

    fn disable_model_invocation(&self) -> bool {
        false
    }

    fn tags(&self) -> Vec<&'static str> {
        vec!["mcp"]
    }

    async fn execute(&self, _args: Value, _ctx: Arc<StorageBackend>) -> Result<Value, SkillError> {
        Err(SkillError::Internal(
            "MCP tools are routed through the MCP registry, not directly executed".into(),
        ))
    }
}
