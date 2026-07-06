use async_trait::async_trait;
use autoagents::core::tool::{ToolCallError, ToolInputT, ToolRuntime, ToolT};
use autoagents_derive::tool;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::tools::{exec_err, ToolOutput};

#[derive(Serialize, Deserialize, Debug)]
pub struct NotifyArgs {
    pub summary: String,
    pub body: Option<String>,
    pub urgency: Option<String>,
    pub timeout_ms: Option<i32>,
}

impl ToolInputT for NotifyArgs {
    fn io_schema() -> &'static str {
        r#"{"type":"object","properties":{"summary":{"type":"string","description":"Notification title (required). Briefly describes the notification purpose."},"body":{"type":"string","description":"Optional notification body text with details (supports plain text only, no HTML)."},"urgency":{"type":"string","description":"Urgency level: 'low' — subtle, no sound; 'normal' (default) — standard notification; 'critical' — urgent, may stay on screen."},"timeout_ms":{"type":"integer","description":"Display duration in milliseconds. Default: system setting. Values ≤ 0 use system default. Note: macOS ignores this setting."}}}"#
    }
}

#[tool(name = "notify", description = "Send a desktop notification to the user. Falls back to stderr print on headless/SSH/Docker systems. Cross-platform (Linux via D-Bus, macOS Notification Center, Windows Toast). BEST FOR: alerting the user when a long task completes, requesting attention, reporting errors. Use the separate watch_path tool for FILE SYSTEM event monitoring.", input = NotifyArgs)]
#[derive(Default, Clone)]
pub struct NotifyTool;

#[async_trait]
impl ToolRuntime for NotifyTool {
    async fn execute(&self, args: Value) -> Result<Value, ToolCallError> {
        let a: NotifyArgs = serde_json::from_value(args)?;
        let summary = a.summary;
        let body = a.body;
        let urgency = a.urgency.unwrap_or_else(|| "normal".into());
        let timeout_ms = a.timeout_ms;

        let result = tokio::task::spawn_blocking(move || -> Result<String, String> {
            // Try desktop notification; fall back to stderr print
            let send_result = try_desktop_notify(&summary, body.as_deref(), &urgency, timeout_ms);

            match send_result {
                Ok(()) => Ok("Notification sent.".into()),
                Err(e) => {
                    // Fallback: print to stderr
                    let level = match urgency.as_str() {
                        "critical" => "CRITICAL",
                        "low" => "LOW",
                        _ => "NORMAL",
                    };
                    eprintln!("[{level}] {summary}");
                    if let Some(ref b) = body {
                        eprintln!("  {b}");
                    }
                    eprintln!("  (desktop notification unavailable: {e})");
                    Ok("Notification sent via stderr fallback.".into())
                }
            }
        })
        .await
        .map_err(|e| exec_err(format!("Spawn: {e}")))?;

        Ok(ToolOutput::ok(result.map_err(exec_err)?).into())
    }
}

#[cfg(any(
    target_os = "linux",
    target_os = "macos",
    target_os = "windows",
    target_os = "freebsd",
    target_os = "openbsd",
    target_os = "netbsd"
))]
fn try_desktop_notify(
    summary: &str,
    body: Option<&str>,
    urgency: &str,
    timeout_ms: Option<i32>,
) -> Result<(), String> {
    let mut notification = notify_rust::Notification::new();
    notification.summary(summary);

    if let Some(b) = body {
        notification.body(b);
    }

    let urg = match urgency {
        "low" => notify_rust::Urgency::Low,
        "critical" => notify_rust::Urgency::Critical,
        _ => notify_rust::Urgency::Normal,
    };
    notification.urgency(urg);

    if let Some(timeout) = timeout_ms {
        if timeout > 0 {
            notification.timeout(notify_rust::Timeout::Milliseconds(timeout as u32));
        }
    }

    notification.show().map_err(|e| format!("{e}"))?;
    Ok(())
}

#[cfg(not(any(
    target_os = "linux",
    target_os = "macos",
    target_os = "windows",
    target_os = "freebsd",
    target_os = "openbsd",
    target_os = "netbsd"
)))]
fn try_desktop_notify(
    _summary: &str,
    _body: Option<&str>,
    _urgency: &str,
    _timeout_ms: Option<i32>,
) -> Result<(), String> {
    Err("Desktop notifications not supported on this platform".into())
}
