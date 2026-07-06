use std::collections::HashMap;
use std::sync::{Arc, OnceLock, RwLock};
use std::time::Duration;

use async_trait::async_trait;
use autoagents::core::tool::{ToolCallError, ToolInputT, ToolRuntime, ToolT};
use chrono::Utc;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use uuid::Uuid;

use crate::sub_agent::{CancelToken, SubAgentRegistry};
use crate::tools::{exec_err, ToolOutput};

// ── Recursion guard ────────────────────────────────────────────────────────

use std::cell::Cell;

thread_local! {
    static TASK_DEPTH: Cell<u32> = const { Cell::new(0) };
}

struct RecursionGuard;

impl RecursionGuard {
    fn new() -> Result<Self, ToolCallError> {
        if TASK_DEPTH.get() > 0 {
            return Err(exec_err(
                "Recursive task() calls are not allowed. Sub-agents cannot launch other sub-agents.",
            ));
        }
        TASK_DEPTH.set(1);
        Ok(RecursionGuard)
    }
}

impl Drop for RecursionGuard {
    fn drop(&mut self) {
        TASK_DEPTH.set(0);
    }
}

// ── Tool args ─────────────────────────────────────────────────────────────

#[derive(Serialize, Deserialize, Debug)]
pub struct TaskArgs {
    /// Short (3-5 words) summary, used for logging and tracking.
    pub description: String,
    /// The detailed task for the sub-agent. Include file paths and context.
    pub prompt: String,
    /// Which sub-agent to invoke.
    pub subagent_name: String,
    /// Resume a previous session by passing its task_id from an earlier call.
    #[serde(default)]
    pub task_id: Option<String>,
    /// What command triggered this task (tracking / auditing).
    #[serde(default)]
    pub command: Option<String>,
}

impl ToolInputT for TaskArgs {
    fn io_schema() -> &'static str {
        r#"{"type":"object","properties":{"description":{"type":"string","description":"A short (3-5 words) summary of what this sub-agent task is for. Used for logging and tracking."},"prompt":{"type":"string","description":"The detailed task for the sub-agent to perform. Include file paths, specific questions, and any context the sub-agent needs. For best results, provide a self-contained prompt that tells the sub-agent exactly what information to return."},                "subagent_name":{"type":"string","description":"Which sub-agent to invoke. Use \"media_analysis\" for images, audio, video, PDFs. See the DECISION TREE in the tool description above to determine when to call task()."},"task_id":{"type":"string","description":"Optional. Pass a previous task_id to resume that session and get the result again, or to continue an incomplete task. Only set this when the sub-agent explicitly requested follow-up or when you need to re-read a previous result."},"command":{"type":"string","description":"Optional. The command or action that triggered this task. Used for auditing and tracking purposes."}},"required":["description","prompt","subagent_name"]}"#
    }
}

// ── Tool struct (no #[tool] macro — we implement ToolT manually so the
//    description can be built dynamically from the registry at startup) ─────

#[derive(Default, Clone, Debug)]
pub struct TaskTool;

impl ToolT for TaskTool {
    fn name(&self) -> &str {
        "task"
    }

    fn description(&self) -> &str {
        TASK_TOOL_DESC
            .get()
            .map(|s| s.as_str())
            .unwrap_or("(sub-agent system initialising)")
    }

    fn args_schema(&self) -> Value {
        serde_json::from_str(TaskArgs::io_schema()).unwrap()
    }
}

// ── ToolRuntime ───────────────────────────────────────────────────────────

#[async_trait]
impl ToolRuntime for TaskTool {
    async fn execute(&self, args: Value) -> Result<Value, ToolCallError> {
        let a: TaskArgs = serde_json::from_value(args)?;

        // 1. Prevent recursive delegation — sub-agents cannot call task().
        let _guard = RecursionGuard::new()?;

        // 2. Resolve session (existing or new).
        let cancel = CancelToken::new();
        let session_id = a.task_id.clone().unwrap_or_else(generate_session_id);

        if let Some(cached) = try_get_cached(&session_id) {
            return Ok(
                ToolOutput::ok(structure_output(&cached, None).as_str().unwrap_or("")).into(),
            );
        }

        // 3. Look up sub-agent.
        let registry = sub_agent_registry();
        let agent = registry.get(&a.subagent_name).ok_or_else(|| {
            exec_err(format!(
                "Unknown sub-agent '{}'. Available: {}",
                a.subagent_name,
                registry
                    .list()
                    .iter()
                    .map(|i| &i.name)
                    .cloned()
                    .collect::<Vec<_>>()
                    .join(", ")
            ))
        })?;

        // 4. Mark session as running.
        let session = TaskSession {
            id: session_id.clone(),
            state: "running".into(),
            subagent_name: a.subagent_name.clone(),
            description: a.description.clone(),
            prompt: a.prompt.clone(),
            command: a.command.clone(),
            result: None,
            error: None,
            created_at: Utc::now().to_rfc3339(),
            updated_at: Utc::now().to_rfc3339(),
        };
        store_session(session);

        // 5. Execute with timeout.
        let config = crate::tools::config();
        let timeout_secs = 150u64;
        let result = tokio::time::timeout(
            Duration::from_secs(timeout_secs),
            agent.execute(&a.prompt, config, cancel.clone()),
        )
        .await;

        // 6. Handle result.
        let (state, output, error) = match result {
            Ok(Ok(text)) => ("completed", Some(text.clone()), None),
            Ok(Err(e)) => ("error", None, Some(e.to_string())),
            Err(_) => {
                cancel.cancel();
                (
                    "error",
                    None,
                    Some(format!("Timed out after {timeout_secs}s")),
                )
            }
        };

        // 7. Update session.
        update_session(&session_id, state, output.clone(), error.clone());

        Ok(ToolOutput::ok(
            structure_output(
                &TaskSession {
                    id: session_id,
                    state: state.into(),
                    subagent_name: a.subagent_name,
                    description: a.description,
                    prompt: a.prompt,
                    command: a.command,
                    result: output,
                    error,
                    created_at: String::new(),
                    updated_at: Utc::now().to_rfc3339(),
                },
                None,
            )
            .as_str()
            .unwrap_or(""),
        )
        .into())
    }
}

// ── Structured XML output ─────────────────────────────────────────────────

fn structure_output(session: &TaskSession, _summary: Option<&str>) -> Value {
    let tag = if session.state == "error" {
        "task_error"
    } else {
        "task_result"
    };

    let body = session
        .result
        .as_deref()
        .unwrap_or(session.error.as_deref().unwrap_or("(no output)"));

    Value::String(format!(
        r#"<task id="{}" state="{}">
<{} task_id="{}" subagent="{}">
{}
</{}>
</task>"#,
        session.id, session.state, tag, session.id, session.subagent_name, body, tag,
    ))
}

// ── Dynamic tool description ──────────────────────────────────────────────

static TASK_TOOL_DESC: OnceLock<String> = OnceLock::new();

pub fn init_task_tool_description(registry: &SubAgentRegistry) {
    let agents = registry.list();
    let mut desc = String::new();

    desc.push_str(
        "Use `task()` ONLY when the request involves content the main text model cannot process \
         directly: images, photos, screenshots (that weren't just captured), audio recordings, \
         voice memos, video files, or PDFs.\n\n",
    );

    desc.push_str(
        "DECISION TREE — before calling task(), ask yourself:\n\n\
         1. Is there a file path involved?\n\
            NO  → Do NOT use task(). Handle the request with your native text tools.\n\
            YES → Continue to 2.\n\n\
         2. Is the file a TEXT file (`.txt`, `.md`, `.rs`, `.py`, `.json`, `.csv`, `.toml`, etc.)?\n\
            YES → Use `read_file` directly — do NOT use task().\n\
            NO  → Continue to 3.\n\n\
         3. Does the file match one of these types?\n\
            Image (`.jpg`, `.png`, `.gif`, `.bmp`, `.webp`, `.tiff`, `.ico`, `.avif`, `.heic`)\n\
            Audio (`.mp3`, `.wav`, `.ogg`, `.flac`, `.m4a`, `.aac`, `.wma`)\n\
            Video (`.mp4`, `.avi`, `.mov`, `.mkv`, `.wmv`, `.flv`, `.webm`)\n\
            PDF   (`.pdf`)\n\
            YES → Use task() with subagent_name=\"media_analysis\". Include the full path.\n\
            NO  → Continue to 4.\n\n\
         4. The file has an unknown extension. Use `read_file` to check its contents first.\n\
            If it appears to be binary data (images/audio/video/PDF by magic bytes), \
            then call task() with media_analysis.\n\n"
    );

    if agents.is_empty() {
        desc.push_str("No sub-agents are currently available.\n\n");
    } else {
        desc.push_str("Available sub-agents:\n");
        let max_name = agents.iter().map(|a| a.name.len()).max().unwrap_or(0);
        for agent in &agents {
            let pad = " ".repeat(max_name.saturating_sub(agent.name.len()) + 2);
            desc.push_str(&format!("  - {}{}{}\n", agent.name, pad, agent.description));
            if let Some(model) = &agent.model {
                desc.push_str(&format!("    Model: {model}\n"));
            }
        }
        desc.push('\n');
    }

    desc.push_str(
        "USAGE NOTES:\n\
         1. Set `description` to a short (3-5 word) label (e.g. \"analyze photo\").\n\
         2. Write a detailed `prompt` — the sub-agent gets this verbatim. Include the \
         full file path(s).\n\
         3. Each file path should be on its own line in the prompt.\n\
         4. OPTIONAL: Pass `task_id` from a previous call to get the cached result.\n\
         5. OPTIONAL: Set `command` for audit tracking.\n\
         6. NEVER call `task()` from within a sub-agent (causes infinite loops).\n\
         7. The sub-agent returns structured XML. Parse <task_result> for success \
         or <task_error> for failure.\n\
         8. You MAY run multiple sub-agents concurrently in a single turn.\n\
         9. Do NOT duplicate work — if you call task() for something, don't also \
         analyze it yourself.\n\
         10. The sub-agent handles file-size limits internally. If a file is too large, \
         it returns a clear error with guidance.\n\
         11. For videos: frames are extracted and analyzed in parallel. Audio track is \
         also transcribed if present.",
    );

    TASK_TOOL_DESC.set(desc).ok();
}

// ── Session store ─────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
struct TaskSession {
    id: String,
    state: String,
    subagent_name: String,
    description: String,
    prompt: String,
    command: Option<String>,
    result: Option<String>,
    error: Option<String>,
    created_at: String,
    updated_at: String,
}

static SESSION_STORE: OnceLock<Arc<RwLock<HashMap<String, TaskSession>>>> = OnceLock::new();

fn session_store() -> &'static Arc<RwLock<HashMap<String, TaskSession>>> {
    SESSION_STORE.get_or_init(|| Arc::new(RwLock::new(HashMap::new())))
}

fn generate_session_id() -> String {
    Uuid::new_v4().to_string()
}

fn store_session(session: TaskSession) {
    let mut store = session_store().write().unwrap();
    store.insert(session.id.clone(), session);
}

fn update_session(id: &str, state: &str, result: Option<String>, error: Option<String>) {
    let mut store = session_store().write().unwrap();
    if let Some(s) = store.get_mut(id) {
        s.state = state.into();
        s.result = result;
        s.error = error;
        s.updated_at = Utc::now().to_rfc3339();
    }
}

fn try_get_cached(id: &str) -> Option<TaskSession> {
    let store = session_store().read().unwrap();
    store.get(id).cloned()
}

// ── Global registry reference ─────────────────────────────────────────────

static SUB_AGENT_REGISTRY: OnceLock<SubAgentRegistry> = OnceLock::new();

pub fn init_sub_agent_registry(registry: SubAgentRegistry) {
    SUB_AGENT_REGISTRY
        .set(registry)
        .unwrap_or_else(|_| panic!("init_sub_agent_registry called more than once"));
}

pub(crate) fn sub_agent_registry() -> &'static SubAgentRegistry {
    SUB_AGENT_REGISTRY
        .get()
        .expect("SubAgentRegistry not initialised — call init_sub_agent_registry before use")
}
