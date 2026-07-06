use async_trait::async_trait;
use autoagents::core::agent::task::Task;
use autoagents::core::agent::{AgentHooks, Context, HookOutcome};
use autoagents::core::tool::ToolCallResult;
use autoagents::llm::ToolCall;
use chrono::Utc;
use serde_json::Value;
use tokio::sync::mpsc;
use tracing::{error, info, warn};
use uuid::Uuid;

use crate::agent::{VeloAgent, VeloOutput};
use crate::events::{event_names, NimStreamChunk, StepKind, StepRecord, TaskRecord, TaskStatus};
use crate::tools::check_destructive;

#[async_trait]
impl AgentHooks for VeloAgent {
    async fn on_run_start(&self, task: &Task, _ctx: &Context) -> HookOutcome {
        let request_id = Uuid::new_v4();
        info!(%request_id, task = %task.prompt, "Run started");

        if let Some(hooks) = &self.hooks {
            *hooks.pending_task_id.lock().await = Some(request_id);
            let record = TaskRecord {
                id: request_id,
                description: task.prompt.clone(),
                status: TaskStatus::Running,
                started_at: Utc::now(),
                finished_at: None,
                steps: vec![],
            };
            if let Ok(payload) = serde_json::to_value(record) {
                let _ = hooks
                    .event_tx
                    .send((event_names::TASK_STATUS_UPDATE.into(), payload));
            }

            if let Some(memory) = &hooks.memory {
                if let Err(e) = memory
                    .store_task(request_id, None, task.prompt.clone(), "InProgress".into())
                    .await
                {
                    error!("Failed to store task in memory: {e}");
                }
                let _ = memory.update_working_state(
                    Some(request_id),
                    Some(task.prompt.clone()),
                    None,
                    None,
                );
            }
        }

        HookOutcome::Continue
    }

    async fn on_run_complete(&self, task: &Task, result: &VeloOutput, _ctx: &Context) {
        info!("Run completed, response length: {}", result.response.len());
        if let Some(hooks) = &self.hooks {
            let task_id = *hooks.pending_task_id.lock().await;
            *hooks.pending_task_id.lock().await = None;

            if let Some(memory) = &hooks.memory {
                if let Some(id) = task_id {
                    if let Err(e) = memory.update_task_status(id, "Success".into()).await {
                        error!("Failed to update task status: {e}");
                    }

                    let text = format!(
                        "Task complete: {} → {}",
                        task.prompt,
                        result.response.chars().take(500).collect::<String>(),
                    );
                    let metadata = serde_json::json!({
                        "type": "task_completion",
                        "task_id": id.to_string(),
                    });
                    if let Err(e) = memory.store_embedding(text, metadata.to_string()).await {
                        error!("Failed to store task embedding: {e}");
                    }
                }
            }

            if let Ok(payload) = serde_json::to_value(NimStreamChunk {
                request_id: Uuid::nil(),
                delta: result.response.clone(),
                done: false,
            }) {
                let _ = hooks
                    .event_tx
                    .send((event_names::NIM_STREAM_CHUNK.into(), payload));
            }
            if let Ok(payload) = serde_json::to_value(NimStreamChunk {
                request_id: Uuid::nil(),
                delta: String::new(),
                done: true,
            }) {
                let _ = hooks
                    .event_tx
                    .send((event_names::NIM_STREAM_CHUNK.into(), payload));
            }
        }
    }

    async fn on_turn_start(&self, turn_index: usize, _ctx: &Context) {
        info!("Turn {turn_index} starting");
    }

    async fn on_turn_complete(&self, _turn_index: usize, _ctx: &Context) {}

    async fn on_tool_call(&self, tool_call: &ToolCall, _ctx: &Context) -> HookOutcome {
        let Some(hooks) = &self.hooks else {
            return HookOutcome::Continue;
        };

        let args: Value =
            serde_json::from_str(&tool_call.function.arguments).unwrap_or(Value::Null);

        let Some(mut intercept) = check_destructive(&tool_call.function.name, &args) else {
            return HookOutcome::Continue;
        };

        warn!(
            risk = ?intercept.risk_level,
            tool = %tool_call.function.name,
            "Intercepting destructive action"
        );

        if let Some(task_id) = *hooks.pending_task_id.lock().await {
            intercept.task_id = task_id;
        }

        if let Ok(payload) = serde_json::to_value(&intercept) {
            let _ = hooks
                .event_tx
                .send((event_names::DESTRUCTIVE_ACTION_INTERCEPT.into(), payload));
        }

        let (response_tx, mut response_rx) = mpsc::unbounded_channel::<bool>();
        let action_id = intercept.action_id;
        let _ = hooks.approval_tx.send((action_id, response_tx));

        match tokio::time::timeout(std::time::Duration::from_secs(300), response_rx.recv()).await {
            Ok(Some(true)) => {
                info!(%action_id, "User approved destructive action");
                HookOutcome::Continue
            }
            Ok(Some(false)) => {
                warn!(%action_id, "User rejected");
                HookOutcome::Abort
            }
            Ok(None) => {
                warn!(%action_id, "Channel closed");
                HookOutcome::Abort
            }
            Err(_) => {
                warn!(%action_id, "Timeout (300s)");
                HookOutcome::Abort
            }
        }
    }

    async fn on_tool_start(&self, tool_call: &ToolCall, _ctx: &Context) {
        info!(
            "Tool: {}({})",
            tool_call.function.name, tool_call.function.arguments
        );
        if let Some(hooks) = &self.hooks {
            if let Some(task_id) = *hooks.pending_task_id.lock().await {
                let step = TaskRecord {
                    id: task_id,
                    description: String::new(),
                    status: TaskStatus::Running,
                    started_at: Utc::now(),
                    finished_at: None,
                    steps: vec![StepRecord {
                        id: Uuid::new_v4(),
                        kind: StepKind::ToolCall,
                        content: format!(
                            "{}({:?})",
                            tool_call.function.name, tool_call.function.arguments
                        ),
                        timestamp: Utc::now(),
                    }],
                };
                if let Ok(payload) = serde_json::to_value(step) {
                    let _ = hooks
                        .event_tx
                        .send((event_names::TASK_STATUS_UPDATE.into(), payload));
                }
            }

            if let Some(memory) = &hooks.memory {
                let _ = memory.update_working_state(
                    None,
                    None,
                    Some(format!(
                        "{}({})",
                        tool_call.function.name, tool_call.function.arguments
                    )),
                    None,
                );
            }
        }
    }

    async fn on_tool_result(&self, tool_call: &ToolCall, result: &ToolCallResult, _ctx: &Context) {
        info!("Tool done: {} success={}", result.tool_name, result.success);
        if let Some(hooks) = &self.hooks {
            if let Some(task_id) = *hooks.pending_task_id.lock().await {
                let content = match &result.result {
                    Value::Null => "(no result)".into(),
                    other => other.to_string(),
                };
                let step = TaskRecord {
                    id: task_id,
                    description: String::new(),
                    status: TaskStatus::Running,
                    started_at: Utc::now(),
                    finished_at: None,
                    steps: vec![StepRecord {
                        id: Uuid::new_v4(),
                        kind: StepKind::ToolResult,
                        content,
                        timestamp: Utc::now(),
                    }],
                };
                if let Ok(payload) = serde_json::to_value(step) {
                    let _ = hooks
                        .event_tx
                        .send((event_names::TASK_STATUS_UPDATE.into(), payload));
                }

                if let Some(memory) = &hooks.memory {
                    let log_id = Uuid::new_v4();
                    let exit_code = if result.success { 0 } else { 1 };
                    let input_payload = tool_call.function.arguments.clone();
                    let output_payload = result.result.to_string();

                    if let Err(e) = memory
                        .store_execution_log(
                            log_id,
                            task_id,
                            tool_call.function.name.clone(),
                            input_payload,
                            output_payload,
                            exit_code,
                        )
                        .await
                    {
                        error!("Failed to store execution log: {e}");
                    }

                    let _ = memory.append_session_message(
                        "assistant".into(),
                        serde_json::json!({
                            "tool": tool_call.function.name,
                            "arguments": tool_call.function.arguments,
                        })
                        .to_string(),
                    );
                    let _ = memory.append_session_message("tool".into(), result.result.to_string());

                    let _ = memory.update_working_state(
                        None,
                        None,
                        None,
                        Some(result.result.to_string()),
                    );

                    let embed_text = format!(
                        "Tool {}: {} → {}",
                        tool_call.function.name,
                        tool_call
                            .function
                            .arguments
                            .chars()
                            .take(200)
                            .collect::<String>(),
                        result
                            .result
                            .to_string()
                            .chars()
                            .take(300)
                            .collect::<String>(),
                    );
                    let embed_meta = serde_json::json!({
                        "type": "tool_result",
                        "task_id": task_id.to_string(),
                        "tool_name": tool_call.function.name,
                        "success": result.success,
                    });
                    if let Err(e) = memory
                        .store_embedding(embed_text, embed_meta.to_string())
                        .await
                    {
                        error!("Failed to store tool embedding: {e}");
                    }
                }
            }
        }
    }

    async fn on_tool_error(&self, tool_call: &ToolCall, err: serde_json::Value, _ctx: &Context) {
        error!("Tool error on {}: {err}", tool_call.function.name);
        if let Some(hooks) = &self.hooks {
            if let Some(memory) = &hooks.memory {
                if let Some(task_id) = *hooks.pending_task_id.lock().await {
                    let log_id = Uuid::new_v4();
                    if let Err(e) = memory
                        .store_execution_log(
                            log_id,
                            task_id,
                            tool_call.function.name.clone(),
                            tool_call.function.arguments.clone(),
                            err.to_string(),
                            -1,
                        )
                        .await
                    {
                        error!("Failed to store error execution log: {e}");
                    }
                }
            }
        }
    }
}
