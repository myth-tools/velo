use std::collections::HashMap;
use std::time::Duration;

use tauri::{AppHandle, Emitter, Manager};
use tokio::sync::{mpsc, RwLock};
use tracing::{error, info, warn};
use uuid::Uuid;

use velo_core::audio::{self, resample::PcmResampler, AudioCapture};
use velo_core::config::VeloConfig;
use velo_core::events::{self, TaskRecord};
use velo_core::{builder::VeloAgentBuilder, handle::VeloAgentHandle};

use crate::commands::VoiceCommand;

/// Shared application state stored in Tauri's managed state container.
pub struct AppState {
    pub handle: RwLock<Option<VeloAgentHandle>>,
    pub config: RwLock<VeloConfig>,
    pub task_history: RwLock<Vec<TaskRecord>>,
    pub recording: RwLock<bool>,
    pub voice_cmd_tx: RwLock<Option<mpsc::UnboundedSender<VoiceCommand>>>,
    /// Pending destructive action approval channels, keyed by action_id.
    pub pending_approvals: RwLock<HashMap<Uuid, mpsc::UnboundedSender<bool>>>,
}

impl AppState {
    pub fn new(config: VeloConfig) -> Self {
        Self {
            handle: RwLock::new(None),
            config: RwLock::new(config),
            task_history: RwLock::new(Vec::new()),
            recording: RwLock::new(false),
            voice_cmd_tx: RwLock::new(None),
            pending_approvals: RwLock::new(HashMap::new()),
        }
    }
}

/// Spawn the event-forwarding task that reads from the agent event channel
/// and emits each event to the Tauri window.
pub fn spawn_event_bridge(
    app: AppHandle,
    mut event_rx: mpsc::UnboundedReceiver<(String, serde_json::Value)>,
) {
    tokio::spawn(async move {
        info!("Event bridge started");

        while let Some((event_name, payload)) = event_rx.recv().await {
            if event_name == events::event_names::TASK_STATUS_UPDATE {
                if let Ok(record) = serde_json::from_value::<TaskRecord>(payload.clone()) {
                    if let Some(state) = app.try_state::<AppState>() {
                        let mut history = state.task_history.write().await;
                        if let Some(pos) = history.iter().position(|r| r.id == record.id) {
                            history[pos] = record;
                        } else {
                            history.push(record);
                            if history.len() > 100 {
                                history.remove(0);
                            }
                        }
                    }
                }
            }

            if let Err(e) = app.emit(&event_name, &payload) {
                error!("Failed to emit event '{event_name}': {e}");
            }
        }

        info!("Event bridge channel closed — shutting down");
    });
}

/// Listen for destructive action approval requests from agent hooks and
/// store the response channel in AppState so approve/reject commands can
/// resolve them.
pub fn spawn_approval_relay(
    app: AppHandle,
    mut approval_rx: mpsc::UnboundedReceiver<(Uuid, mpsc::UnboundedSender<bool>)>,
) {
    tokio::spawn(async move {
        info!("Approval relay started");
        while let Some((action_id, response_tx)) = approval_rx.recv().await {
            if let Some(state) = app.try_state::<AppState>() {
                state
                    .pending_approvals
                    .write()
                    .await
                    .insert(action_id, response_tx);
            }
        }
        info!("Approval relay channel closed");
    });
}

/// Spawn the push-to-talk voice pipeline on a dedicated OS thread.
pub fn spawn_voice_pipeline(
    config: VeloConfig,
    event_tx: mpsc::UnboundedSender<(String, serde_json::Value)>,
    agent_handle: VeloAgentHandle,
) -> mpsc::UnboundedSender<VoiceCommand> {
    let (tx, mut rx) = mpsc::unbounded_channel::<VoiceCommand>();

    std::thread::spawn(move || {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("voice pipeline tokio runtime");

        let mut capture: Option<AudioCapture> = None;
        let mut pcm_rx: Option<crossbeam_channel::Receiver<Vec<f32>>> = None;
        let mut sample_rate: u32 = 0;
        let mut channels: u16 = 0;

        while let Some(cmd) = rt.block_on(rx.recv()) {
            match cmd {
                VoiceCommand::Start => {
                    if capture.is_some() {
                        warn!("Voice pipeline: Start received but already capturing");
                        continue;
                    }
                    match AudioCapture::start() {
                        Ok((cap, handle)) => {
                            capture = Some(cap);
                            pcm_rx = Some(handle.rx);
                            sample_rate = handle.sample_rate;
                            channels = handle.channels;
                            info!(
                                "Voice capture started ({} Hz, {} ch)",
                                sample_rate, channels
                            );
                        }
                        Err(e) => {
                            error!("Failed to start audio capture: {e}");
                        }
                    }
                }

                VoiceCommand::Stop => {
                    let Some(rx) = pcm_rx.take() else {
                        warn!("Voice pipeline: Stop received but no active capture");
                        continue;
                    };
                    drop(capture.take());

                    let mut raw_pcm: Vec<f32> = Vec::new();
                    loop {
                        match rx.recv_timeout(Duration::from_millis(80)) {
                            Ok(chunk) => raw_pcm.extend(chunk),
                            Err(crossbeam_channel::RecvTimeoutError::Timeout) => break,
                            Err(crossbeam_channel::RecvTimeoutError::Disconnected) => break,
                        }
                    }

                    if raw_pcm.is_empty() {
                        info!("Voice pipeline: no audio captured");
                        continue;
                    }

                    let resampled = match PcmResampler::new(sample_rate, channels as usize) {
                        Ok(mut r) => match r.process_interleaved(&raw_pcm) {
                            Ok(v) => v,
                            Err(e) => {
                                error!("Resample failed: {e}");
                                continue;
                            }
                        },
                        Err(e) => {
                            error!("Resampler init failed: {e}");
                            continue;
                        }
                    };

                    let transcript = match rt.block_on(audio::stt::transcribe(&resampled, &config))
                    {
                        Ok(t) => t,
                        Err(e) => {
                            error!("Cloud STT failed: {e}");
                            continue;
                        }
                    };

                    if let Ok(payload) = serde_json::to_value(&transcript) {
                        let _ =
                            event_tx.send((events::event_names::STT_TRANSCRIPT.into(), payload));
                    }

                    let handle = agent_handle.clone();
                    rt.spawn(async move {
                        if let Err(e) = handle.run(&transcript.text).await {
                            error!("Voice agent run failed: {e}");
                        }
                    });
                }
            }
        }

        info!("Voice pipeline thread exiting");
    });

    tx
}

/// Build the agent and return the event channel.
pub async fn bootstrap_agent(
    config: VeloConfig,
) -> Result<
    (
        VeloAgentHandle,
        mpsc::UnboundedSender<(String, serde_json::Value)>,
        mpsc::UnboundedReceiver<(String, serde_json::Value)>,
        mpsc::UnboundedReceiver<(Uuid, mpsc::UnboundedSender<bool>)>,
    ),
    velo_core::error::VeloError,
> {
    let (event_tx, event_rx) = mpsc::unbounded_channel::<(String, serde_json::Value)>();
    let (approval_tx, approval_rx) = mpsc::unbounded_channel();

    let handle = VeloAgentBuilder::new(config)
        .with_event_channel(event_tx.clone())
        .with_approval_channel(approval_tx)
        .build()
        .await?;

    Ok((handle, event_tx, event_rx, approval_rx))
}
