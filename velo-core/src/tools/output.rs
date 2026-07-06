use serde::Serialize;
use serde_json::Value;

/// Structured output for tools.
///
/// Tools construct this instead of raw `Value::String(...)`. It serialises as a
/// JSON object with `stdout`/`stderr`/`exit_code`/`interrupted`/`truncated`
/// fields so the hook middleware can inspect structured data, plus a `display`
/// field for human-readable fallback.
#[derive(Debug, Clone, Serialize)]
pub struct ToolOutput {
    #[serde(default)]
    pub stdout: String,
    #[serde(default)]
    pub stderr: String,
    #[serde(default)]
    pub exit_code: i32,
    #[serde(default)]
    pub interrupted: bool,
    #[serde(default)]
    pub truncated: bool,
    /// Human-readable display string (e.g. for task records).
    pub display: String,
}

// ── Constructors ──────────────────────────────────────────────────────────

impl ToolOutput {
    /// Successful result with only stdout (most tools).
    pub fn ok(msg: impl Into<String>) -> Self {
        let msg = msg.into();
        Self {
            display: msg.clone(),
            stdout: msg,
            stderr: String::new(),
            exit_code: 0,
            interrupted: false,
            truncated: false,
        }
    }

    /// Failure result – message goes to stderr + display.
    pub fn err(msg: impl Into<String>) -> Self {
        let msg = msg.into();
        Self {
            display: msg.clone(),
            stdout: String::new(),
            stderr: msg,
            exit_code: 1,
            interrupted: false,
            truncated: false,
        }
    }

    /// Full shell-style output with separate stdout / stderr / exit code.
    pub fn shell(stdout: impl Into<String>, stderr: impl Into<String>, exit_code: i32) -> Self {
        let stdout = stdout.into();
        let stderr = stderr.into();
        let display = Self::format_shell(&stdout, &stderr, exit_code);
        Self {
            display,
            stdout,
            stderr,
            exit_code,
            interrupted: false,
            truncated: false,
        }
    }

    /// Mark the output as truncated (e.g. due to size limits).
    pub fn with_truncated(mut self) -> Self {
        self.truncated = true;
        self
    }

    /// Mark the output as interrupted (e.g. process killed).
    pub fn with_interrupted(mut self) -> Self {
        self.interrupted = true;
        self
    }

    fn format_shell(stdout: &str, stderr: &str, exit_code: i32) -> String {
        let mut out = format!("Exit code: {exit_code}\n");
        if !stdout.is_empty() {
            out.push_str("STDOUT:\n");
            out.push_str(stdout);
            out.push('\n');
        }
        if !stderr.is_empty() {
            out.push_str("STDERR:\n");
            out.push_str(stderr);
            out.push('\n');
        }
        out.trim_end().to_string()
    }
}

impl From<ToolOutput> for Value {
    fn from(o: ToolOutput) -> Self {
        let display = o.display.clone();
        serde_json::to_value(o).unwrap_or(Value::String(display))
    }
}
