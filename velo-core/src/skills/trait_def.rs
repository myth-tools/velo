use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use serde::Serialize;
use serde_json::Value;

use super::SkillError;
use crate::memory::StorageBackend;

#[derive(Debug, Clone, Serialize)]
pub struct TriggerExample {
    pub pattern: &'static str,
    pub description: &'static str,
}

#[derive(Debug, Clone, Serialize)]
pub struct ReferenceDoc {
    pub name: &'static str,
    pub description: &'static str,
    pub content: &'static str,
}

#[derive(Debug, Clone, Serialize)]
pub struct BundledScript {
    pub name: &'static str,
    pub language: &'static str,
    pub description: &'static str,
    pub code: &'static str,
}

#[derive(Debug, Clone, Serialize)]
pub struct ResourceFile {
    pub name: &'static str,
    pub content: &'static str,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
pub enum SkillPriority {
    Low,
    Normal,
    High,
    Critical,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
pub enum ExecutionContext {
    Inline,
    Fork,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
pub enum SkillSource {
    Bundled,
    User,
    Project,
    Plugin,
    MCP,
}

#[derive(Debug, Clone)]
pub struct ValidationReport {
    pub is_valid: bool,
    pub errors: Vec<String>,
    pub warnings: Vec<String>,
}

impl Default for ValidationReport {
    fn default() -> Self {
        Self {
            is_valid: true,
            errors: Vec::new(),
            warnings: Vec::new(),
        }
    }
}

impl ValidationReport {
    pub fn error(msg: impl Into<String>) -> Self {
        Self {
            is_valid: false,
            errors: vec![msg.into()],
            warnings: Vec::new(),
        }
    }

    pub fn with_error(mut self, msg: impl Into<String>) -> Self {
        self.is_valid = false;
        self.errors.push(msg.into());
        self
    }

    pub fn with_warning(mut self, msg: impl Into<String>) -> Self {
        self.warnings.push(msg.into());
        self
    }

    pub fn merge(mut self, other: Self) -> Self {
        self.is_valid = self.is_valid && other.is_valid;
        self.errors.extend(other.errors);
        self.warnings.extend(other.warnings);
        self
    }
}

pub struct SkillContext {
    pub storage: Arc<StorageBackend>,
}

impl SkillContext {
    pub fn new(storage: Arc<StorageBackend>) -> Self {
        Self { storage }
    }
}

#[async_trait]
pub trait AgentSkill: Send + Sync {
    fn name(&self) -> &'static str;

    fn version(&self) -> &'static str {
        "0.1.0"
    }

    fn author(&self) -> Option<&'static str> {
        None
    }

    fn tags(&self) -> Vec<&'static str> {
        vec![]
    }

    fn description(&self) -> String;

    fn trigger_examples(&self) -> Vec<TriggerExample> {
        vec![]
    }

    fn when_to_use(&self) -> Option<String> {
        None
    }

    fn path_triggers(&self) -> Vec<&'static str> {
        vec![]
    }

    fn aliases(&self) -> Vec<&'static str> {
        vec![]
    }

    fn argument_hint(&self) -> Option<&'static str> {
        None
    }

    fn allowed_tools(&self) -> Option<Vec<&'static str>> {
        None
    }

    fn model_override(&self) -> Option<&'static str> {
        None
    }

    fn parameters(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "properties": {}
        })
    }

    fn user_invocable(&self) -> bool {
        true
    }

    fn disable_model_invocation(&self) -> bool {
        false
    }

    fn is_enabled(&self) -> bool {
        true
    }

    fn execution_context(&self) -> ExecutionContext {
        ExecutionContext::Inline
    }

    fn agent_override(&self) -> Option<&'static str> {
        None
    }

    fn default_timeout(&self) -> Duration {
        Duration::from_secs(300)
    }

    fn priority(&self) -> SkillPriority {
        SkillPriority::Normal
    }

    fn dependencies(&self) -> Vec<&'static str> {
        vec![]
    }

    fn localized_instruction(&self) -> Option<String> {
        None
    }

    fn reference_material(&self) -> Vec<ReferenceDoc> {
        vec![]
    }

    fn bundled_scripts(&self) -> Vec<BundledScript> {
        vec![]
    }

    fn resource_files(&self) -> Vec<ResourceFile> {
        vec![]
    }

    fn validate(&self) -> ValidationReport {
        ValidationReport::default()
    }

    async fn on_register(&self, _ctx: &SkillContext) -> Result<(), SkillError> {
        Ok(())
    }

    async fn execute(&self, args: Value, ctx: Arc<StorageBackend>) -> Result<Value, SkillError>;
}
