use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use autoagents::core::tool::{ToolCallError, ToolInputT, ToolRuntime, ToolT};
use autoagents_derive::tool;
use notify::EventKind;
use notify::{Config, RecommendedWatcher, RecursiveMode, Watcher};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tokio::sync::Mutex;

use crate::tools::{exec_err, ToolOutput};

#[derive(Serialize, Deserialize, Debug)]
pub struct WatchPathArgs {
    pub path: String,
    pub events: Option<Vec<String>>,
    pub duration_secs: Option<u64>,
    pub recursive: Option<bool>,
}

impl ToolInputT for WatchPathArgs {
    fn io_schema() -> &'static str {
        r#"{"type":"object","properties":{"path":{"type":"string","description":"Directory path to monitor for file system events."},"events":{"type":"array","items":{"type":"string"},"description":"Filter by event type(s): 'create', 'modify', 'delete'. Omit to capture all event types."},"duration_secs":{"type":"integer","description":"How long to watch (in seconds). Default: 10, Max: 60."},"recursive":{"type":"boolean","description":"If true (default), watch subdirectories recursively. Set false for top-level only."}}}"#
    }
}

#[tool(name = "watch_path", description = "Watch a directory for file system events (create, modify, delete) for a configurable duration (max 60s). Returns a log of all events. BEST FOR: monitoring for new files, detecting changes, waiting for file downloads to complete. Use the separate 'notify' tool (notify_user) for desktop notifications, NOT file watching.", input = WatchPathArgs)]
#[derive(Default, Clone)]
pub struct WatchPathTool;

#[async_trait]
impl ToolRuntime for WatchPathTool {
    async fn execute(&self, args: Value) -> Result<Value, ToolCallError> {
        let a: WatchPathArgs = serde_json::from_value(args)?;
        let path = PathBuf::from(&a.path);
        if !path.exists() {
            return Err(exec_err(format!("Path does not exist: {}", a.path)));
        }
        if !path.is_dir() {
            return Err(exec_err(format!("Not a directory: {}", a.path)));
        }

        let events_filter: Option<Vec<String>> = a
            .events
            .map(|v| v.into_iter().map(|s| s.to_lowercase()).collect());
        let duration = Duration::from_secs(a.duration_secs.unwrap_or(10).min(60));
        let recursive = a.recursive.unwrap_or(true);
        let rec_mode = if recursive {
            RecursiveMode::Recursive
        } else {
            RecursiveMode::NonRecursive
        };

        let result = Arc::new(Mutex::new(Vec::new()));
        let result_clone = result.clone();

        let mut watcher = RecommendedWatcher::new(
            move |event: Result<notify::Event, notify::Error>| {
                let result = result_clone.clone();
                if let Ok(event) = event {
                    let kind = match event.kind {
                        EventKind::Create(_) => "created",
                        EventKind::Modify(_) => "modified",
                        EventKind::Remove(_) => "deleted",
                        _ => "other",
                    };

                    // Apply event type filter
                    if let Some(ref filter) = events_filter {
                        if !filter.contains(&kind.to_string()) {
                            return;
                        }
                    }

                    let paths: Vec<String> = event
                        .paths
                        .iter()
                        .map(|p| p.to_string_lossy().to_string())
                        .collect();

                    let mut guard = result.try_lock().ok();
                    if let Some(ref mut guard) = guard {
                        for p in paths {
                            guard.push(format!("[{kind}] {p}"));
                        }
                    }
                }
            },
            Config::default(),
        )
        .map_err(|e| exec_err(format!("Watcher error: {e}")))?;

        watcher
            .watch(&path, rec_mode)
            .map_err(|e| exec_err(format!("Watch failed: {e}")))?;

        tokio::time::sleep(duration).await;

        // Explicitly drop watcher to stop it
        drop(watcher);

        let events = result.lock().await;
        let output = if events.is_empty() {
            "No events recorded in the watch period.".into()
        } else {
            format!("Events ({}):\n{}", events.len(), events.join("\n"))
        };

        Ok(ToolOutput::ok(output).into())
    }
}
