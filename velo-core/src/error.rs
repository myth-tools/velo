//! Typed error hierarchy for velo-core.

use thiserror::Error;

/// Top-level error type for all Velo operations.
#[derive(Debug, Error)]
pub enum VeloError {
    // ── Configuration ──────────────────────────────────────────────────────────
    #[error("Configuration error: {0}")]
    Config(String),

    #[error("YAML config error: {0}")]
    Yaml(String),

    #[error("Missing required configuration: {0}")]
    MissingConfig(String),

    // ── NIM / HTTP ─────────────────────────────────────────────────────────────
    #[error("HTTP request failed: {0}")]
    Http(#[from] reqwest::Error),

    #[error("SSE stream error: {0}")]
    SseStream(String),

    #[error("NIM API error {status}: {body}")]
    NimApi { status: u16, body: String },

    #[error("JSON (de)serialization error: {0}")]
    Json(#[from] serde_json::Error),

    // ── Actor / Runtime ────────────────────────────────────────────────────────
    #[error("Actor send error: {0}")]
    ActorSend(String),

    #[error("Actor spawn error: {0}")]
    ActorSpawn(String),

    // ── Tool Execution ─────────────────────────────────────────────────────────
    #[error("Shell command failed (exit {code}): {stderr}")]
    ShellFailed { code: i32, stderr: String },

    #[error("Shell command timed out after {secs}s")]
    ShellTimeout { secs: u64 },

    #[error("Destructive action requires user approval: {description}")]
    DestructiveActionBlocked { description: String },

    #[error("Browser error: {0}")]
    Browser(String),

    #[error("Screen capture error: {0}")]
    ScreenCapture(String),

    #[error("Window control error: {0}")]
    WindowControl(String),

    #[error("Clipboard error: {0}")]
    Clipboard(String),

    #[error("File operation error: {0}")]
    FileOp(#[from] std::io::Error),

    // ── Audio / STT ────────────────────────────────────────────────────────────
    #[error("Audio device error: {0}")]
    AudioDevice(String),

    #[error("Audio stream error: {0}")]
    AudioStream(String),

    #[error("Resampler error: {0}")]
    Resample(String),

    #[error("STT/Whisper error: {0}")]
    Stt(String),

    // ── Snapshot ───────────────────────────────────────────────────────────────
    #[error("Snapshot error: {0}")]
    Snapshot(String),

    // ── WASM Sandbox ───────────────────────────────────────────────────────────
    #[error("WASM sandbox error: {0}")]
    Wasm(String),

    // ── Generic ────────────────────────────────────────────────────────────────
    #[error(transparent)]
    Other(#[from] anyhow::Error),
}

impl VeloError {
    /// Returns `true` if this error indicates a user-visible destructive-action block.
    pub fn is_destructive_block(&self) -> bool {
        matches!(self, VeloError::DestructiveActionBlocked { .. })
    }
}
