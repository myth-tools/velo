pub mod bootstrap;
pub mod builtin;
pub mod manager;
pub mod trait_def;

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use ractor::{rpc::CallResult, Actor, ActorRef, RpcReplyPort};
use serde::Serialize;
use serde_json::Value;
use thiserror::Error;

pub use builtin::register_default_skills;
pub(crate) use manager::SkillManagerActor;
pub use trait_def::{
    AgentSkill, BundledScript, ExecutionContext, ReferenceDoc, ResourceFile, SkillContext,
    SkillPriority, SkillSource, TriggerExample, ValidationReport,
};

use crate::memory::StorageBackend;

#[derive(Debug, Error)]
pub enum SkillError {
    #[error("Validation error: {0}")]
    Validation(String),

    #[error("Execution error: {0}")]
    Execution(String),

    #[error("Skill timed out after {0:?}")]
    Timeout(Duration),

    #[error("Skill panicked: {0}")]
    Panic(String),

    #[error("Skill not found: {0}")]
    NotFound(String),

    #[error("Skill already registered: {0}")]
    AlreadyRegistered(String),

    #[error("Dependency not satisfied: {0}")]
    Dependency(String),

    #[error("Resource error: {0}")]
    Resource(String),

    #[error("Pre-execution check failed: {0}")]
    PreCondition(String),

    #[error("Execution cancelled: {0}")]
    Cancelled(String),

    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),

    #[error("Serialization error: {0}")]
    Serialization(#[from] serde_json::Error),

    #[error("Internal error: {0}")]
    Internal(String),
}

#[derive(Debug, Clone, Serialize)]
pub struct SkillMetrics {
    pub name: String,
    pub version: &'static str,
    pub invocation_count: u64,
    pub total_execution_time_ns: u128,
    pub error_count: u64,
    pub last_execution_elapsed_ns: Option<u128>,
    pub average_execution_time_ns: Option<u128>,
}

impl SkillMetrics {
    pub fn new(name: String, version: &'static str) -> Self {
        Self {
            name,
            version,
            invocation_count: 0,
            total_execution_time_ns: 0,
            error_count: 0,
            last_execution_elapsed_ns: None,
            average_execution_time_ns: None,
        }
    }

    pub fn record(&mut self, start: Instant, result: &Result<Value, SkillError>) {
        let elapsed = start.elapsed();
        let elapsed_ns = elapsed.as_nanos();
        self.invocation_count += 1;
        self.total_execution_time_ns += elapsed_ns;
        self.last_execution_elapsed_ns = Some(elapsed_ns);
        self.average_execution_time_ns =
            Some(self.total_execution_time_ns / self.invocation_count as u128);
        if result.is_err() {
            self.error_count += 1;
        }
    }
}

#[derive(Clone)]
pub struct SkillEntry {
    pub skill: Arc<dyn AgentSkill>,
    pub source: SkillSource,
    pub file_path: Option<String>,
}

impl std::fmt::Debug for SkillEntry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SkillEntry")
            .field("name", &self.skill.name())
            .field("source", &self.source)
            .field("file_path", &self.file_path)
            .finish()
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct SkillInfo {
    pub name: String,
    pub version: &'static str,
    pub author: Option<&'static str>,
    pub tags: Vec<&'static str>,
    pub description: String,
    pub trigger_examples: Vec<TriggerExample>,
    pub when_to_use: Option<String>,
    pub path_triggers: Vec<&'static str>,
    pub aliases: Vec<&'static str>,
    pub argument_hint: Option<&'static str>,
    pub allowed_tools: Option<Vec<&'static str>>,
    pub model_override: Option<&'static str>,
    pub user_invocable: bool,
    pub disable_model_invocation: bool,
    pub parameters: Value,
    pub execution_context: String,
    pub agent_override: Option<&'static str>,
    pub localized_instruction: Option<String>,
    pub reference_docs: Vec<ReferenceDoc>,
    pub bundled_scripts: Vec<BundledScript>,
    pub resource_files: Vec<ResourceFile>,
    pub default_timeout_secs: u64,
    pub priority: SkillPriority,
    pub dependencies: Vec<&'static str>,
    pub source: SkillSource,
}

#[derive(Debug, Clone, Serialize)]
pub struct SkillListing {
    pub skills: Vec<SkillListingEntry>,
    pub total_count: usize,
    pub listing_generated_at: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct SkillListingEntry {
    pub name: String,
    pub description: String,
    pub version: &'static str,
    pub tags: Vec<&'static str>,
    pub priority: SkillPriority,
    pub execution_context: String,
    pub user_invocable: bool,
    pub argument_hint: Option<&'static str>,
    pub source: SkillSource,
}

pub enum SkillMessage {
    RegisterSkill {
        skill: Arc<dyn AgentSkill>,
        source: SkillSource,
    },
    LoadSkillsFromDirectory {
        path: String,
        source: SkillSource,
        reply: RpcReplyPort<Result<Vec<String>, SkillError>>,
    },
    ExecuteSkill {
        name: String,
        arguments: Value,
        reply: RpcReplyPort<Result<Value, SkillError>>,
    },
    GetAvailableTools(RpcReplyPort<Vec<Value>>),
    GetFreshListing(RpcReplyPort<SkillListing>),
    GetMetrics(RpcReplyPort<Vec<SkillMetrics>>),
    GetSkillInfo {
        name: String,
        reply: RpcReplyPort<Result<SkillInfo, SkillError>>,
    },
    ResolveAlias {
        name: String,
        reply: RpcReplyPort<Option<String>>,
    },
    ListEnabled(RpcReplyPort<Vec<String>>),
}

#[derive(Clone)]
pub struct SkillManagerHandle {
    actor_ref: ActorRef<SkillMessage>,
}

impl SkillManagerHandle {
    pub async fn new(storage: Arc<StorageBackend>) -> Result<Self, SkillError> {
        let actor = SkillManagerActor;
        let (actor_ref, _handle) = Actor::spawn(Some("skill-manager".into()), actor, storage)
            .await
            .map_err(|e| SkillError::Internal(format!("Failed to spawn SkillManagerActor: {e}")))?;

        Ok(Self { actor_ref })
    }

    pub async fn bootstrap(&self) -> Result<(), SkillError> {
        bootstrap::bootstrap_skills(self, true).await
    }

    pub async fn bootstrap_with_extra(
        &self,
        extra_dirs: &[(PathBuf, SkillSource)],
    ) -> Result<(), SkillError> {
        bootstrap::bootstrap_skills(self, true).await?;
        for (dir, source) in extra_dirs {
            bootstrap::load_skills_from_directory_robust(self, dir, *source).await;
        }
        Ok(())
    }

    pub fn register(&self, skill: Arc<dyn AgentSkill>) {
        let _ = self.actor_ref.cast(SkillMessage::RegisterSkill {
            skill,
            source: SkillSource::Bundled,
        });
    }

    pub fn register_with_source(&self, skill: Arc<dyn AgentSkill>, source: SkillSource) {
        let _ = self
            .actor_ref
            .cast(SkillMessage::RegisterSkill { skill, source });
    }

    pub async fn load_skills_from_directory(
        &self,
        path: &str,
        source: SkillSource,
    ) -> Result<Vec<String>, SkillError> {
        let result = self
            .actor_ref
            .call(
                |reply| SkillMessage::LoadSkillsFromDirectory {
                    path: path.to_string(),
                    source,
                    reply,
                },
                Some(Duration::from_secs(30)),
            )
            .await
            .map_err(|e| SkillError::Internal(format!("Actor call failed: {e}")))?;

        match result {
            CallResult::Success(r) => r,
            CallResult::Timeout => Err(SkillError::Timeout(Duration::from_secs(30))),
            CallResult::SenderError => Err(SkillError::Internal(
                "LoadSkillsFromDirectory sender error".into(),
            )),
        }
    }

    pub async fn execute(&self, name: &str, arguments: Value) -> Result<Value, SkillError> {
        let result = self
            .actor_ref
            .call(
                |reply| SkillMessage::ExecuteSkill {
                    name: name.to_string(),
                    arguments,
                    reply,
                },
                Some(Duration::from_secs(310)),
            )
            .await
            .map_err(|e| SkillError::Internal(format!("Actor call failed: {e}")))?;

        match result {
            CallResult::Success(r) => r,
            CallResult::Timeout => Err(SkillError::Timeout(Duration::from_secs(310))),
            CallResult::SenderError => {
                Err(SkillError::Internal("Actor sender channel error".into()))
            }
        }
    }

    pub async fn get_available_tools(&self) -> Result<Vec<Value>, SkillError> {
        let result = self
            .actor_ref
            .call(
                SkillMessage::GetAvailableTools,
                Some(Duration::from_secs(5)),
            )
            .await
            .map_err(|e| SkillError::Internal(format!("Actor call failed: {e}")))?;

        match result {
            CallResult::Success(tools) => Ok(tools),
            CallResult::Timeout => Err(SkillError::Timeout(Duration::from_secs(5))),
            CallResult::SenderError => Err(SkillError::Internal(
                "GetAvailableTools sender channel error".into(),
            )),
        }
    }

    pub async fn get_fresh_listing(&self) -> Result<SkillListing, SkillError> {
        let result = self
            .actor_ref
            .call(SkillMessage::GetFreshListing, Some(Duration::from_secs(5)))
            .await
            .map_err(|e| SkillError::Internal(format!("Actor call failed: {e}")))?;

        match result {
            CallResult::Success(r) => Ok(r),
            CallResult::Timeout => Err(SkillError::Timeout(Duration::from_secs(5))),
            CallResult::SenderError => Err(SkillError::Internal(
                "GetFreshListing sender channel error".into(),
            )),
        }
    }

    pub async fn get_metrics(&self) -> Result<Vec<SkillMetrics>, SkillError> {
        let result = self
            .actor_ref
            .call(SkillMessage::GetMetrics, Some(Duration::from_secs(5)))
            .await
            .map_err(|e| SkillError::Internal(format!("Actor call failed: {e}")))?;

        match result {
            CallResult::Success(r) => Ok(r),
            CallResult::Timeout => Err(SkillError::Timeout(Duration::from_secs(5))),
            CallResult::SenderError => Err(SkillError::Internal(
                "GetMetrics sender channel error".into(),
            )),
        }
    }

    pub async fn get_skill_info(&self, name: &str) -> Result<SkillInfo, SkillError> {
        let result = self
            .actor_ref
            .call(
                |reply| SkillMessage::GetSkillInfo {
                    name: name.to_string(),
                    reply,
                },
                Some(Duration::from_secs(5)),
            )
            .await
            .map_err(|e| SkillError::Internal(format!("Actor call failed: {e}")))?;

        match result {
            CallResult::Success(r) => r,
            CallResult::Timeout => Err(SkillError::Timeout(Duration::from_secs(5))),
            CallResult::SenderError => Err(SkillError::Internal(
                "GetSkillInfo sender channel error".into(),
            )),
        }
    }

    pub async fn resolve_alias(&self, name: &str) -> Result<Option<String>, SkillError> {
        let result = self
            .actor_ref
            .call(
                |reply| SkillMessage::ResolveAlias {
                    name: name.to_string(),
                    reply,
                },
                Some(Duration::from_secs(5)),
            )
            .await
            .map_err(|e| SkillError::Internal(format!("Actor call failed: {e}")))?;

        match result {
            CallResult::Success(r) => Ok(r),
            CallResult::Timeout => Err(SkillError::Timeout(Duration::from_secs(5))),
            CallResult::SenderError => {
                Err(SkillError::Internal("ResolveAlias sender error".into()))
            }
        }
    }

    pub async fn list_enabled(&self) -> Result<Vec<String>, SkillError> {
        let result = self
            .actor_ref
            .call(SkillMessage::ListEnabled, Some(Duration::from_secs(5)))
            .await
            .map_err(|e| SkillError::Internal(format!("Actor call failed: {e}")))?;

        match result {
            CallResult::Success(r) => Ok(r),
            CallResult::Timeout => Err(SkillError::Timeout(Duration::from_secs(5))),
            CallResult::SenderError => Err(SkillError::Internal("ListEnabled sender error".into())),
        }
    }

    pub fn actor_ref(&self) -> ActorRef<SkillMessage> {
        self.actor_ref.clone()
    }
}

impl From<&SkillEntry> for SkillInfo {
    fn from(entry: &SkillEntry) -> Self {
        let skill = &*entry.skill;
        Self {
            name: skill.name().to_string(),
            version: skill.version(),
            author: skill.author(),
            tags: skill.tags(),
            description: skill.description(),
            trigger_examples: skill.trigger_examples(),
            when_to_use: skill.when_to_use(),
            path_triggers: skill.path_triggers(),
            aliases: skill.aliases(),
            argument_hint: skill.argument_hint(),
            allowed_tools: skill.allowed_tools(),
            model_override: skill.model_override(),
            user_invocable: skill.user_invocable(),
            disable_model_invocation: skill.disable_model_invocation(),
            parameters: skill.parameters(),
            execution_context: match skill.execution_context() {
                ExecutionContext::Inline => "inline",
                ExecutionContext::Fork => "fork",
            }
            .to_string(),
            agent_override: skill.agent_override(),
            localized_instruction: skill.localized_instruction(),
            reference_docs: skill.reference_material(),
            bundled_scripts: skill.bundled_scripts(),
            resource_files: skill.resource_files(),
            default_timeout_secs: skill.default_timeout().as_secs(),
            priority: skill.priority(),
            dependencies: skill.dependencies(),
            source: entry.source,
        }
    }
}

impl From<&SkillEntry> for SkillListingEntry {
    fn from(entry: &SkillEntry) -> Self {
        let skill = &*entry.skill;
        Self {
            name: skill.name().to_string(),
            description: skill.description(),
            version: skill.version(),
            tags: skill.tags(),
            priority: skill.priority(),
            execution_context: match skill.execution_context() {
                ExecutionContext::Inline => "inline",
                ExecutionContext::Fork => "fork",
            }
            .to_string(),
            user_invocable: skill.user_invocable(),
            argument_hint: skill.argument_hint(),
            source: entry.source,
        }
    }
}

fn parse_skill_yaml_frontmatter(content: &str) -> Option<HashMap<String, String>> {
    let trimmed = content.trim_start();
    if !trimmed.starts_with("---") {
        return None;
    }
    let end = trimmed[3..].find("---")?;
    let frontmatter = &trimmed[3..3 + end];
    let mut map = HashMap::new();
    for line in frontmatter.lines() {
        let line = line.trim();
        if let Some(eq) = line.find(':') {
            let key = line[..eq].trim().to_string();
            let value = line[eq + 1..].trim().to_string();
            map.insert(key, value);
        }
    }
    Some(map)
}

pub async fn load_skills_from_directory(
    path: &Path,
    _source: SkillSource,
) -> Result<Vec<(Box<dyn AgentSkill>, String)>, SkillError> {
    if !path.exists() {
        return Ok(Vec::new());
    }
    if !path.is_dir() {
        return Err(SkillError::Validation(format!(
            "Skill directory path is not a directory: {}",
            path.display()
        )));
    }

    let self_skill_md = path.join("SKILL.md");
    if self_skill_md.is_file() {
        let content = tokio::fs::read_to_string(&self_skill_md)
            .await
            .map_err(|e| {
                SkillError::Io(std::io::Error::other(format!(
                    "Failed to read SKILL.md at {}: {e}",
                    self_skill_md.display()
                )))
            })?;

        let frontmatter = parse_skill_yaml_frontmatter(&content);
        let name = frontmatter
            .as_ref()
            .and_then(|m| m.get("name").cloned())
            .or_else(|| {
                path.file_stem()
                    .and_then(|s| s.to_str())
                    .map(|s| s.to_string())
            })
            .unwrap_or_default();

        let description = frontmatter
            .as_ref()
            .and_then(|m| m.get("description").cloned())
            .unwrap_or_default();

        let skill = FilesystemSkill {
            name: name.clone(),
            description,
            content,
            frontmatter: frontmatter.unwrap_or_default(),
            base_dir: path.to_string_lossy().to_string(),
        };

        return Ok(vec![(Box::new(skill) as Box<dyn AgentSkill>, name)]);
    }

    let mut reader = tokio::fs::read_dir(path).await.map_err(|e| {
        SkillError::Io(std::io::Error::other(format!(
            "Failed to read skill directory: {e}"
        )))
    })?;

    let mut loaded = Vec::new();

    while let Some(entry) = reader.next_entry().await.map_err(|e| {
        SkillError::Io(std::io::Error::other(format!(
            "Failed to read directory entry: {e}"
        )))
    })? {
        let file_type = entry.file_type().await.map_err(|e| {
            SkillError::Io(std::io::Error::other(format!(
                "Failed to get file type: {e}"
            )))
        })?;

        if !file_type.is_dir() && !file_type.is_symlink() {
            continue;
        }

        let skill_dir = entry.path();
        let skill_md_path = skill_dir.join("SKILL.md");

        if !skill_md_path.exists() {
            continue;
        }

        let content = tokio::fs::read_to_string(&skill_md_path)
            .await
            .map_err(|e| {
                SkillError::Io(std::io::Error::other(format!(
                    "Failed to read SKILL.md at {}: {e}",
                    skill_md_path.display()
                )))
            })?;

        let frontmatter = parse_skill_yaml_frontmatter(&content);
        let name = frontmatter
            .as_ref()
            .and_then(|m| m.get("name").cloned())
            .or_else(|| {
                skill_dir
                    .file_stem()
                    .and_then(|s| s.to_str())
                    .map(|s| s.to_string())
            })
            .unwrap_or_default();

        let description = frontmatter
            .as_ref()
            .and_then(|m| m.get("description").cloned())
            .unwrap_or_default();

        let skill = FilesystemSkill {
            name: name.clone(),
            description,
            content,
            frontmatter: frontmatter.unwrap_or_default(),
            base_dir: skill_dir.to_string_lossy().to_string(),
        };

        loaded.push((Box::new(skill) as Box<dyn AgentSkill>, name));
    }

    Ok(loaded)
}

struct FilesystemSkill {
    name: String,
    description: String,
    content: String,
    frontmatter: HashMap<String, String>,
    base_dir: String,
}

#[async_trait::async_trait]
impl AgentSkill for FilesystemSkill {
    fn name(&self) -> &'static str {
        Box::leak(self.name.clone().into_boxed_str())
    }

    fn description(&self) -> String {
        self.description.clone()
    }

    fn parameters(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "properties": {}
        })
    }

    fn when_to_use(&self) -> Option<String> {
        self.frontmatter.get("when_to_use").cloned()
    }

    fn path_triggers(&self) -> Vec<&'static str> {
        self.frontmatter
            .get("paths")
            .map(|p| {
                p.split(',')
                    .map(|s| Box::leak(s.trim().to_string().into_boxed_str()) as &'static str)
                    .collect()
            })
            .unwrap_or_default()
    }

    fn aliases(&self) -> Vec<&'static str> {
        self.frontmatter
            .get("aliases")
            .map(|a| {
                a.split(',')
                    .map(|s| Box::leak(s.trim().to_string().into_boxed_str()) as &'static str)
                    .collect()
            })
            .unwrap_or_default()
    }

    fn argument_hint(&self) -> Option<&'static str> {
        self.frontmatter
            .get("argument-hint")
            .map(|s| Box::leak(s.clone().into_boxed_str()) as &'static str)
    }

    fn allowed_tools(&self) -> Option<Vec<&'static str>> {
        self.frontmatter.get("allowed-tools").map(|t| {
            t.split(',')
                .map(|s| Box::leak(s.trim().to_string().into_boxed_str()) as &'static str)
                .collect()
        })
    }

    fn model_override(&self) -> Option<&'static str> {
        self.frontmatter
            .get("model")
            .map(|s| Box::leak(s.clone().into_boxed_str()) as &'static str)
    }

    fn user_invocable(&self) -> bool {
        self.frontmatter
            .get("user-invocable")
            .map(|v| v == "true")
            .unwrap_or(true)
    }

    fn disable_model_invocation(&self) -> bool {
        self.frontmatter
            .get("disable-model-invocation")
            .map(|v| v == "true")
            .unwrap_or(false)
    }

    fn execution_context(&self) -> ExecutionContext {
        match self.frontmatter.get("context").map(|s| s.as_str()) {
            Some("fork") => ExecutionContext::Fork,
            _ => ExecutionContext::Inline,
        }
    }

    fn agent_override(&self) -> Option<&'static str> {
        self.frontmatter
            .get("agent")
            .map(|s| Box::leak(s.clone().into_boxed_str()) as &'static str)
    }

    fn localized_instruction(&self) -> Option<String> {
        Some(format!(
            "Loaded disk-based skill '{}' from {}\n\n{}",
            self.name, self.base_dir, self.content
        ))
    }

    async fn execute(&self, _args: Value, _ctx: Arc<StorageBackend>) -> Result<Value, SkillError> {
        Ok(serde_json::json!({
            "skill": self.name,
            "content": self.content,
            "base_dir": self.base_dir,
        }))
    }
}
