use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

use ractor::{Actor, ActorProcessingErr, ActorRef, RpcReplyPort};
use serde_json::Value;
use tokio::sync::RwLock;
use tracing::{debug, info, warn};

use super::client::McpClient;
use super::config::discover_all_servers;
use super::types::{McpContentItem, McpPrompt, McpResource, McpServerInfo, McpTool, McpToolResult};

#[derive(Clone)]
pub struct McpRegistryActor;

pub struct McpRegistryState {
    clients: HashMap<String, Arc<McpClient>>,
    server_order: Vec<String>,
    extra_config_paths: Vec<PathBuf>,
    _connected_count: Arc<RwLock<usize>>,
}

#[async_trait::async_trait]
impl Actor for McpRegistryActor {
    type State = McpRegistryState;
    type Msg = McpRegistryMessage;
    type Arguments = Vec<PathBuf>;

    async fn pre_start(
        &self,
        _myself: ActorRef<Self::Msg>,
        extra_paths: Self::Arguments,
    ) -> Result<Self::State, ActorProcessingErr> {
        Ok(McpRegistryState {
            clients: HashMap::new(),
            server_order: Vec::new(),
            extra_config_paths: extra_paths,
            _connected_count: Arc::new(RwLock::new(0)),
        })
    }

    async fn handle(
        &self,
        _myself: ActorRef<Self::Msg>,
        message: Self::Msg,
        state: &mut Self::State,
    ) -> Result<(), ActorProcessingErr> {
        match message {
            McpRegistryMessage::DiscoverAndConnect { reply } => {
                let result = self.discover_and_connect(state).await;
                let _ = reply.send(result);
            }

            McpRegistryMessage::ConnectServer { name, reply } => {
                let result = self.connect_server(state, &name).await;
                let _ = reply.send(result);
            }

            McpRegistryMessage::DisconnectServer { name, reply } => {
                let result = self.disconnect_server(state, &name).await;
                let _ = reply.send(result);
            }

            McpRegistryMessage::ReconnectServer { name, reply } => {
                let result = self.reconnect_server(state, &name).await;
                let _ = reply.send(result);
            }

            McpRegistryMessage::CallTool {
                server_name,
                tool_name,
                arguments,
                reply,
            } => {
                let result = self
                    .call_tool(state, &server_name, &tool_name, arguments)
                    .await;
                let _ = reply.send(result);
            }

            McpRegistryMessage::ListServers(reply) => {
                let servers = self.list_servers(state).await;
                let _ = reply.send(servers);
            }

            McpRegistryMessage::GetServerInfo { name, reply } => {
                let info = self.get_server_info(state, &name).await;
                let _ = reply.send(info);
            }

            McpRegistryMessage::GetAllTools(reply) => {
                let tools = self.get_all_tools(state).await;
                let _ = reply.send(tools);
            }

            McpRegistryMessage::ResolveToolCall {
                prefixed_name,
                arguments,
                reply,
            } => {
                let result = self
                    .resolve_and_call(state, &prefixed_name, arguments)
                    .await;
                let _ = reply.send(result);
            }

            McpRegistryMessage::ShutdownAll(reply) => {
                let result = self.shutdown_all(state).await;
                let _ = reply.send(result);
            }

            McpRegistryMessage::ListResources { server_name, reply } => {
                let result = self.list_resources(state, &server_name).await;
                let _ = reply.send(result);
            }

            McpRegistryMessage::ReadResource {
                server_name,
                uri,
                reply,
            } => {
                let result = self.read_resource(state, &server_name, &uri).await;
                let _ = reply.send(result);
            }

            McpRegistryMessage::ListPrompts { server_name, reply } => {
                let result = self.list_prompts(state, &server_name).await;
                let _ = reply.send(result);
            }

            McpRegistryMessage::GetPrompt {
                server_name,
                name,
                arguments,
                reply,
            } => {
                let result = self.get_prompt(state, &server_name, &name, arguments).await;
                let _ = reply.send(result);
            }
        }

        Ok(())
    }
}

impl McpRegistryActor {
    async fn discover_and_connect(
        &self,
        state: &mut McpRegistryState,
    ) -> Result<usize, McpRegistryError> {
        let servers = discover_all_servers(&state.extra_config_paths);

        if servers.is_empty() {
            info!("No MCP servers discovered");
            return Ok(0);
        }

        info!("Discovered {} MCP server(s), connecting...", servers.len());

        for (_scope, name, config) in &servers {
            if state.clients.contains_key(name) {
                debug!("MCP server '{name}' already registered, skipping");
                continue;
            }

            let client = Arc::new(McpClient::new(name.clone(), config.clone()));

            let client_clone = client.clone();
            let server_name = name.clone();
            tokio::spawn(async move {
                if let Err(e) = client_clone.connect().await {
                    warn!("MCP server '{server_name}' connection failed: {e}");
                }
            });

            state.clients.insert(name.clone(), client);
            state.server_order.push(name.clone());
        }

        Ok(servers.len())
    }

    async fn connect_server(
        &self,
        state: &mut McpRegistryState,
        name: &str,
    ) -> Result<(), McpRegistryError> {
        let client = state
            .clients
            .get(name)
            .ok_or_else(|| McpRegistryError::NotFound(name.to_string()))?;

        client
            .connect()
            .await
            .map_err(|e| McpRegistryError::ConnectionFailed(name.to_string(), e.to_string()))?;

        Ok(())
    }

    async fn disconnect_server(
        &self,
        state: &mut McpRegistryState,
        name: &str,
    ) -> Result<(), McpRegistryError> {
        let client = state
            .clients
            .get(name)
            .ok_or_else(|| McpRegistryError::NotFound(name.to_string()))?;

        client
            .disconnect()
            .await
            .map_err(|e| McpRegistryError::OperationFailed(name.to_string(), e.to_string()))?;

        Ok(())
    }

    async fn reconnect_server(
        &self,
        state: &mut McpRegistryState,
        name: &str,
    ) -> Result<(), McpRegistryError> {
        let client = state
            .clients
            .get(name)
            .ok_or_else(|| McpRegistryError::NotFound(name.to_string()))?;

        client
            .reconnect()
            .await
            .map_err(|e| McpRegistryError::ConnectionFailed(name.to_string(), e.to_string()))?;

        Ok(())
    }

    async fn call_tool(
        &self,
        state: &McpRegistryState,
        server_name: &str,
        tool_name: &str,
        arguments: Option<Value>,
    ) -> Result<McpToolResult, McpRegistryError> {
        let client = state
            .clients
            .get(server_name)
            .ok_or_else(|| McpRegistryError::NotFound(server_name.to_string()))?;

        client.call_tool(tool_name, arguments).await.map_err(|e| {
            McpRegistryError::CallFailed(
                server_name.to_string(),
                tool_name.to_string(),
                e.to_string(),
            )
        })
    }

    async fn list_servers(&self, state: &McpRegistryState) -> Vec<McpServerInfo> {
        let mut servers = Vec::new();
        for name in &state.server_order {
            if let Some(client) = state.clients.get(name) {
                servers.push(client.info().await);
            }
        }
        servers
    }

    async fn get_server_info(&self, state: &McpRegistryState, name: &str) -> Option<McpServerInfo> {
        match state.clients.get(name) {
            Some(client) => Some(client.info().await),
            None => None,
        }
    }

    async fn get_all_tools(&self, state: &McpRegistryState) -> Vec<McpTool> {
        let mut all_tools = Vec::new();
        for name in &state.server_order {
            if let Some(client) = state.clients.get(name) {
                all_tools.extend(client.cached_tools().await);
            }
        }
        all_tools
    }

    async fn resolve_and_call(
        &self,
        state: &McpRegistryState,
        prefixed_name: &str,
        arguments: Option<Value>,
    ) -> Result<McpToolResult, McpRegistryError> {
        let parts: Vec<&str> = prefixed_name.splitn(3, "__").collect();
        if parts.len() < 3 {
            return Err(McpRegistryError::InvalidToolName(prefixed_name.to_string()));
        }

        let server_name = parts[1];
        let tool_name = parts[2];

        self.call_tool(state, server_name, tool_name, arguments)
            .await
    }

    async fn list_resources(
        &self,
        state: &McpRegistryState,
        server_name: &str,
    ) -> Result<Vec<McpResource>, McpRegistryError> {
        let client = state
            .clients
            .get(server_name)
            .ok_or_else(|| McpRegistryError::NotFound(server_name.to_string()))?;

        client
            .list_resources()
            .await
            .map_err(|e| McpRegistryError::OperationFailed(server_name.to_string(), e.to_string()))
    }

    async fn read_resource(
        &self,
        state: &McpRegistryState,
        server_name: &str,
        uri: &str,
    ) -> Result<Vec<McpContentItem>, McpRegistryError> {
        let client = state
            .clients
            .get(server_name)
            .ok_or_else(|| McpRegistryError::NotFound(server_name.to_string()))?;

        client
            .read_resource(uri)
            .await
            .map_err(|e| McpRegistryError::OperationFailed(server_name.to_string(), e.to_string()))
    }

    async fn list_prompts(
        &self,
        state: &McpRegistryState,
        server_name: &str,
    ) -> Result<Vec<McpPrompt>, McpRegistryError> {
        let client = state
            .clients
            .get(server_name)
            .ok_or_else(|| McpRegistryError::NotFound(server_name.to_string()))?;

        client
            .list_prompts()
            .await
            .map_err(|e| McpRegistryError::OperationFailed(server_name.to_string(), e.to_string()))
    }

    async fn get_prompt(
        &self,
        state: &McpRegistryState,
        server_name: &str,
        name: &str,
        arguments: Option<Value>,
    ) -> Result<Vec<McpContentItem>, McpRegistryError> {
        let client = state
            .clients
            .get(server_name)
            .ok_or_else(|| McpRegistryError::NotFound(server_name.to_string()))?;

        client
            .get_prompt(name, arguments)
            .await
            .map_err(|e| McpRegistryError::OperationFailed(server_name.to_string(), e.to_string()))
    }

    async fn shutdown_all(&self, state: &mut McpRegistryState) -> Result<(), McpRegistryError> {
        let names: Vec<String> = state.server_order.clone();
        for name in &names {
            if let Some(client) = state.clients.get(name) {
                if let Err(e) = client.disconnect().await {
                    warn!("Error disconnecting MCP '{name}': {e}");
                }
            }
        }
        state.clients.clear();
        state.server_order.clear();
        info!("All MCP servers shut down");
        Ok(())
    }
}

pub enum McpRegistryMessage {
    DiscoverAndConnect {
        reply: RpcReplyPort<Result<usize, McpRegistryError>>,
    },
    ConnectServer {
        name: String,
        reply: RpcReplyPort<Result<(), McpRegistryError>>,
    },
    DisconnectServer {
        name: String,
        reply: RpcReplyPort<Result<(), McpRegistryError>>,
    },
    ReconnectServer {
        name: String,
        reply: RpcReplyPort<Result<(), McpRegistryError>>,
    },
    CallTool {
        server_name: String,
        tool_name: String,
        arguments: Option<Value>,
        reply: RpcReplyPort<Result<McpToolResult, McpRegistryError>>,
    },
    ListServers(RpcReplyPort<Vec<McpServerInfo>>),
    GetServerInfo {
        name: String,
        reply: RpcReplyPort<Option<McpServerInfo>>,
    },
    GetAllTools(RpcReplyPort<Vec<McpTool>>),
    ResolveToolCall {
        prefixed_name: String,
        arguments: Option<Value>,
        reply: RpcReplyPort<Result<McpToolResult, McpRegistryError>>,
    },
    ShutdownAll(RpcReplyPort<Result<(), McpRegistryError>>),

    ListResources {
        server_name: String,
        reply: RpcReplyPort<Result<Vec<McpResource>, McpRegistryError>>,
    },
    ReadResource {
        server_name: String,
        uri: String,
        reply: RpcReplyPort<Result<Vec<McpContentItem>, McpRegistryError>>,
    },
    ListPrompts {
        server_name: String,
        reply: RpcReplyPort<Result<Vec<McpPrompt>, McpRegistryError>>,
    },
    GetPrompt {
        server_name: String,
        name: String,
        arguments: Option<Value>,
        reply: RpcReplyPort<Result<Vec<McpContentItem>, McpRegistryError>>,
    },
}

#[derive(Debug)]
pub enum McpRegistryError {
    NotFound(String),
    ConnectionFailed(String, String),
    OperationFailed(String, String),
    CallFailed(String, String, String),
    InvalidToolName(String),
    Internal(String),
}

impl std::fmt::Display for McpRegistryError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotFound(n) => write!(f, "MCP server '{n}' not found"),
            Self::ConnectionFailed(n, e) => write!(f, "MCP '{n}' connection failed: {e}"),
            Self::OperationFailed(n, e) => write!(f, "MCP '{n}' operation failed: {e}"),
            Self::CallFailed(s, t, e) => write!(f, "MCP '{s}' tool '{t}' call failed: {e}"),
            Self::InvalidToolName(n) => write!(
                f,
                "Invalid MCP tool name '{n}' (expected mcp__server__tool)"
            ),
            Self::Internal(m) => write!(f, "Internal error: {m}"),
        }
    }
}

impl std::error::Error for McpRegistryError {}
