//! Tauri application library root.
//!
//! Called by the generated `main.rs` (Tauri v2 convention).

mod bridge;
mod commands;

use tauri::Manager;
use tracing::info;

use bridge::{
    bootstrap_agent, spawn_approval_relay, spawn_event_bridge, spawn_voice_pipeline, AppState,
};
use velo_core::{config::VeloConfig, init_tracing};

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    init_tracing();

    let config = VeloConfig::load().expect("Failed to load config — check ~/.velo/config.yaml");

    let app_state = AppState::new(config.clone());

    tauri::Builder::default()
        .on_window_event(|window, event| {
            match event {
                tauri::WindowEvent::CloseRequested { api, .. } => {
                    tracing::info!("Close requested — hiding window instead of closing");
                    let _ = window.hide();
                    api.prevent_close();
                }
                tauri::WindowEvent::Focused(true) => {
                    if !window.is_visible().unwrap_or(true) {
                        let _ = window.show();
                    }
                    let _ = window.set_focus();
                }
                _ => {}
            }
        })
        .manage(app_state)
        .setup(move |app| {
            let app_handle = app.handle().clone();
            let config_clone = config.clone();

            // Bootstrap agent + event bridge in background
            tauri::async_runtime::spawn(async move {
                match bootstrap_agent(config_clone.clone()).await {
                    Ok((handle, event_tx, event_rx, approval_rx)) => {
                        info!("Agent built successfully");

                        // Spawn voice pipeline
                        let voice_cmd_tx = spawn_voice_pipeline(
                            config_clone.clone(),
                            event_tx.clone(),
                            handle.clone(),
                        );

                        // Spawn approval relay so approve/reject commands can
                        // send decisions back to the blocking agent hook.
                        spawn_approval_relay(app_handle.clone(), approval_rx);

                        // Store handle + voice cmd channel in managed state
                        if let Some(state) = app_handle.try_state::<AppState>() {
                            *state.handle.write().await = Some(handle);
                            *state.voice_cmd_tx.write().await = Some(voice_cmd_tx);
                        }

                        spawn_event_bridge(app_handle, event_rx);
                    }
                    Err(e) => {
                        tracing::error!("Failed to bootstrap agent: {e}");
                    }
                }
            });

            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            commands::submit_text_command,
            commands::approve_destructive_action,
            commands::reject_destructive_action,
            commands::undo_last_snapshot,
            commands::get_task_history,
            commands::toggle_dashboard,
            commands::set_window_height,
            commands::minimize_window,
            commands::close_window,
            commands::quit_app,
            commands::start_voice_input,
            commands::stop_voice_input,
            commands::morph_window,
            commands::set_window_size,
            commands::raise_window,
            commands::set_skip_taskbar,
            commands::get_window_geometry,
            commands::set_window_position,
        ])
        .run(tauri::generate_context!())
        .expect("Error while running Velo Tauri application");
}
