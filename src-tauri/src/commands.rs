use serde_json::Value;
use tauri::{AppHandle, Emitter, Manager, State};
use uuid::Uuid;

use crate::bridge::AppState;
use velo_core::events::{event_names, NimStreamChunk};
use velo_core::snapshot::SnapshotManager;

/// Submit a text command from the UI into the ReAct loop with streaming.
#[tauri::command]
pub async fn submit_text_command(
    text: String,
    state: State<'_, AppState>,
    app: AppHandle,
) -> Result<String, String> {
    let handle = state.handle.read().await;
    let handle = handle.as_ref().ok_or("Agent not initialised")?.clone();

    let request_id = Uuid::new_v4();

    tokio::spawn(async move {
        match handle.run_stream(&text).await {
            Ok(mut stream) => {
                use futures_util::StreamExt;
                while let Some(chunk_result) = stream.next().await {
                    match chunk_result {
                        Ok(delta) if !delta.is_empty() => {
                            let payload = NimStreamChunk {
                                request_id,
                                delta,
                                done: false,
                            };
                            if let Ok(val) = serde_json::to_value(payload) {
                                let _ = app.emit(event_names::NIM_STREAM_CHUNK, val);
                            }
                        }
                        Ok(_) => {} // empty chunk (e.g., tool results)
                        Err(e) => {
                            tracing::error!("Agent stream error: {e}");
                            let payload = NimStreamChunk {
                                request_id,
                                delta: format!("Error: {e}"),
                                done: true,
                            };
                            if let Ok(val) = serde_json::to_value(payload) {
                                let _ = app.emit(event_names::NIM_STREAM_CHUNK, val);
                            }
                            break;
                        }
                    }
                }
                // Stream ended — emit final done=true
                let payload = NimStreamChunk {
                    request_id,
                    delta: String::new(),
                    done: true,
                };
                if let Ok(val) = serde_json::to_value(payload) {
                    let _ = app.emit(event_names::NIM_STREAM_CHUNK, val);
                }
            }
            Err(e) => {
                tracing::error!("Agent run_stream failed: {e}");
                let payload = NimStreamChunk {
                    request_id,
                    delta: format!("Error: {e}"),
                    done: true,
                };
                if let Ok(val) = serde_json::to_value(payload) {
                    let _ = app.emit(event_names::NIM_STREAM_CHUNK, val);
                }
            }
        }
    });

    Ok(request_id.to_string())
}

/// Approve a pending destructive action (user clicked "Confirm" in the modal).
#[tauri::command]
pub async fn approve_destructive_action(
    action_id: String,
    state: State<'_, AppState>,
) -> Result<(), String> {
    let id = Uuid::parse_str(&action_id).map_err(|e| e.to_string())?;
    let mut approvals = state.pending_approvals.write().await;
    let tx = approvals
        .remove(&id)
        .ok_or("No pending approval with that ID")?;
    tx.send(true)
        .map_err(|_| "Failed to send approval signal".into())
}

/// Reject a pending destructive action (user clicked "Cancel" in the modal).
#[tauri::command]
pub async fn reject_destructive_action(
    action_id: String,
    state: State<'_, AppState>,
) -> Result<(), String> {
    let id = Uuid::parse_str(&action_id).map_err(|e| e.to_string())?;
    let mut approvals = state.pending_approvals.write().await;
    let tx = approvals
        .remove(&id)
        .ok_or("No pending approval with that ID")?;
    tx.send(false)
        .map_err(|_| "Failed to send rejection signal".into())
}

/// Undo the most recent snapshot — restores files from the last captured manifest.
#[tauri::command]
pub async fn undo_last_snapshot(state: State<'_, AppState>) -> Result<String, String> {
    let config = state.config.read().await;
    let mgr = SnapshotManager::new(config.snapshot_dir.clone());

    let snapshots = mgr.list_snapshots().await.map_err(|e| e.to_string())?;

    let latest = snapshots
        .into_iter()
        .next()
        .ok_or("No snapshots available to undo")?;

    let restored = mgr.restore(latest.id).await.map_err(|e| e.to_string())?;

    Ok(format!(
        "Restored {} file(s) from snapshot '{}' ({})",
        restored, latest.label, latest.created_at
    ))
}

/// Get the list of recent task records for the dashboard.
#[tauri::command]
pub async fn get_task_history(state: State<'_, AppState>) -> Result<Value, String> {
    let history = state.task_history.read().await;
    serde_json::to_value(&*history).map_err(|e| e.to_string())
}

/// Toggle the dashboard drawer.
#[tauri::command]
pub async fn toggle_dashboard(expanded: bool, app: AppHandle) -> Result<(), String> {
    let window = app
        .get_webview_window("main")
        .ok_or("Main window not found")?;

    let (width, _) = window
        .outer_size()
        .map(|s| (s.width, s.height))
        .map_err(|e| e.to_string())?;

    let new_height = if expanded { 480u32 } else { 72u32 };

    window
        .set_size(tauri::Size::Physical(tauri::PhysicalSize {
            width,
            height: new_height,
        }))
        .map_err(|e| e.to_string())?;

    Ok(())
}

#[tauri::command]
pub async fn set_window_height(height: u32, app: AppHandle) -> Result<(), String> {
    let window = app
        .get_webview_window("main")
        .ok_or("Main window not found")?;

    let (width, _) = window
        .outer_size()
        .map(|s| (s.width, s.height))
        .map_err(|e| e.to_string())?;

    window
        .set_size(tauri::Size::Physical(tauri::PhysicalSize { width, height }))
        .map_err(|e| e.to_string())?;

    Ok(())
}

#[tauri::command]
pub async fn minimize_window(app: AppHandle) -> Result<(), String> {
    let window = app
        .get_webview_window("main")
        .ok_or("Main window not found")?;

    window.minimize().map_err(|e| e.to_string())?;
    Ok(())
}

/// Animated morph: resize + reposition the window in one OS call.
/// Used for orb↔bar↔app transitions. The frontend drives the easing
/// by calling this repeatedly with interpolated values.
#[tauri::command]
pub async fn morph_window(
    width: u32,
    height: u32,
    x: i32,
    y: i32,
    app: AppHandle,
) -> Result<(), String> {
    let window = app
        .get_webview_window("main")
        .ok_or("Main window not found")?;

    window
        .set_size(tauri::Size::Physical(tauri::PhysicalSize { width, height }))
        .map_err(|e| e.to_string())?;

    window
        .set_position(tauri::Position::Physical(tauri::PhysicalPosition { x, y }))
        .map_err(|e| e.to_string())?;

    Ok(())
}

/// Set window size only (used by transcript auto-resize while in app state).
#[tauri::command]
pub async fn set_window_size(width: u32, height: u32, app: AppHandle) -> Result<(), String> {
    let window = app
        .get_webview_window("main")
        .ok_or("Main window not found")?;

    window
        .set_size(tauri::Size::Physical(tauri::PhysicalSize { width, height }))
        .map_err(|e| e.to_string())?;

    Ok(())
}

/// Grab keyboard focus + raise the window on the desktop z-stack.
#[tauri::command]
pub async fn raise_window(app: AppHandle) -> Result<(), String> {
    let window = app
        .get_webview_window("main")
        .ok_or("Main window not found")?;

    window.show().map_err(|e| e.to_string())?;
    window.set_focus().map_err(|e| e.to_string())?;
    Ok(())
}

/// Toggle whether the window appears in the taskbar.
/// While in orb mode we hide from the taskbar so the orb feels like
/// a floating assistant rather than another app.
#[tauri::command]
pub async fn set_skip_taskbar(skip: bool, app: AppHandle) -> Result<(), String> {
    let window = app
        .get_webview_window("main")
        .ok_or("Main window not found")?;

    window.set_skip_taskbar(skip).map_err(|e| e.to_string())?;
    Ok(())
}

/// Read the current outer position + outer size of the window.
/// Used by the orb controller to know where to morph from.
#[tauri::command]
pub async fn get_window_geometry(app: AppHandle) -> Result<WindowGeometry, String> {
    let window = app
        .get_webview_window("main")
        .ok_or("Main window not found")?;

    let pos = window.outer_position().map_err(|e| e.to_string())?;
    let size = window.outer_size().map_err(|e| e.to_string())?;

    Ok(WindowGeometry {
        x: pos.x,
        y: pos.y,
        width: size.width,
        height: size.height,
    })
}

/// Move only — used while dragging the orb.
#[tauri::command]
pub async fn set_window_position(x: i32, y: i32, app: AppHandle) -> Result<(), String> {
    let window = app
        .get_webview_window("main")
        .ok_or("Main window not found")?;

    window
        .set_position(tauri::Position::Physical(tauri::PhysicalPosition { x, y }))
        .map_err(|e| e.to_string())?;

    Ok(())
}

#[derive(serde::Serialize)]
pub struct WindowGeometry {
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
}

#[tauri::command]
pub async fn close_window(app: AppHandle) -> Result<(), String> {
    let window = app
        .get_webview_window("main")
        .ok_or("Main window not found")?;

    window.hide().map_err(|e| e.to_string())?;
    Ok(())
}

/// Fully quit the application (including background agent tasks).
#[tauri::command]
pub async fn quit_app(app: AppHandle) -> Result<(), String> {
    app.exit(0);
    Ok(())
}

#[tauri::command]
pub async fn start_voice_input(state: State<'_, AppState>) -> Result<(), String> {
    let tx = state
        .voice_cmd_tx
        .read()
        .await
        .clone()
        .ok_or("Voice pipeline not initialised yet")?;

    let mut recording = state.recording.write().await;
    if *recording {
        return Err("Already recording".into());
    }
    *recording = true;
    drop(recording);

    tx.send(VoiceCommand::Start).map_err(|e| e.to_string())?;
    Ok(())
}

#[tauri::command]
pub async fn stop_voice_input(state: State<'_, AppState>) -> Result<(), String> {
    let tx = state
        .voice_cmd_tx
        .read()
        .await
        .clone()
        .ok_or("Voice pipeline not initialised yet")?;

    tx.send(VoiceCommand::Stop).map_err(|e| e.to_string())?;
    *state.recording.write().await = false;
    Ok(())
}

/// Voice control signal.
#[derive(Debug)]
pub enum VoiceCommand {
    Start,
    Stop,
}
