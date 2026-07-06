use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use ractor::{Actor, ActorProcessingErr, ActorRef, RpcReplyPort};
use serde_json::Value;
use tokio_util::sync::CancellationToken;

use super::{
    load_skills_from_directory,
    trait_def::{ExecutionContext, SkillPriority},
    SkillEntry, SkillError, SkillInfo, SkillListing, SkillListingEntry, SkillMessage, SkillMetrics,
};
use crate::memory::StorageBackend;

#[derive(Clone)]
pub(crate) struct SkillManagerActor;

pub(crate) struct SkillManagerState {
    skills: Vec<SkillEntry>,
    alias_map: HashMap<String, String>,
    path_triggers: HashMap<String, Vec<String>>,
    metrics: HashMap<String, SkillMetrics>,
    storage: Arc<StorageBackend>,
}

impl SkillManagerState {
    fn new(storage: Arc<StorageBackend>) -> Self {
        Self {
            skills: Vec::new(),
            alias_map: HashMap::new(),
            path_triggers: HashMap::new(),
            metrics: HashMap::new(),
            storage,
        }
    }

    fn validate_and_register(&mut self, entry: SkillEntry) -> Result<(), SkillError> {
        let report = entry.skill.validate();
        if !report.is_valid {
            return Err(SkillError::Validation(format!(
                "Skill '{}' validation failed: {}",
                entry.skill.name(),
                report.errors.join("; ")
            )));
        }

        if !report.warnings.is_empty() {
            tracing::warn!(
                "Skill '{}' validation warnings: {}",
                entry.skill.name(),
                report.warnings.join("; ")
            );
        }

        let name = entry.skill.name().to_string();
        let deps = entry.skill.dependencies();

        for dep in &deps {
            let found = self.skills.iter().any(|e| e.skill.name() == *dep);
            if !found {
                return Err(SkillError::Dependency(format!(
                    "Skill '{name}' depends on '{dep}' which is not registered"
                )));
            }
        }

        if let Some(pos) = self.skills.iter().position(|e| e.skill.name() == name) {
            let old = &self.skills[pos];
            tracing::debug!(
                "Replacing skill '{name}' (source {:?} -> {:?})",
                old.source,
                entry.source
            );
            for alias in old.skill.aliases() {
                if self.alias_map.get(alias) == Some(&name) {
                    self.alias_map.remove(alias);
                }
            }
            self.path_triggers.remove(&name);
            self.skills.remove(pos);
        }

        for alias in entry.skill.aliases() {
            if let Some(existing) = self.alias_map.get(alias) {
                if existing != &name {
                    tracing::warn!(
                        "Alias '{alias}' already maps to '{existing}', overwriting with '{name}'"
                    );
                }
            }
            self.alias_map.insert(alias.to_string(), name.clone());
        }

        let paths = entry.skill.path_triggers();
        if !paths.is_empty() {
            self.path_triggers
                .insert(name.clone(), paths.iter().map(|p| p.to_string()).collect());
        }

        self.metrics
            .entry(name.clone())
            .or_insert_with(|| SkillMetrics::new(name.clone(), entry.skill.version()));
        self.skills.push(entry);

        Ok(())
    }

    fn find_skill(&self, name: &str) -> Option<&SkillEntry> {
        self.skills
            .iter()
            .find(|e| e.skill.name() == name)
            .or_else(|| {
                self.alias_map
                    .get(name)
                    .and_then(|resolved| self.skills.iter().find(|e| e.skill.name() == resolved))
            })
    }

    fn find_enabled_skills(&self) -> Vec<&SkillEntry> {
        self.skills
            .iter()
            .filter(|e| e.skill.is_enabled())
            .collect()
    }

    fn build_skill_listing(&self) -> SkillListing {
        let entries: Vec<SkillListingEntry> = self
            .find_enabled_skills()
            .iter()
            .map(|e| SkillListingEntry::from(*e))
            .collect();
        SkillListing {
            total_count: entries.len(),
            skills: entries,
            listing_generated_at: format!("{:?}", std::time::SystemTime::now()),
        }
    }
}

#[async_trait::async_trait]
impl Actor for SkillManagerActor {
    type State = SkillManagerState;
    type Msg = SkillMessage;
    type Arguments = Arc<StorageBackend>;

    async fn pre_start(
        &self,
        _myself: ActorRef<Self::Msg>,
        storage: Self::Arguments,
    ) -> Result<Self::State, ActorProcessingErr> {
        Ok(SkillManagerState::new(storage))
    }

    async fn handle(
        &self,
        _myself: ActorRef<Self::Msg>,
        message: Self::Msg,
        state: &mut Self::State,
    ) -> Result<(), ActorProcessingErr> {
        match message {
            SkillMessage::RegisterSkill { skill, source } => {
                let entry = SkillEntry {
                    skill,
                    source,
                    file_path: None,
                };

                match state.validate_and_register(entry) {
                    Ok(()) => (),
                    Err(e) => {
                        tracing::error!("Failed to register skill: {e}");
                    }
                }
            }

            SkillMessage::LoadSkillsFromDirectory {
                path,
                source,
                reply,
            } => {
                let path = std::path::Path::new(&path);
                let result = load_skills_from_directory(path, source).await;
                match result {
                    Ok(skills_with_names) => {
                        let names: Vec<String> = skills_with_names
                            .iter()
                            .map(|(_, name)| name.clone())
                            .collect();
                        for (skill, _name) in skills_with_names {
                            let entry = SkillEntry {
                                skill: skill.into(),
                                source,
                                file_path: Some(path.to_string_lossy().to_string()),
                            };
                            if let Err(e) = state.validate_and_register(entry) {
                                tracing::error!("Failed to register loaded skill: {e}");
                            }
                        }
                        let _ = reply.send(Ok(names));
                    }
                    Err(e) => {
                        let _ = reply.send(Err(e));
                    }
                }
            }

            SkillMessage::ExecuteSkill {
                name,
                arguments,
                reply,
            } => {
                let start = Instant::now();
                let result = self.execute_skill(state, &name, arguments, &reply).await;
                let skill_name = name.clone();
                if let Some(metrics) = state.metrics.get_mut(&skill_name) {
                    metrics.record(start, &result);
                }
                let _ = reply.send(result);
            }

            SkillMessage::GetAvailableTools(reply) => {
                let tools = self.serialize_tools(state);
                let _ = reply.send(tools);
            }

            SkillMessage::GetFreshListing(reply) => {
                let listing = state.build_skill_listing();
                let _ = reply.send(listing);
            }

            SkillMessage::GetMetrics(reply) => {
                let all_metrics: Vec<SkillMetrics> = state.metrics.values().cloned().collect();
                let _ = reply.send(all_metrics);
            }

            SkillMessage::GetSkillInfo { name, reply } => match state.find_skill(&name) {
                Some(entry) => {
                    let _ = reply.send(Ok(SkillInfo::from(entry)));
                }
                None => {
                    let _ = reply.send(Err(SkillError::NotFound(name)));
                }
            },

            SkillMessage::ResolveAlias { name, reply } => {
                let resolved = state.alias_map.get(&name).cloned();
                let _ = reply.send(resolved);
            }

            SkillMessage::ListEnabled(reply) => {
                let names: Vec<String> = state
                    .find_enabled_skills()
                    .iter()
                    .map(|e| e.skill.name().to_string())
                    .collect();
                let _ = reply.send(names);
            }
        }

        Ok(())
    }
}

impl SkillManagerActor {
    fn serialize_tools(&self, state: &SkillManagerState) -> Vec<Value> {
        state
            .find_enabled_skills()
            .iter()
            .map(|entry| {
                let skill = &*entry.skill;
                let mut tool = serde_json::json!({
                    "type": "function",
                    "function": {
                        "name": skill.name(),
                        "description": skill.description(),
                        "parameters": skill.parameters(),
                        "version": skill.version(),
                        "priority": match skill.priority() {
                            SkillPriority::Critical => "critical",
                            SkillPriority::High => "high",
                            SkillPriority::Normal => "normal",
                            SkillPriority::Low => "low",
                        },
                        "user_invocable": skill.user_invocable(),
                    }
                });

                let tags = skill.tags();
                if !tags.is_empty() {
                    tool["function"]["tags"] = serde_json::json!(tags);
                }

                let examples = skill.trigger_examples();
                if !examples.is_empty() {
                    tool["function"]["trigger_examples"] = serde_json::json!(examples);
                }

                let hint = skill.argument_hint();
                if let Some(h) = hint {
                    tool["function"]["argument_hint"] = serde_json::json!(h);
                }

                let aliases = skill.aliases();
                if !aliases.is_empty() {
                    tool["function"]["aliases"] = serde_json::json!(aliases);
                }

                let allowed = skill.allowed_tools();
                if let Some(t) = allowed {
                    tool["function"]["allowed_tools"] = serde_json::json!(t);
                }

                let model_override = skill.model_override();
                if let Some(m) = model_override {
                    tool["function"]["model_override"] = serde_json::json!(m);
                }

                let exec_ctx = match skill.execution_context() {
                    ExecutionContext::Inline => "inline",
                    ExecutionContext::Fork => "fork",
                };
                tool["function"]["execution_context"] = serde_json::json!(exec_ctx);

                let deps = skill.dependencies();
                if !deps.is_empty() {
                    tool["function"]["dependencies"] = serde_json::json!(deps);
                }

                let when_to_use = skill.when_to_use();
                if let Some(w) = when_to_use {
                    tool["function"]["when_to_use"] = serde_json::json!(w);
                }

                tool
            })
            .collect()
    }

    async fn execute_skill(
        &self,
        state: &mut SkillManagerState,
        name: &str,
        arguments: Value,
        _reply: &RpcReplyPort<Result<Value, SkillError>>,
    ) -> Result<Value, SkillError> {
        let entry = state
            .find_skill(name)
            .ok_or_else(|| SkillError::NotFound(format!("Skill '{name}' not found")))?;

        if !entry.skill.is_enabled() {
            return Err(SkillError::PreCondition(format!(
                "Skill '{name}' is disabled"
            )));
        }

        if entry.skill.disable_model_invocation() {
            return Err(SkillError::PreCondition(format!(
                "Skill '{name}' does not allow model invocation"
            )));
        }

        if let Some(allowed) = entry.skill.allowed_tools() {
            if let Some(tool_name) = arguments.get("tool_name").and_then(Value::as_str) {
                if !allowed.contains(&tool_name) {
                    return Err(SkillError::PreCondition(format!(
                        "Tool '{tool_name}' is not in allowed_tools for skill '{}'",
                        entry.skill.name()
                    )));
                }
            }
        }

        let default_timeout = entry.skill.default_timeout();
        let timeout = if default_timeout.as_secs() == 0 {
            Duration::from_secs(300)
        } else {
            default_timeout
        };

        let cancellation_token = CancellationToken::new();
        let cancel_on_drop = cancellation_token.clone();

        let skill = entry.skill.clone();
        let ctx = state.storage.clone();
        let result = tokio::select! {
            biased;

            _ = cancellation_token.cancelled() => {
                Err(SkillError::Cancelled("Skill execution cancelled".into()))
            }

            res = tokio::time::timeout(timeout, async {
                let handle = tokio::spawn(async move {
                    skill.execute(arguments, ctx).await
                });
                match handle.await {
                    Ok(inner) => inner,
                    Err(join_err) => {
                        if join_err.is_panic() {
                            let msg = if let Some(s) = join_err.into_panic().downcast_ref::<String>() {
                                s.clone()
                            } else {
                                "unknown panic".into()
                            };
                            Err(SkillError::Panic(msg))
                        } else {
                            Err(SkillError::Internal("Task join failed".into()))
                        }
                    }
                }
            }) => {
                match res {
                    Ok(r) => r,
                    Err(_elapsed) => {
                        cancellation_token.cancel();
                        Err(SkillError::Timeout(timeout))
                    }
                }
            }
        };

        drop(cancel_on_drop);
        result
    }
}
