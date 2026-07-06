use serde::{Deserialize, Serialize};
use serde_json::Value;
use uuid::Uuid;

use super::types::{McpContentItem, McpPrompt, McpResource, McpResourceTemplate, McpTool};

pub type RequestId = serde_json::Value;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JsonRpcRequest {
    pub jsonrpc: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub id: Option<RequestId>,
    pub method: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub params: Option<Value>,
}

impl JsonRpcRequest {
    pub fn new(method: impl Into<String>) -> Self {
        Self {
            jsonrpc: "2.0".to_string(),
            id: Some(Value::String(Uuid::new_v4().to_string())),
            method: method.into(),
            params: None,
        }
    }

    pub fn with_params(method: impl Into<String>, params: Value) -> Self {
        Self {
            jsonrpc: "2.0".to_string(),
            id: Some(Value::String(Uuid::new_v4().to_string())),
            method: method.into(),
            params: Some(params),
        }
    }

    pub fn notification(method: impl Into<String>) -> Self {
        Self {
            jsonrpc: "2.0".to_string(),
            id: None,
            method: method.into(),
            params: None,
        }
    }

    pub fn id_str(&self) -> String {
        self.id
            .as_ref()
            .and_then(|v| v.as_str().map(|s| s.to_string()))
            .unwrap_or_else(|| "notification".to_string())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JsonRpcResponse {
    pub jsonrpc: String,
    pub id: RequestId,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<JsonRpcError>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JsonRpcError {
    pub code: i64,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data: Option<Value>,
}

impl JsonRpcError {
    pub fn new(code: i64, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
            data: None,
        }
    }

    pub fn parse_error(msg: impl Into<String>) -> Self {
        Self::new(-32700, msg)
    }

    pub fn invalid_request(msg: impl Into<String>) -> Self {
        Self::new(-32600, msg)
    }

    pub fn method_not_found(msg: impl Into<String>) -> Self {
        Self::new(-32601, msg)
    }

    pub fn invalid_params(msg: impl Into<String>) -> Self {
        Self::new(-32602, msg)
    }

    pub fn internal_error(msg: impl Into<String>) -> Self {
        Self::new(-32603, msg)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum McpMessage {
    Request(JsonRpcRequest),
    Response(JsonRpcResponse),
    Notification(JsonRpcRequest),
}

impl McpMessage {
    pub fn parse(data: &str) -> Result<Self, serde_json::Error> {
        let value: Value = serde_json::from_str(data)?;

        let has_id = value
            .get("id")
            .and_then(|v| if v.is_null() { None } else { Some(v) })
            .is_some();
        let has_method = value.get("method").is_some();
        let has_result = value.get("result").is_some();
        let has_error = value.get("error").is_some();

        if has_method && has_id {
            serde_json::from_value::<JsonRpcRequest>(value.clone()).map(McpMessage::Request)
        } else if has_method && !has_id {
            serde_json::from_value::<JsonRpcRequest>(value.clone()).map(McpMessage::Notification)
        } else if has_result || has_error {
            serde_json::from_value::<JsonRpcResponse>(value.clone()).map(McpMessage::Response)
        } else {
            Err(serde::de::Error::custom(format!(
                "Unknown MCP message: {data}"
            )))
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InitializeParams {
    pub protocol_version: String,
    pub capabilities: ClientCapabilities,
    pub client_info: ClientInfo,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClientCapabilities {
    #[serde(default)]
    pub roots: Option<RootsCapability>,
    #[serde(default)]
    pub sampling: Option<Value>,
    #[serde(default)]
    pub experimental: Option<Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RootsCapability {
    #[serde(default)]
    pub list_changed: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClientInfo {
    pub name: String,
    pub version: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InitializeResult {
    pub protocol_version: String,
    pub capabilities: ServerCapabilities,
    pub server_info: ServerInfo,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServerCapabilities {
    #[serde(default)]
    pub tools: Option<ToolCapabilities>,
    #[serde(default)]
    pub resources: Option<Value>,
    #[serde(default)]
    pub prompts: Option<Value>,
    #[serde(default)]
    pub logging: Option<Value>,
    #[serde(default)]
    pub experimental: Option<Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolCapabilities {
    #[serde(default)]
    pub list_changed: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServerInfo {
    pub name: String,
    pub version: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ListToolsResult {
    pub tools: Vec<McpTool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub next_cursor: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CallToolParams {
    pub name: String,
    pub arguments: Option<Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CallToolResult {
    pub content: Vec<McpContentItem>,
    #[serde(default)]
    pub is_error: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub meta: Option<Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ListResourcesResult {
    pub resources: Vec<McpResource>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub next_cursor: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ListResourceTemplatesResult {
    pub resource_templates: Vec<McpResourceTemplate>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub next_cursor: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ListPromptsResult {
    pub prompts: Vec<McpPrompt>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub next_cursor: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReadResourceResult {
    pub contents: Vec<super::types::McpContentItem>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GetPromptResult {
    pub messages: Vec<super::types::McpContentItem>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub meta: Option<Value>,
}

pub fn parse_response<T: serde::de::DeserializeOwned>(
    response: &JsonRpcResponse,
) -> Result<T, McpProtocolError> {
    if let Some(ref err) = response.error {
        return Err(McpProtocolError::RemoteError {
            code: err.code,
            message: err.message.clone(),
        });
    }

    let result = response
        .result
        .as_ref()
        .ok_or(McpProtocolError::MissingResult)?;

    serde_json::from_value(result.clone())
        .map_err(|e| McpProtocolError::Deserialization(e.to_string()))
}

#[derive(Debug)]
pub enum McpProtocolError {
    RemoteError { code: i64, message: String },
    MissingResult,
    Timeout,
    Deserialization(String),
    ConnectionLost(String),
}

impl std::fmt::Display for McpProtocolError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::RemoteError { code, message } => {
                write!(f, "MCP error {code}: {message}")
            }
            Self::MissingResult => write!(f, "MCP response missing result"),
            Self::Timeout => write!(f, "MCP request timed out"),
            Self::Deserialization(msg) => write!(f, "MCP deserialization error: {msg}"),
            Self::ConnectionLost(msg) => write!(f, "MCP connection lost: {msg}"),
        }
    }
}

impl std::error::Error for McpProtocolError {}
