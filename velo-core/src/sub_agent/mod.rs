pub mod media_analysis;

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc, RwLock,
};
use std::time::Duration;

use async_trait::async_trait;
use serde::Deserialize;
use tokio::sync::mpsc;

use crate::config::VeloConfig;

// ── Constants ───────────────────────────────────────────────────────────────

/// Maximum nesting depth for sub-agents (5 levels).
const MAX_SUBAGENT_DEPTH: u32 = 5;

/// Maximum agent file size we'll parse (1 MiB).
const MAX_AGENT_FILE_SIZE: u64 = 1024 * 1024;

// ── Agent metadata (from YAML frontmatter in agents/*.md) ───────────────────

/// Represents the YAML frontmatter parsed from an agent .md file.
#[derive(Debug, Clone, Deserialize)]
pub struct SubAgentMetadata {
    pub name: String,
    pub description: String,

    #[serde(default)]
    pub version: Option<String>,

    #[serde(default)]
    pub author: Option<String>,

    #[serde(default)]
    pub tags: Vec<String>,

    #[serde(default)]
    pub model: Option<String>,

    /// Explicit tool allow-list. If present, agent can ONLY use these tools.
    #[serde(default)]
    pub tools: Vec<String>,

    /// Colour for display (blue/cyan/green/yellow/magenta/red).
    #[serde(default)]
    pub color: Option<String>,

    /// Input schema as raw JSON string.
    #[serde(default)]
    pub input_schema: Option<String>,

    /// Output schema as raw JSON string.
    #[serde(default)]
    pub output_schema: Option<String>,
}

impl SubAgentMetadata {
    pub fn parse(content: &str) -> Option<Self> {
        let trimmed = content.trim_start();
        if !trimmed.starts_with("---") {
            return None;
        }
        let end = trimmed[3..].find("---")?;
        let frontmatter = &trimmed[3..3 + end];

        serde_yaml::from_str(frontmatter).ok()
    }
}

// ── Spawn parameters ────────────────────────────────────────────────────────

/// Parameters passed to a sub-agent at spawn time.
#[derive(Debug, Clone)]
pub struct SpawnParams {
    /// Current nesting depth (0 = root agent).
    pub depth: u32,
    /// Maximum allowed nesting depth.
    pub max_depth: u32,
    /// Optional timeout for the entire sub-agent execution.
    pub timeout: Option<Duration>,
    /// ID of the parent agent (for tracing/attribution).
    pub parent_id: Option<String>,
    /// Tools explicitly allowed for this agent (empty = all tools).
    pub allowed_tools: Vec<String>,
    /// Tools explicitly disallowed for this agent.
    pub disallowed_tools: Vec<String>,
    /// Model override for this agent.
    pub model_override: Option<String>,
}

impl Default for SpawnParams {
    fn default() -> Self {
        Self {
            depth: 0,
            max_depth: MAX_SUBAGENT_DEPTH,
            timeout: None,
            parent_id: None,
            allowed_tools: Vec::new(),
            disallowed_tools: Vec::new(),
            model_override: None,
        }
    }
}

impl SpawnParams {
    /// Create params for a child/spawned agent (depth + 1).
    pub fn for_child(&self) -> Result<SpawnParams, SubAgentError> {
        if self.depth >= self.max_depth {
            return Err(SubAgentError::DepthLimit(format!(
                "Max sub-agent depth ({}) reached",
                self.max_depth
            )));
        }
        Ok(SpawnParams {
            depth: self.depth + 1,
            max_depth: self.max_depth,
            timeout: self.timeout,
            parent_id: self.parent_id.clone(),
            allowed_tools: self.allowed_tools.clone(),
            disallowed_tools: self.disallowed_tools.clone(),
            model_override: self.model_override.clone(),
        })
    }
}

// ── Progress reporting ─────────────────────────────────────────────────────

/// A sender that sub-agents can use to report progress / partial results.
#[derive(Debug, Clone)]
pub struct ProgressSender {
    tx: mpsc::UnboundedSender<String>,
}

impl ProgressSender {
    pub fn new(tx: mpsc::UnboundedSender<String>) -> Self {
        Self { tx }
    }

    /// Report progress. Returns an error if the receiver has been dropped
    /// (parent no longer listening).
    pub fn send(&self, msg: String) -> Result<(), SubAgentError> {
        self.tx
            .send(msg)
            .map_err(|_| SubAgentError::Internal("Progress receiver dropped".into()))
    }
}

/// A receiver for consuming sub-agent progress messages.
pub type ProgressReceiver = mpsc::UnboundedReceiver<String>;

/// Create a progress channel pair.
pub fn progress_channel() -> (ProgressSender, ProgressReceiver) {
    let (tx, rx) = mpsc::unbounded_channel();
    (ProgressSender { tx }, rx)
}

// ── Agent metadata (descriptive) ────────────────────────────────────────────

/// Metadata describing a registered sub-agent.
#[derive(Debug, Clone)]
pub struct AgentInfo {
    pub name: String,
    pub description: String,
    pub version: Option<String>,
    pub author: Option<String>,
    pub tags: Vec<String>,
    pub model: Option<String>,
    pub tools: Vec<String>,
    pub color: Option<String>,
    pub input_schema: Option<String>,
    pub output_schema: Option<String>,
    /// Whether this agent was discovered from the filesystem (vs registered in code).
    pub filesystem: bool,
}

impl AgentInfo {
    pub fn new(name: impl Into<String>, description: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            description: description.into(),
            version: None,
            author: None,
            tags: Vec::new(),
            model: None,
            tools: Vec::new(),
            color: None,
            input_schema: None,
            output_schema: None,
            filesystem: false,
        }
    }
}

// ── Cancellation ────────────────────────────────────────────────────────────

/// Lightweight cancellation flag passed to every sub-agent execution.
#[derive(Clone, Debug)]
pub struct CancelToken(Arc<AtomicBool>);

impl CancelToken {
    pub fn new() -> Self {
        Self(Arc::new(AtomicBool::new(false)))
    }

    pub fn cancel(&self) {
        self.0.store(true, Ordering::SeqCst);
    }

    pub fn is_cancelled(&self) -> bool {
        self.0.load(Ordering::SeqCst)
    }
}

impl Default for CancelToken {
    fn default() -> Self {
        Self::new()
    }
}

// ── Error types ─────────────────────────────────────────────────────────────

/// Error type for sub-agent operations.
#[derive(Debug, thiserror::Error)]
pub enum SubAgentError {
    #[error("Sub-agent '{0}' not found")]
    NotFound(String),
    #[error("Execution failed: {0}")]
    Execution(String),
    #[error("Media error: {0}")]
    Media(String),
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
    #[error("HTTP error: {0}")]
    Http(String),
    #[error("Parse error: {0}")]
    Parse(String),
    #[error("Timed out")]
    TimedOut,
    #[error("Cancelled")]
    Cancelled,
    #[error("Depth limit: {0}")]
    DepthLimit(String),
    #[error("Tool not allowed: {0}")]
    ToolNotAllowed(String),
    #[error("Validation error: {0}")]
    Validation(String),
    #[error("Internal error: {0}")]
    Internal(String),
}

impl SubAgentError {
    pub fn state_label(&self) -> &'static str {
        match self {
            Self::TimedOut | Self::Cancelled => "cancelled",
            _ => "error",
        }
    }

    pub fn is_retryable(&self) -> bool {
        matches!(
            self,
            Self::Http(_) | Self::Io(_) | Self::TimedOut | Self::Execution(_)
        )
    }
}

// ── SubAgent trait ──────────────────────────────────────────────────────────

/// A sub-agent that can be delegated specialized tasks.
///
/// Each sub-agent is a self-contained execution unit with its own model, tools,
/// and reasoning. The main agent delegates via the Task tool and receives a
/// text result.
#[async_trait]
pub trait SubAgent: Send + Sync {
    /// Unique name used in the Task tool's `subagent_name` parameter.
    fn name(&self) -> &str;

    /// Human-readable description for the main LLM to know when to delegate.
    fn description(&self) -> &str;

    /// Structured metadata about this agent.
    fn info(&self) -> AgentInfo {
        AgentInfo::new(self.name(), self.description())
    }

    /// Execute a task with full spawn parameters.
    ///
    /// The default implementation wraps `execute()` with timeout enforcement
    /// and cancellation checking. Implementations SHOULD override this when
    /// they need access to spawn params (depth, tool scoping, progress, etc.).
    async fn execute_with_params(
        &self,
        prompt: &str,
        config: &VeloConfig,
        params: SpawnParams,
        cancel: CancelToken,
        _progress: Option<ProgressSender>,
    ) -> Result<String, SubAgentError> {
        if cancel.is_cancelled() {
            return Err(SubAgentError::Cancelled);
        }

        // Check depth limit
        if params.depth >= params.max_depth {
            return Err(SubAgentError::DepthLimit(format!(
                "Max sub-agent depth ({}) reached",
                params.max_depth
            )));
        }

        // Apply timeout if specified
        if let Some(timeout_dur) = params.timeout {
            tokio::time::timeout(timeout_dur, self.execute(prompt, config, cancel))
                .await
                .map_err(|_| SubAgentError::TimedOut)?
        } else {
            self.execute(prompt, config, cancel).await
        }
    }

    /// Execute a task and return the result text (simple interface).
    async fn execute(
        &self,
        prompt: &str,
        config: &VeloConfig,
        cancel: CancelToken,
    ) -> Result<String, SubAgentError>;
}

// ── Filesystem agent wrapper ────────────────────────────────────────────────

/// Wraps agent metadata parsed from a .md file so it can be registered
/// as a SubAgent. Delegates execution to an inner agent if one is provided,
/// or returns a descriptive error.
pub struct FilesystemAgent {
    metadata: SubAgentMetadata,
    source_path: PathBuf,
}

impl FilesystemAgent {
    pub fn new(metadata: SubAgentMetadata, source_path: PathBuf) -> Self {
        Self {
            metadata,
            source_path,
        }
    }

    pub fn source_path(&self) -> &Path {
        &self.source_path
    }
}

#[async_trait]
impl SubAgent for FilesystemAgent {
    fn name(&self) -> &str {
        &self.metadata.name
    }

    fn description(&self) -> &str {
        &self.metadata.description
    }

    fn info(&self) -> AgentInfo {
        AgentInfo {
            name: self.metadata.name.clone(),
            description: self.metadata.description.clone(),
            version: self.metadata.version.clone(),
            author: self.metadata.author.clone(),
            tags: self.metadata.tags.clone(),
            model: self.metadata.model.clone(),
            tools: self.metadata.tools.clone(),
            color: self.metadata.color.clone(),
            input_schema: self.metadata.input_schema.clone(),
            output_schema: self.metadata.output_schema.clone(),
            filesystem: true,
        }
    }

    async fn execute(
        &self,
        prompt: &str,
        _config: &VeloConfig,
        cancel: CancelToken,
    ) -> Result<String, SubAgentError> {
        if cancel.is_cancelled() {
            return Err(SubAgentError::Cancelled);
        }
        // Read the agent's .md file and use its body as the system prompt,
        // then delegate to a generic LLM call with the user's prompt.
        let source = tokio::fs::read_to_string(&self.source_path).await?;
        let body = strip_frontmatter(&source);

        // For now, return a structured result with the system prompt context.
        // This can be upgraded to spawn an actual LLM call.
        Ok(format!(
            "Agent: {}\n\nSystem Prompt:\n{}\n\nUser Task:\n{}\n\n\
             (Filesystem agents require an LLM executor to produce actual results)",
            self.name(),
            body,
            prompt,
        ))
    }
}

fn strip_frontmatter(content: &str) -> String {
    let trimmed = content.trim_start();
    if !trimmed.starts_with("---") {
        return content.to_string();
    }
    let after_first = &trimmed[3..];
    if let Some(end) = after_first.find("---") {
        after_first[end + 3..].trim().to_string()
    } else {
        content.to_string()
    }
}

// ── Registry ────────────────────────────────────────────────────────────────

/// Thread-safe registry of sub-agents.
#[derive(Clone, Default)]
pub struct SubAgentRegistry {
    agents: Arc<RwLock<HashMap<String, Arc<dyn SubAgent>>>>,
    agent_order: Arc<RwLock<Vec<String>>>,
}

impl SubAgentRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn register(&self, agent: Arc<dyn SubAgent>) {
        let name = agent.name().to_string();
        let mut agents = self.agents.write().unwrap();
        if !agents.contains_key(&name) {
            let mut order = self.agent_order.write().unwrap();
            order.push(name.clone());
        }
        agents.insert(name, agent);
    }

    pub fn get(&self, name: &str) -> Option<Arc<dyn SubAgent>> {
        self.agents.read().unwrap().get(name).cloned()
    }

    /// List all registered sub-agents with their structured info.
    pub fn list(&self) -> Vec<AgentInfo> {
        let guard = self.agents.read().unwrap();
        let order = self.agent_order.read().unwrap();
        let mut infos: Vec<AgentInfo> = order
            .iter()
            .filter_map(|name| guard.get(name).map(|a| a.info()))
            .collect();
        infos.sort_by(|a, b| a.name.cmp(&b.name));
        infos
    }

    /// Check whether a tool name is allowed for a given agent.
    /// If the agent specifies `tools`, only those tools are allowed.
    /// If the agent specifies `disallowed_tools`, those are blocked.
    pub fn is_tool_allowed(&self, agent_name: &str, tool_name: &str) -> bool {
        let guard = self.agents.read().unwrap();
        let Some(agent) = guard.get(agent_name) else {
            return true; // unknown agent, allow
        };
        let info = agent.info();

        // If tools list is non-empty, it's an allow-list
        if !info.tools.is_empty()
            && !info
                .tools
                .iter()
                .any(|t| t == tool_name || tool_name.starts_with(&format!("{t}::")) || t == "*")
        {
            return false;
        }

        true
    }

    /// Discover agents from a directory of .md files (upward walk).
    /// Returns the number of agents discovered.
    pub fn discover_from(&self, dirs: &[PathBuf]) -> usize {
        let mut count = 0;
        for dir in dirs {
            if !dir.is_dir() {
                continue;
            }
            if let Ok(read_dir) = dir.read_dir() {
                for entry in read_dir.flatten() {
                    let path = entry.path();
                    if path.extension().and_then(|e| e.to_str()) != Some("md") {
                        continue;
                    }
                    if path.file_name().and_then(|n| n.to_str()) == Some("README.md") {
                        continue;
                    }
                    let Ok(meta) = path.metadata() else { continue };
                    if meta.len() > MAX_AGENT_FILE_SIZE {
                        continue;
                    }
                    let Ok(content) = std::fs::read_to_string(&path) else {
                        continue;
                    };
                    let Some(metadata) = SubAgentMetadata::parse(&content) else {
                        continue;
                    };
                    let agent = Arc::new(FilesystemAgent::new(metadata, path));
                    self.register(agent);
                    count += 1;
                }
            }
        }
        count
    }
}

// ── Agent directory discovery ───────────────────────────────────────────────

/// Find potential agent directories by walking upward from cwd.
/// Scans `<dir>/agents/` for each ancestor directory.
pub fn find_agent_directories(cwd: &Path) -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    let home = std::env::var("HOME")
        .or_else(|_| std::env::var("USERPROFILE"))
        .ok()
        .map(PathBuf::from);

    let mut current = Some(cwd);
    let mut depth: u32 = 0;
    const MAX_WALK_DEPTH: u32 = 64;

    while let Some(dir) = current {
        if depth >= MAX_WALK_DEPTH {
            break;
        }

        // Scan <dir>/agents/
        let agents_dir = dir.join("agents");
        if agents_dir.is_dir() {
            dirs.push(agents_dir);
        }

        // Scan <dir>/.velo/agents/
        let velo_agents_dir = dir.join(".velo").join("agents");
        if velo_agents_dir.is_dir() {
            dirs.push(velo_agents_dir);
        }

        if let Some(ref home) = home {
            if dir == home.as_path() {
                break;
            }
        }

        current = dir.parent();
        depth += 1;
    }

    dirs.reverse();
    dirs
}

/// Find user-level agent directories (~/.velo/agents/, ~/.agents/agents/, etc.).
pub fn find_user_agent_directories() -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    let home = std::env::var("HOME")
        .or_else(|_| std::env::var("USERPROFILE"))
        .ok()
        .map(PathBuf::from);

    if let Some(ref home) = home {
        let candidates = [
            home.join(".velo").join("agents"),
            home.join(".agents").join("agents"),
        ];
        for c in &candidates {
            if c.is_dir() {
                dirs.push(c.clone());
            }
        }
    }

    dirs
}

/// Convenience: discover agents from all standard locations.
pub fn discover_all_agents() -> SubAgentRegistry {
    let registry = SubAgentRegistry::new();

    // Register built-in agents first
    registry.register(Arc::new(media_analysis::MediaAnalysisSubAgent));

    // Discover from standard directories
    if let Ok(cwd) = std::env::current_dir() {
        let project_dirs = find_agent_directories(&cwd);
        registry.discover_from(&project_dirs);

        let user_dirs = find_user_agent_directories();
        registry.discover_from(&user_dirs);
    }

    registry
}

// ── Legacy convenience ──────────────────────────────────────────────────────

/// Convenience function to create the default set of sub-agents.
pub fn default_sub_agents() -> SubAgentRegistry {
    let registry = SubAgentRegistry::new();
    registry.register(Arc::new(media_analysis::MediaAnalysisSubAgent));
    registry
}
