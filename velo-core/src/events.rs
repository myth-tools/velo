//! Serializable event types that cross the Rust ↔ Tauri ↔ TypeScript boundary.
//! All types derive `Serialize` so Tauri can emit them as JSON payloads.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

// ── Token streaming ────────────────────────────────────────────────────────────

/// Incremental text chunk emitted by the NIM SSE stream.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NimStreamChunk {
    pub request_id: Uuid,
    /// Partial token text.
    pub delta: String,
    /// `true` when the stream is finished.
    pub done: bool,
}

// ── Task lifecycle ─────────────────────────────────────────────────────────────

/// Current execution status of a task.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum TaskStatus {
    Pending,
    Running,
    Reflecting,
    Succeeded,
    Failed,
    Cancelled,
}

/// A single task record shown in the dashboard timeline.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskRecord {
    pub id: Uuid,
    pub description: String,
    pub status: TaskStatus,
    pub started_at: DateTime<Utc>,
    pub finished_at: Option<DateTime<Utc>>,
    /// Ordered list of step summaries.
    pub steps: Vec<StepRecord>,
}

/// A single ReAct step within a task.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StepRecord {
    pub id: Uuid,
    pub kind: StepKind,
    pub content: String,
    pub timestamp: DateTime<Utc>,
}

/// Classification of a ReAct step.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StepKind {
    Thought,
    ToolCall,
    ToolResult,
    Reflection,
    FinalAnswer,
}

// ── Destructive action intercept ───────────────────────────────────────────────

/// Severity classification for an action.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum RiskLevel {
    Low,
    Medium,
    High,
    Critical,
}

/// Emitted when the agent wants to perform a destructive action.
/// The frontend must show a confirmation modal and reply via Tauri command.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DestructiveActionRequest {
    pub action_id: Uuid,
    pub task_id: Uuid,
    pub risk_level: RiskLevel,
    /// Human-readable description of what will happen.
    pub description: String,
    /// The exact tool call that triggered the intercept.
    pub tool_name: String,
    pub tool_args: serde_json::Value,
}

// ── Suggestion (clipboard awakening) ──────────────────────────────────────────

/// A proactive suggestion derived from clipboard content analysis.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SuggestionReady {
    pub id: Uuid,
    /// Short headline for the suggestion pill.
    pub headline: String,
    /// Full suggestion text / fix shown on expand.
    pub body: String,
    /// The original clipboard snippet that triggered the analysis (truncated).
    pub trigger_snippet: String,
}

// ── STT transcript ─────────────────────────────────────────────────────────────

/// Emitted by the STT worker with a transcription segment.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SttTranscript {
    /// `true` while the segment is still being refined.
    pub partial: bool,
    pub text: String,
    pub timestamp: DateTime<Utc>,
}

// ── Voice recording state ──────────────────────────────────────────────────────

/// Simple voice UI state update.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VoiceStateUpdate {
    pub recording: bool,
    /// Normalised audio level 0.0–1.0 for visualiser.
    pub level: f32,
}

// ── Tauri event names (type-safe string constants) ─────────────────────────────

pub mod event_names {
    pub const NIM_STREAM_CHUNK: &str = "nim-stream-chunk";
    pub const TASK_STATUS_UPDATE: &str = "task-status-update";
    pub const DESTRUCTIVE_ACTION_INTERCEPT: &str = "destructive-action-intercept";
    pub const SUGGESTION_READY: &str = "suggestion-ready";
    pub const STT_TRANSCRIPT: &str = "stt-transcript";
    pub const VOICE_STATE_UPDATE: &str = "voice-state-update";
}
