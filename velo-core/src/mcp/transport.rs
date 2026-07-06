use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use futures_util::{SinkExt, StreamExt};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, ChildStdin, Command};
use tokio::sync::{mpsc, Mutex};
use tokio::time::timeout;
use tracing::{debug, info, trace, warn};

use super::types::{McpServerConfig, TransportType};

#[async_trait]
pub trait McpTransport: Send + Sync {
    async fn connect(&mut self) -> Result<(), TransportError>;
    async fn send(&mut self, message: &str) -> Result<(), TransportError>;
    async fn receive(&mut self) -> Result<String, TransportError>;
    async fn close(&mut self) -> Result<(), TransportError>;
    fn is_connected(&self) -> bool;
}

const SSE_MAX_FRAME_SIZE: usize = 16 * 1024 * 1024;
const STDIO_MAX_NONPROTOCOL_LINE: usize = 1024 * 1024;

pub struct StdioTransport {
    command: String,
    args: Vec<String>,
    env: HashMap<String, String>,
    timeout: Duration,
    child: Option<Child>,
    stdin: Option<Arc<Mutex<ChildStdin>>>,
    receiver: Option<mpsc::Receiver<String>>,
    connected: bool,
}

impl Drop for StdioTransport {
    fn drop(&mut self) {
        if let Some(mut child) = self.child.take() {
            let _ = child.start_kill();
        }
    }
}

impl StdioTransport {
    pub fn new(config: &McpServerConfig) -> Self {
        let timeout_ms = config.timeout.map(|t| t.max(1000)).unwrap_or(60_000);

        Self {
            command: config.command.clone().unwrap_or_default(),
            args: config.args.clone(),
            env: config.env.clone(),
            timeout: Duration::from_millis(timeout_ms),
            child: None,
            stdin: None,
            receiver: None,
            connected: false,
        }
    }

    fn build_env(&self) -> HashMap<String, String> {
        let mut env_map: HashMap<String, String> = std::env::vars().collect();

        for (key, value) in &self.env {
            env_map.insert(key.clone(), value.clone());
        }

        env_map.insert("VELO".to_string(), "1".to_string());
        if let Ok(session_id) =
            std::env::var("VELO_SESSION_ID").or_else(|_| std::env::var("MCP_SESSION_ID"))
        {
            env_map.insert("VELO_SESSION_ID".to_string(), session_id);
        }
        if let Ok(proj_dir) = std::env::var("VELO_PROJECT_DIR")
            .or_else(|_| std::env::current_dir().map(|d| d.to_string_lossy().to_string()))
        {
            env_map.insert("VELO_PROJECT_DIR".to_string(), proj_dir);
        }

        for key in self.env.keys() {
            if key.starts_with("OTEL_") {
                env_map.remove(key);
            }
        }

        env_map
    }
}

#[async_trait]
impl McpTransport for StdioTransport {
    async fn connect(&mut self) -> Result<(), TransportError> {
        let mut cmd = Command::new(&self.command);
        cmd.args(&self.args)
            .envs(&self.build_env())
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped());

        let mut child = cmd.spawn().map_err(|e| {
            TransportError::Connection(format!(
                "Failed to spawn MCP server '{}': {e}",
                self.command
            ))
        })?;

        let stdin = child
            .stdin
            .take()
            .ok_or(TransportError::Connection("Failed to capture stdin".into()))?;

        let stdout = child.stdout.take().ok_or(TransportError::Connection(
            "Failed to capture stdout".into(),
        ))?;

        let stderr = child.stderr.take().ok_or(TransportError::Connection(
            "Failed to capture stderr".into(),
        ))?;

        let (tx, rx) = mpsc::channel::<String>(256);

        let reader = BufReader::new(stdout);
        let tx_clone = tx.clone();
        let server_name = self.command.clone();
        let server_name2 = server_name.clone();
        let _stdout_handle = tokio::spawn(async move {
            let mut lines = reader.lines();
            let mut total_bytes: usize = 0;
            while let Ok(Some(line)) = lines.next_line().await {
                if line.len() > STDIO_MAX_NONPROTOCOL_LINE {
                    warn!("MCP stdout line too large ({} bytes), skipping", line.len());
                    continue;
                }
                total_bytes += line.len() + 1;
                if total_bytes > SSE_MAX_FRAME_SIZE {
                    warn!("MCP stdout reader exceeded 16MB cap for {server_name}");
                    break;
                }
                let trimmed = line.trim().to_string();
                if trimmed.is_empty() {
                    continue;
                }
                if tx_clone.send(trimmed).await.is_err() {
                    break;
                }
            }
            debug!("MCP stdout reader ended for {server_name}");
        });

        let _stderr_handle = tokio::spawn(async move {
            let reader = BufReader::new(stderr);
            let mut lines = reader.lines();
            while let Ok(Some(line)) = lines.next_line().await {
                let trimmed = line.trim().to_string();
                if trimmed.is_empty() {
                    continue;
                }
                trace!("MCP stderr [{}]: {trimmed}", server_name2);
            }
        });

        self.child = Some(child);
        self.stdin = Some(Arc::new(Mutex::new(stdin)));
        self.receiver = Some(rx);
        self.connected = true;

        info!("MCP stdio server '{}' connected", self.command);
        Ok(())
    }

    async fn send(&mut self, message: &str) -> Result<(), TransportError> {
        let stdin = self.stdin.as_ref().ok_or(TransportError::NotConnected)?;

        let mut guard = stdin.lock().await;
        guard
            .write_all(message.as_bytes())
            .await
            .map_err(|e| TransportError::Send(format!("Failed to send: {e}")))?;
        guard
            .write_all(b"\n")
            .await
            .map_err(|e| TransportError::Send(format!("Failed to send delimiter: {e}")))?;
        guard
            .flush()
            .await
            .map_err(|e| TransportError::Send(format!("Failed to flush: {e}")))?;

        Ok(())
    }

    async fn receive(&mut self) -> Result<String, TransportError> {
        let rx = self.receiver.as_mut().ok_or(TransportError::NotConnected)?;

        match timeout(self.timeout, rx.recv()).await {
            Ok(Some(line)) => Ok(line),
            Ok(None) => {
                self.connected = false;
                Err(TransportError::Connection(
                    "Server closed connection".into(),
                ))
            }
            Err(_) => Err(TransportError::Timeout),
        }
    }

    async fn close(&mut self) -> Result<(), TransportError> {
        self.connected = false;
        if let Some(mut child) = self.child.take() {
            let _ = child.kill().await;
            let _ = child.wait().await;
            info!("MCP stdio server '{}' terminated", self.command);
        }
        Ok(())
    }

    fn is_connected(&self) -> bool {
        self.connected
    }
}

pub struct HttpSseTransport {
    url: String,
    headers: HashMap<String, String>,
    timeout: Duration,
    client: reqwest::Client,
    connected: bool,
    receiver: Option<mpsc::Receiver<String>>,
    event_sender: mpsc::Sender<String>,
}

impl HttpSseTransport {
    pub fn new(config: &McpServerConfig) -> Self {
        let timeout_ms = config.timeout.map(|t| t.max(1000)).unwrap_or(60_000);

        let client = reqwest::Client::builder()
            .timeout(Duration::from_millis(timeout_ms))
            .danger_accept_invalid_certs(false)
            .build()
            .unwrap_or_default();

        let (tx, rx) = mpsc::channel(256);

        Self {
            url: config.url.clone().unwrap_or_default(),
            headers: config.headers.clone(),
            timeout: Duration::from_millis(timeout_ms),
            client,
            connected: false,
            receiver: Some(rx),
            event_sender: tx,
        }
    }

    fn build_headers(&self) -> reqwest::header::HeaderMap {
        let mut map = reqwest::header::HeaderMap::new();
        for (key, value) in &self.headers {
            if let Ok(name) = reqwest::header::HeaderName::from_bytes(key.as_bytes()) {
                if let Ok(val) = reqwest::header::HeaderValue::from_str(value) {
                    map.insert(name, val);
                }
            }
        }
        map
    }
}

#[async_trait]
impl McpTransport for HttpSseTransport {
    async fn connect(&mut self) -> Result<(), TransportError> {
        let sse_url = if self.url.ends_with("/sse") {
            self.url.clone()
        } else {
            format!("{}/sse", self.url.trim_end_matches('/'))
        };

        let request = self
            .client
            .get(&sse_url)
            .headers(self.build_headers())
            .build()
            .map_err(|e| TransportError::Connection(format!("Failed to build request: {e}")))?;

        let response = self
            .client
            .execute(request)
            .await
            .map_err(|e| TransportError::Connection(format!("SSE connection failed: {e}")))?;

        let status = response.status();
        if !status.is_success() {
            let msg = if status.as_u16() == 401 {
                "SSE connection returned 401 Unauthorized — server may need authentication"
                    .to_string()
            } else if status.as_u16() == 403 {
                "SSE connection returned 403 Forbidden — check credentials".to_string()
            } else {
                format!("SSE connection returned {status}")
            };
            return Err(TransportError::Connection(msg));
        }

        let tx = self.event_sender.clone();
        let _timeout_dur = self.timeout;
        let sse_url_for_spawn = sse_url.clone();

        tokio::spawn(async move {
            let mut stream = response.bytes_stream();
            let mut buffer = String::new();
            let mut event_type = String::new();
            let mut total_bytes: usize = 0;

            use futures_util::StreamExt;
            while let Some(chunk_result) = stream.next().await {
                match chunk_result {
                    Ok(chunk) => {
                        total_bytes += chunk.len();
                        if total_bytes > SSE_MAX_FRAME_SIZE {
                            warn!("SSE stream exceeded 16MB cap from {sse_url_for_spawn}");
                            break;
                        }
                        let chunk_str = String::from_utf8_lossy(&chunk);
                        buffer.push_str(&chunk_str);
                        if buffer.len() > SSE_MAX_FRAME_SIZE {
                            warn!("SSE buffer exceeded 16MB cap from {sse_url_for_spawn}");
                            break;
                        }

                        while let Some(line_end) = buffer.find('\n') {
                            let line = buffer[..line_end].trim().to_string();
                            buffer = buffer[line_end + 1..].to_string();

                            if let Some(stripped) = line.strip_prefix("event: ") {
                                event_type = stripped.to_string();
                            } else if let Some(stripped) = line.strip_prefix("data: ") {
                                let data = stripped.to_string();
                                if (event_type == "message" || event_type.is_empty())
                                    && tx.send(data).await.is_err()
                                {
                                    return;
                                }
                                event_type.clear();
                            }
                        }
                    }
                    Err(e) => {
                        debug!("SSE stream error: {e}");
                        break;
                    }
                }
            }
        });

        self.connected = true;
        info!("MCP SSE server connected to {sse_url}");
        Ok(())
    }

    async fn send(&mut self, message: &str) -> Result<(), TransportError> {
        let message_url = self
            .url
            .strip_suffix("/sse")
            .unwrap_or(&self.url)
            .to_string();

        let request = self
            .client
            .post(&message_url)
            .headers(self.build_headers())
            .header("Content-Type", "application/json")
            .body(message.to_string())
            .build()
            .map_err(|e| TransportError::Send(format!("Failed to build POST: {e}")))?;

        let response = self
            .client
            .execute(request)
            .await
            .map_err(|e| TransportError::Send(format!("POST failed: {e}")))?;

        let status = response.status();
        if !status.is_success() {
            let msg = if status.as_u16() == 401 {
                "POST returned 401 Unauthorized — server may need authentication".to_string()
            } else if status.as_u16() == 403 {
                "POST returned 403 Forbidden — check credentials".to_string()
            } else {
                format!("POST returned {status}")
            };
            return Err(TransportError::Send(msg));
        }

        Ok(())
    }

    async fn receive(&mut self) -> Result<String, TransportError> {
        let rx = self.receiver.as_mut().ok_or(TransportError::NotConnected)?;

        match timeout(self.timeout, rx.recv()).await {
            Ok(Some(line)) => Ok(line),
            Ok(None) => {
                self.connected = false;
                Err(TransportError::Connection("SSE stream ended".into()))
            }
            Err(_) => Err(TransportError::Timeout),
        }
    }

    async fn close(&mut self) -> Result<(), TransportError> {
        self.connected = false;
        Ok(())
    }

    fn is_connected(&self) -> bool {
        self.connected
    }
}

pub struct WebSocketTransport {
    url: String,
    headers: HashMap<String, String>,
    timeout: Duration,
    connected: bool,
    outgoing_tx: Option<mpsc::UnboundedSender<String>>,
    receiver: Option<mpsc::Receiver<String>>,
    task_handle: Option<tokio::task::JoinHandle<()>>,
}

impl WebSocketTransport {
    pub fn new(config: &McpServerConfig) -> Self {
        let timeout_ms = config.timeout.map(|t| t.max(1000)).unwrap_or(60_000);

        Self {
            url: config.url.clone().unwrap_or_default(),
            headers: config.headers.clone(),
            timeout: Duration::from_millis(timeout_ms),
            connected: false,
            outgoing_tx: None,
            receiver: None,
            task_handle: None,
        }
    }
}

#[async_trait]
impl McpTransport for WebSocketTransport {
    async fn connect(&mut self) -> Result<(), TransportError> {
        let trimmed = self.url.trim();
        if !trimmed.starts_with("ws://") && !trimmed.starts_with("wss://") {
            return Err(TransportError::Connection(format!(
                "Invalid WebSocket URL '{}': must start with ws:// or wss://",
                self.url
            )));
        }

        let mut request =
            tokio_tungstenite::tungstenite::client::IntoClientRequest::into_client_request(trimmed)
                .map_err(|e| {
                    TransportError::Connection(format!("Failed to build WS request: {e}"))
                })?;

        let headers = request.headers_mut();
        for (key, value) in &self.headers {
            if let Ok(name) = reqwest::header::HeaderName::from_bytes(key.as_bytes()) {
                if let Ok(val) = reqwest::header::HeaderValue::from_str(value) {
                    headers.insert(name, val);
                }
            }
        }

        let (ws_stream, _response) =
            tokio::time::timeout(self.timeout, tokio_tungstenite::connect_async(request))
                .await
                .map_err(|_| TransportError::Timeout)?
                .map_err(|e| {
                    TransportError::Connection(format!("WebSocket connection failed: {e}"))
                })?;

        let (mut write, read) = ws_stream.split();

        let (outgoing_tx, outgoing_rx) = mpsc::unbounded_channel::<String>();
        let (incoming_tx, incoming_rx) = mpsc::channel::<String>(256);
        let url_for_spawn = self.url.clone();

        let handle = tokio::spawn(async move {
            let mut read = read;
            let mut outgoing_rx = outgoing_rx;

            loop {
                tokio::select! {
                    biased;

                    msg = outgoing_rx.recv() => {
                        match msg {
                            Some(text) => {
                                if write.send(tokio_tungstenite::tungstenite::Message::Text(text)).await.is_err() {
                                    warn!("WebSocket write failed for {url_for_spawn}");
                                    break;
                                }
                            }
                            None => {
                                let _ = write.close().await;
                                break;
                            }
                        }
                    }

                    frame = read.next() => {
                        match frame {
                            Some(Ok(msg)) => {
                                let text = if msg.is_text() {
                                    msg.to_text().ok().map(|s| s.to_string())
                                } else if msg.is_binary() {
                                    msg.to_text().ok().map(|s| s.to_string())
                                } else if msg.is_close() {
                                    debug!("WebSocket close frame received from {url_for_spawn}");
                                    break;
                                } else {
                                    None
                                };
                                if let Some(t) = text {
                                    if incoming_tx.send(t).await.is_err() {
                                        break;
                                    }
                                }
                            }
                            Some(Err(e)) => {
                                debug!("WebSocket read error from {url_for_spawn}: {e}");
                                break;
                            }
                            None => {
                                debug!("WebSocket stream ended for {url_for_spawn}");
                                break;
                            }
                        }
                    }
                }
            }
        });

        self.outgoing_tx = Some(outgoing_tx);
        self.receiver = Some(incoming_rx);
        self.task_handle = Some(handle);
        self.connected = true;

        info!("MCP WebSocket server connected to {}", self.url);
        Ok(())
    }

    async fn send(&mut self, message: &str) -> Result<(), TransportError> {
        let tx = self
            .outgoing_tx
            .as_ref()
            .ok_or(TransportError::NotConnected)?;

        tx.send(message.to_string())
            .map_err(|_| TransportError::Send("WebSocket send channel closed".into()))
    }

    async fn receive(&mut self) -> Result<String, TransportError> {
        let rx = self.receiver.as_mut().ok_or(TransportError::NotConnected)?;

        match timeout(self.timeout, rx.recv()).await {
            Ok(Some(line)) => Ok(line),
            Ok(None) => {
                self.connected = false;
                Err(TransportError::Connection(
                    "WebSocket connection closed".into(),
                ))
            }
            Err(_) => Err(TransportError::Timeout),
        }
    }

    async fn close(&mut self) -> Result<(), TransportError> {
        self.connected = false;
        self.outgoing_tx = None;
        self.receiver = None;
        if let Some(handle) = self.task_handle.take() {
            handle.abort();
        }
        info!("MCP WebSocket server '{}' disconnected", self.url);
        Ok(())
    }

    fn is_connected(&self) -> bool {
        self.connected
    }
}

impl Drop for WebSocketTransport {
    fn drop(&mut self) {
        if let Some(handle) = self.task_handle.take() {
            handle.abort();
        }
    }
}

pub struct McpTransportFactory;

impl McpTransportFactory {
    pub fn create(config: &McpServerConfig) -> Box<dyn McpTransport> {
        match config.transport {
            TransportType::Stdio => Box::new(StdioTransport::new(config)),
            TransportType::Sse | TransportType::Http => Box::new(HttpSseTransport::new(config)),
            TransportType::WebSocket => Box::new(WebSocketTransport::new(config)),
        }
    }
}

#[derive(Debug)]
pub enum TransportError {
    Connection(String),
    Send(String),
    Receive(String),
    Timeout,
    NotConnected,
    Protocol(String),
}

impl std::fmt::Display for TransportError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Connection(msg) => write!(f, "Connection error: {msg}"),
            Self::Send(msg) => write!(f, "Send error: {msg}"),
            Self::Receive(msg) => write!(f, "Receive error: {msg}"),
            Self::Timeout => write!(f, "Transport timeout"),
            Self::NotConnected => write!(f, "Not connected"),
            Self::Protocol(msg) => write!(f, "Protocol error: {msg}"),
        }
    }
}

impl std::error::Error for TransportError {}
