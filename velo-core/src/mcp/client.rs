use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use serde_json::Value;
use tokio::sync::RwLock;
use tokio::time::sleep;
use tracing::{debug, info, warn};

use super::protocol::{
    parse_response, CallToolParams, CallToolResult, InitializeParams, JsonRpcRequest,
    ListToolsResult, McpMessage, ServerCapabilities,
};
use super::transport::{McpTransport, McpTransportFactory, TransportError};
use super::types::{
    McpContentItem, McpPrompt, McpResource, McpServerConfig, McpServerInfo, McpServerStatus,
    McpTool, McpToolResult,
};

const MAX_RECONNECT_ATTEMPTS: u32 = 3;
const RECONNECT_BACKOFF_MS: &[u64] = &[500, 2000, 5000];
const DEFAULT_TOOL_TIMEOUT_MS: u64 = 60_000;
const DEFAULT_CONNECT_TIMEOUT_MS: u64 = 60_000;
const MAX_LIST_RETRIES: u32 = 2;

pub struct McpClient {
    name: String,
    config: McpServerConfig,
    transport: RwLock<Option<Box<dyn McpTransport>>>,
    tools_cache: RwLock<Vec<McpTool>>,
    status: RwLock<McpServerStatus>,
    error_message: RwLock<Option<String>>,
    shutdown: AtomicBool,
    tool_timeout_ms: u64,
    _connect_timeout_ms: u64,
}

impl McpClient {
    pub fn new(name: String, config: McpServerConfig) -> Self {
        let env_timeout = std::env::var("MCP_TOOL_TIMEOUT")
            .ok()
            .and_then(|v| v.parse::<u64>().ok())
            .unwrap_or(DEFAULT_TOOL_TIMEOUT_MS);

        let tool_timeout_ms = config.timeout.map(|t| t.max(1000)).unwrap_or(env_timeout);

        let env_connect_timeout = std::env::var("MCP_TIMEOUT")
            .ok()
            .and_then(|v| v.parse::<u64>().ok())
            .unwrap_or(DEFAULT_CONNECT_TIMEOUT_MS);

        let connect_timeout_ms = config
            .timeout
            .map(|t| t.max(1000))
            .unwrap_or(env_connect_timeout);

        Self {
            name,
            config,
            transport: RwLock::new(None),
            tools_cache: RwLock::new(Vec::new()),
            status: RwLock::new(McpServerStatus::Disconnected),
            error_message: RwLock::new(None),
            shutdown: AtomicBool::new(false),
            tool_timeout_ms,
            _connect_timeout_ms: connect_timeout_ms,
        }
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    pub async fn current_status(&self) -> McpServerStatus {
        self.status.read().await.clone()
    }

    pub async fn cached_tools(&self) -> Vec<McpTool> {
        self.tools_cache.read().await.clone()
    }

    pub async fn info(&self) -> McpServerInfo {
        McpServerInfo {
            name: self.name.clone(),
            config: self.config.clone(),
            scope: super::types::McpScope::Project,
            status: self.status.read().await.clone(),
            tool_count: self.tools_cache.read().await.len(),
            error_message: self.error_message.read().await.clone(),
        }
    }

    pub async fn connect(&self) -> Result<(), McpClientError> {
        if self.shutdown.load(Ordering::SeqCst) {
            return Err(McpClientError::Shutdown);
        }

        *self.status.write().await = McpServerStatus::Connecting;

        let max_attempts = if self.config.always_load {
            1.max(MAX_RECONNECT_ATTEMPTS)
        } else {
            1
        };

        for attempt in 0..max_attempts {
            if self.shutdown.load(Ordering::SeqCst) {
                return Err(McpClientError::Shutdown);
            }

            if attempt > 0 {
                let delay = RECONNECT_BACKOFF_MS
                    .get(attempt as usize - 1)
                    .copied()
                    .unwrap_or(5000);
                debug!(
                    "MCP '{}' reconnecting in {}ms (attempt {}/{})",
                    self.name,
                    delay,
                    attempt + 1,
                    max_attempts
                );
                sleep(Duration::from_millis(delay)).await;
            }

            match self.try_connect().await {
                Ok(()) => {
                    *self.status.write().await = McpServerStatus::Connected;
                    return Ok(());
                }
                Err(e) => {
                    warn!(
                        "MCP '{}' connection attempt {}/{} failed: {e}",
                        self.name,
                        attempt + 1,
                        max_attempts
                    );
                    *self.error_message.write().await = Some(e.to_string());

                    if attempt + 1 >= max_attempts {
                        *self.status.write().await = McpServerStatus::Error(e.to_string());
                        return Err(e);
                    }
                }
            }
        }

        Err(McpClientError::Connection(
            "Max reconnect attempts reached".into(),
        ))
    }

    async fn try_connect(&self) -> Result<(), McpClientError> {
        let mut transport = McpTransportFactory::create(&self.config);
        transport
            .connect()
            .await
            .map_err(|e| McpClientError::Connection(format!("Transport connect failed: {e}")))?;

        let capabilities = self.send_initialize(&mut *transport).await?;
        self.send_initialized_notification(&mut *transport).await?;

        let tools = self
            .list_all_tools(&mut *transport, MAX_LIST_RETRIES)
            .await?;

        *self.transport.write().await = Some(transport);
        *self.tools_cache.write().await = tools.clone();

        info!(
            "MCP '{}' connected (tools={}, resources={}, prompts={})",
            self.name,
            tools.len(),
            capabilities.resources.is_some() as u8,
            capabilities.prompts.is_some() as u8,
        );
        Ok(())
    }

    async fn send_initialize(
        &self,
        transport: &mut dyn McpTransport,
    ) -> Result<ServerCapabilities, McpClientError> {
        let init_request = JsonRpcRequest::with_params(
            "initialize",
            serde_json::to_value(InitializeParams {
                protocol_version: "2024-11-05".to_string(),
                capabilities: super::protocol::ClientCapabilities {
                    roots: None,
                    sampling: None,
                    experimental: None,
                },
                client_info: super::protocol::ClientInfo {
                    name: "velo".to_string(),
                    version: env!("CARGO_PKG_VERSION").to_string(),
                },
            })
            .map_err(|e| McpClientError::Protocol(format!("Serialize error: {e}")))?,
        );

        let init_json = serde_json::to_string(&init_request)
            .map_err(|e| McpClientError::Protocol(format!("Serialize error: {e}")))?;

        transport
            .send(&init_json)
            .await
            .map_err(McpClientError::Transport)?;

        let response_line = transport
            .receive()
            .await
            .map_err(McpClientError::Transport)?;

        let message = McpMessage::parse(&response_line)
            .map_err(|e| McpClientError::Protocol(format!("Parse error: {e}")))?;

        match message {
            McpMessage::Response(resp) => {
                let result: super::protocol::InitializeResult = parse_response(&resp)?;
                Ok(result.capabilities)
            }
            _ => Err(McpClientError::Protocol(
                "Expected initialize response".into(),
            )),
        }
    }

    async fn send_initialized_notification(
        &self,
        transport: &mut dyn McpTransport,
    ) -> Result<(), McpClientError> {
        let notif = JsonRpcRequest::notification("notifications/initialized");
        let notif_json = serde_json::to_string(&notif)
            .map_err(|e| McpClientError::Protocol(format!("Serialize error: {e}")))?;
        transport
            .send(&notif_json)
            .await
            .map_err(McpClientError::Transport)?;
        Ok(())
    }

    async fn list_all_tools(
        &self,
        transport: &mut dyn McpTransport,
        max_retries: u32,
    ) -> Result<Vec<McpTool>, McpClientError> {
        let mut all_tools: Vec<McpTool> = Vec::new();
        let mut cursor: Option<String> = None;
        let mut retry_count = 0u32;

        loop {
            let list_request = if let Some(ref c) = cursor {
                JsonRpcRequest::with_params("tools/list", serde_json::json!({ "cursor": c }))
            } else {
                JsonRpcRequest::new("tools/list")
            };

            let list_json = serde_json::to_string(&list_request)
                .map_err(|e| McpClientError::Protocol(format!("Serialize error: {e}")))?;

            transport
                .send(&list_json)
                .await
                .map_err(McpClientError::Transport)?;

            let response_line = transport
                .receive()
                .await
                .map_err(McpClientError::Transport)?;

            let message = McpMessage::parse(&response_line)
                .map_err(|e| McpClientError::Protocol(format!("Parse error: {e}")))?;

            match message {
                McpMessage::Response(resp) => {
                    if let Some(ref err) = resp.error {
                        if retry_count < max_retries && is_transient_error(err.code) {
                            let delay = 200 * (retry_count as u64 + 1);
                            retry_count += 1;
                            debug!("tools/list transient error, retrying in {delay}ms (attempt {retry_count}/{max_retries})");
                            sleep(Duration::from_millis(delay)).await;
                            continue;
                        }
                        return Err(McpClientError::Protocol(format!(
                            "tools/list error {}: {}",
                            err.code, err.message
                        )));
                    }

                    let result: ListToolsResult = parse_response(&resp)?;
                    all_tools.extend(result.tools.into_iter().map(|t| McpTool {
                        server_name: self.name.clone(),
                        ..t
                    }));

                    cursor = result.next_cursor;
                    if cursor.is_none() {
                        break;
                    }
                }
                _ => {
                    return Err(McpClientError::Protocol(
                        "Expected tools/list response".into(),
                    ));
                }
            }
        }

        Ok(all_tools)
    }

    pub async fn call_tool(
        &self,
        tool_name: &str,
        arguments: Option<Value>,
    ) -> Result<McpToolResult, McpClientError> {
        if self.shutdown.load(Ordering::SeqCst) {
            return Err(McpClientError::Shutdown);
        }

        {
            let guard = self.transport.read().await;
            if guard.as_ref().map(|t| !t.is_connected()).unwrap_or(true) {
                drop(guard);
                debug!(
                    "MCP '{}' not connected, attempting lazy reconnect",
                    self.name
                );
                let _ = self.reconnect().await;
            }
        }

        let call_request = JsonRpcRequest::with_params(
            "tools/call",
            serde_json::to_value(CallToolParams {
                name: tool_name.to_string(),
                arguments,
            })
            .map_err(|e| McpClientError::Protocol(format!("Serialize error: {e}")))?,
        );

        let call_json = serde_json::to_string(&call_request)
            .map_err(|e| McpClientError::Protocol(format!("Serialize error: {e}")))?;

        {
            let mut guard = self.transport.write().await;
            let transport = guard.as_mut().ok_or(McpClientError::NotConnected)?;

            transport
                .send(&call_json)
                .await
                .map_err(McpClientError::Transport)?;
        }

        let response_line = self.receive_response().await?;

        let message = McpMessage::parse(&response_line)
            .map_err(|e| McpClientError::Protocol(format!("Parse error: {e}")))?;

        match message {
            McpMessage::Response(resp) => {
                let result: CallToolResult = parse_response(&resp)?;
                Ok(McpToolResult {
                    content: result.content,
                    is_error: result.is_error,
                    meta: result.meta,
                })
            }
            _ => Err(McpClientError::Protocol(
                "Expected tools/call response".into(),
            )),
        }
    }

    async fn receive_response(&self) -> Result<String, McpClientError> {
        loop {
            let line = tokio::time::timeout(Duration::from_millis(self.tool_timeout_ms), async {
                let mut guard = self.transport.write().await;
                let transport = guard.as_mut().ok_or(McpClientError::NotConnected)?;
                transport.receive().await.map_err(McpClientError::Transport)
            })
            .await
            .map_err(|_| McpClientError::Timeout)??;

            let parsed = McpMessage::parse(&line)
                .map_err(|e| McpClientError::Protocol(format!("Parse error: {e}")))?;

            match parsed {
                McpMessage::Response(_) => return Ok(line),
                McpMessage::Notification(notif) => {
                    if notif.method == "notifications/progress" {
                        debug!("MCP '{}' progress: {:?}", self.name, notif.params);
                        continue;
                    }
                    debug!(
                        "MCP '{}' ignoring notification: {}",
                        self.name, notif.method
                    );
                }
                McpMessage::Request(_) => {
                    debug!("MCP '{}' unexpected request from server", self.name);
                }
            }
        }
    }

    pub async fn list_resources(&self) -> Result<Vec<McpResource>, McpClientError> {
        if self.shutdown.load(Ordering::SeqCst) {
            return Err(McpClientError::Shutdown);
        }

        let req = JsonRpcRequest::new("resources/list");
        let json = serde_json::to_string(&req)
            .map_err(|e| McpClientError::Protocol(format!("Serialize error: {e}")))?;

        {
            let mut guard = self.transport.write().await;
            let transport = guard.as_mut().ok_or(McpClientError::NotConnected)?;
            transport
                .send(&json)
                .await
                .map_err(McpClientError::Transport)?;
        }

        let resp_line = self.receive_response().await?;
        let message = McpMessage::parse(&resp_line)
            .map_err(|e| McpClientError::Protocol(format!("Parse error: {e}")))?;

        match message {
            McpMessage::Response(resp) => {
                let result: super::protocol::ListResourcesResult = parse_response(&resp)?;
                Ok(result.resources)
            }
            _ => Err(McpClientError::Protocol(
                "Expected resources/list response".into(),
            )),
        }
    }

    pub async fn read_resource(&self, uri: &str) -> Result<Vec<McpContentItem>, McpClientError> {
        if self.shutdown.load(Ordering::SeqCst) {
            return Err(McpClientError::Shutdown);
        }

        let req = JsonRpcRequest::with_params("resources/read", serde_json::json!({ "uri": uri }));
        let json = serde_json::to_string(&req)
            .map_err(|e| McpClientError::Protocol(format!("Serialize error: {e}")))?;

        {
            let mut guard = self.transport.write().await;
            let transport = guard.as_mut().ok_or(McpClientError::NotConnected)?;
            transport
                .send(&json)
                .await
                .map_err(McpClientError::Transport)?;
        }

        let resp_line = self.receive_response().await?;
        let message = McpMessage::parse(&resp_line)
            .map_err(|e| McpClientError::Protocol(format!("Parse error: {e}")))?;

        match message {
            McpMessage::Response(resp) => {
                let result: super::protocol::ReadResourceResult = parse_response(&resp)?;
                Ok(result.contents)
            }
            _ => Err(McpClientError::Protocol(
                "Expected resources/read response".into(),
            )),
        }
    }

    pub async fn list_prompts(&self) -> Result<Vec<McpPrompt>, McpClientError> {
        if self.shutdown.load(Ordering::SeqCst) {
            return Err(McpClientError::Shutdown);
        }

        let req = JsonRpcRequest::new("prompts/list");
        let json = serde_json::to_string(&req)
            .map_err(|e| McpClientError::Protocol(format!("Serialize error: {e}")))?;

        {
            let mut guard = self.transport.write().await;
            let transport = guard.as_mut().ok_or(McpClientError::NotConnected)?;
            transport
                .send(&json)
                .await
                .map_err(McpClientError::Transport)?;
        }

        let resp_line = self.receive_response().await?;
        let message = McpMessage::parse(&resp_line)
            .map_err(|e| McpClientError::Protocol(format!("Parse error: {e}")))?;

        match message {
            McpMessage::Response(resp) => {
                let result: super::protocol::ListPromptsResult = parse_response(&resp)?;
                Ok(result.prompts)
            }
            _ => Err(McpClientError::Protocol(
                "Expected prompts/list response".into(),
            )),
        }
    }

    pub async fn get_prompt(
        &self,
        name: &str,
        arguments: Option<Value>,
    ) -> Result<Vec<McpContentItem>, McpClientError> {
        if self.shutdown.load(Ordering::SeqCst) {
            return Err(McpClientError::Shutdown);
        }

        let req = JsonRpcRequest::with_params(
            "prompts/get",
            serde_json::json!({ "name": name, "arguments": arguments }),
        );
        let json = serde_json::to_string(&req)
            .map_err(|e| McpClientError::Protocol(format!("Serialize error: {e}")))?;

        {
            let mut guard = self.transport.write().await;
            let transport = guard.as_mut().ok_or(McpClientError::NotConnected)?;
            transport
                .send(&json)
                .await
                .map_err(McpClientError::Transport)?;
        }

        let resp_line = self.receive_response().await?;
        let message = McpMessage::parse(&resp_line)
            .map_err(|e| McpClientError::Protocol(format!("Parse error: {e}")))?;

        match message {
            McpMessage::Response(resp) => {
                let result: super::protocol::GetPromptResult = parse_response(&resp)?;
                Ok(result.messages)
            }
            _ => Err(McpClientError::Protocol(
                "Expected prompts/get response".into(),
            )),
        }
    }

    pub async fn disconnect(&self) -> Result<(), McpClientError> {
        self.shutdown.store(true, Ordering::SeqCst);
        *self.status.write().await = McpServerStatus::Disconnected;

        let mut transport = self.transport.write().await;
        if let Some(ref mut t) = *transport {
            let _ = t.close().await;
        }
        *transport = None;

        Ok(())
    }

    pub async fn reconnect(&self) -> Result<(), McpClientError> {
        self.shutdown.store(true, Ordering::SeqCst);

        {
            let mut transport = self.transport.write().await;
            if let Some(ref mut t) = *transport {
                let _ = t.close().await;
            }
            *transport = None;
        }

        *self.status.write().await = McpServerStatus::Disconnected;
        self.shutdown.store(false, Ordering::SeqCst);
        self.connect().await
    }
}

fn is_transient_error(code: i64) -> bool {
    matches!(code, -32003..=-32000)
}

#[derive(Debug)]
pub enum McpClientError {
    Connection(String),
    Transport(TransportError),
    Protocol(String),
    Timeout,
    NotConnected,
    Shutdown,
}

impl From<super::protocol::McpProtocolError> for McpClientError {
    fn from(e: super::protocol::McpProtocolError) -> Self {
        Self::Protocol(e.to_string())
    }
}

impl std::fmt::Display for McpClientError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Connection(msg) => write!(f, "{msg}"),
            Self::Transport(e) => write!(f, "{e}"),
            Self::Protocol(msg) => write!(f, "Protocol error: {msg}"),
            Self::Timeout => write!(f, "Request timed out"),
            Self::NotConnected => write!(f, "Not connected"),
            Self::Shutdown => write!(f, "Client is shut down"),
        }
    }
}

impl std::error::Error for McpClientError {}
