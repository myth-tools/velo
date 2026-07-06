use std::sync::Arc;

use chrono::{DateTime, Utc};
use ractor::{Actor, ActorProcessingErr, ActorRef};
use serde::{Deserialize, Serialize};
use tokio::sync::watch;
use uuid::Uuid;

use super::embeddings::EmbeddingWorker;
use super::relational::StorageBackend;
use super::session::SessionBuffer;
use super::vector::{SemanticMemoryRecord, VectorStore};
use super::MemoryError;

const SESSION_TOKEN_LIMIT: usize = 8192;
const SESSION_TRIM_THRESHOLD: f64 = 0.8;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkingState {
    pub active_task_id: Option<Uuid>,
    pub active_task_description: String,
    pub last_tool_call: Option<String>,
    pub last_tool_result: Option<String>,
    pub terminal_log_tail: Vec<String>,
    pub session_summary: Option<String>,
    pub uptime: DateTime<Utc>,
}

impl Default for WorkingState {
    fn default() -> Self {
        Self {
            active_task_id: None,
            active_task_description: String::new(),
            last_tool_call: None,
            last_tool_result: None,
            terminal_log_tail: Vec::with_capacity(64),
            session_summary: None,
            uptime: Utc::now(),
        }
    }
}

pub struct MemoryActorState {
    pub working_state: WorkingState,
    pub session_buffer: SessionBuffer,
    pub watch_tx: watch::Sender<WorkingState>,
    pub storage: Option<Arc<StorageBackend>>,
    pub embedding_worker: Option<EmbeddingWorker>,
    pub vector_store: Option<VectorStore>,
}

pub struct MemoryManagerActor {
    pub storage: Option<Arc<StorageBackend>>,
    pub embedding_worker: Option<EmbeddingWorker>,
    pub vector_store: Option<VectorStore>,
    pub session_token_limit: usize,
    pub data_dir: String,
}

impl MemoryManagerActor {
    pub fn new(data_dir: String) -> Self {
        Self {
            storage: None,
            embedding_worker: None,
            vector_store: None,
            session_token_limit: SESSION_TOKEN_LIMIT,
            data_dir,
        }
    }

    pub fn with_storage(mut self, storage: Arc<StorageBackend>) -> Self {
        self.storage = Some(storage);
        self
    }

    pub fn with_embedding_worker(mut self, worker: EmbeddingWorker) -> Self {
        self.embedding_worker = Some(worker);
        self
    }

    pub fn with_vector_store(mut self, store: VectorStore) -> Self {
        self.vector_store = Some(store);
        self
    }
}

#[async_trait::async_trait]
impl Actor for MemoryManagerActor {
    type Msg = super::MemoryMessage;
    type State = MemoryActorState;
    type Arguments = ();

    async fn pre_start(
        &self,
        _myself: ActorRef<Self::Msg>,
        _args: Self::Arguments,
    ) -> Result<Self::State, ActorProcessingErr> {
        let (watch_tx, _) = watch::channel(WorkingState::default());

        let state = MemoryActorState {
            working_state: WorkingState::default(),
            session_buffer: SessionBuffer::new(self.session_token_limit),
            watch_tx,
            storage: self.storage.clone(),
            embedding_worker: self.embedding_worker.clone(),
            vector_store: self.vector_store.clone(),
        };

        tracing::info!("MemoryManagerActor started");
        Ok(state)
    }

    async fn handle(
        &self,
        _myself: ActorRef<Self::Msg>,
        message: Self::Msg,
        state: &mut Self::State,
    ) -> Result<(), ActorProcessingErr> {
        match message {
            super::MemoryMessage::UpdateWorkingState {
                task_id,
                description,
                tool_call,
                tool_result,
            } => {
                if let Some(id) = task_id {
                    state.working_state.active_task_id = Some(id);
                }
                if let Some(desc) = description {
                    state.working_state.active_task_description = desc;
                }
                state.working_state.last_tool_call = tool_call;
                state.working_state.last_tool_result = tool_result;
                let _ = state.watch_tx.send(state.working_state.clone());
            }

            super::MemoryMessage::AppendTerminalLog { line } => {
                let tail = &mut state.working_state.terminal_log_tail;
                tail.push(line);
                if tail.len() > 64 {
                    let excess = tail.len() - 64;
                    tail.drain(0..excess);
                }
                let _ = state.watch_tx.send(state.working_state.clone());
            }

            super::MemoryMessage::AppendSessionMessage { role, content } => {
                let approx_tokens = content.len() / 4;
                state.session_buffer.push(role, content, approx_tokens);

                let usage =
                    state.session_buffer.token_usage() as f64 / self.session_token_limit as f64;
                if usage >= SESSION_TRIM_THRESHOLD {
                    let summary_target = state.session_buffer.compress_oldest_ratio(0.3);
                    if let Some((summary, _removed)) = summary_target {
                        state.working_state.session_summary = Some(summary);
                        let _ = state.watch_tx.send(state.working_state.clone());
                    }
                }
            }

            super::MemoryMessage::FlushSession => {
                state.session_buffer.clear();
                state.working_state.session_summary = None;
                let _ = state.watch_tx.send(state.working_state.clone());
            }

            super::MemoryMessage::GetWorkingState { reply } => {
                let result = Ok(state.working_state.clone());
                let _ = reply.send(result);
            }

            super::MemoryMessage::SubscribeWorkingState { reply } => {
                let rx = state.watch_tx.subscribe();
                let _ = reply.send(rx);
            }

            super::MemoryMessage::GetSessionContext { reply } => {
                let ctx = state.session_buffer.context_summary();
                let _ = reply.send(Ok(ctx));
            }

            super::MemoryMessage::StoreTask {
                id,
                parent_id,
                title,
                status,
                reply,
            } => {
                let result = match &state.storage {
                    Some(storage) => storage.insert_task(id, parent_id, &title, &status).await,
                    None => Err(MemoryError::NotInitialized("Storage not configured".into())),
                };
                let _ = reply.send(result);
            }

            super::MemoryMessage::UpdateTaskStatus { id, status, reply } => {
                let result = match &state.storage {
                    Some(storage) => storage.update_task_status(id, &status).await,
                    None => Err(MemoryError::NotInitialized("Storage not configured".into())),
                };
                let _ = reply.send(result);
            }

            super::MemoryMessage::StoreExecutionLog {
                id,
                task_id,
                tool_name,
                input_payload,
                output_payload,
                exit_code,
                reply,
            } => {
                let input_val: serde_json::Value =
                    serde_json::from_str(&input_payload).unwrap_or_default();
                let output_val: serde_json::Value =
                    serde_json::from_str(&output_payload).unwrap_or_default();
                let result = match &state.storage {
                    Some(storage) => {
                        storage
                            .insert_execution_log(
                                id,
                                task_id,
                                &tool_name,
                                &input_val,
                                &output_val,
                                exit_code,
                            )
                            .await
                    }
                    None => Err(MemoryError::NotInitialized("Storage not configured".into())),
                };
                let _ = reply.send(result);
            }

            super::MemoryMessage::QuerySemanticMemory {
                query_text,
                limit,
                reply,
            } => {
                let result = query_semantic_memory_impl(state, &query_text, limit).await;
                let _ = reply.send(result);
            }

            super::MemoryMessage::StoreEmbedding {
                text,
                metadata,
                reply,
            } => {
                let result = store_embedding_impl(state, &text, &metadata).await;
                let _ = reply.send(result);
            }

            super::MemoryMessage::SetPreference { key, value, reply } => {
                let val: serde_json::Value = serde_json::from_str(&value).unwrap_or_default();
                let result = match &state.storage {
                    Some(storage) => storage.set_preference(&key, &val).await,
                    None => Err(MemoryError::NotInitialized("Storage not configured".into())),
                };
                let _ = reply.send(result);
            }

            super::MemoryMessage::GetPreference { key, reply } => {
                let result = match &state.storage {
                    Some(storage) => storage
                        .get_preference(&key)
                        .await
                        .map(|opt| opt.map(|v| v.to_string())),
                    None => Err(MemoryError::NotInitialized("Storage not configured".into())),
                };
                let _ = reply.send(result);
            }
        }

        Ok(())
    }

    async fn post_stop(
        &self,
        _myself: ActorRef<Self::Msg>,
        state: &mut Self::State,
    ) -> Result<(), ActorProcessingErr> {
        tracing::info!("MemoryManagerActor shutting down");

        if let Some(worker) = &state.embedding_worker {
            let _ = worker.shutdown();
        }

        if let Some(vector_store) = &state.vector_store {
            if let Err(e) = vector_store.close().await {
                tracing::error!("Error closing vector store: {e}");
            }
        }

        if let Some(storage) = &state.storage {
            if let Err(e) = storage.close().await {
                tracing::error!("Error closing storage backend: {e}");
            }
        }

        tracing::info!("MemoryManagerActor shutdown complete");
        Ok(())
    }
}

async fn get_embedding(state: &MemoryActorState, text: &str) -> Result<Vec<f32>, MemoryError> {
    let worker = state
        .embedding_worker
        .as_ref()
        .ok_or_else(|| MemoryError::NotInitialized("Embedding worker not configured".into()))?;
    let rx = worker.submit_with_reply(text.to_string())?;
    rx.await
        .map_err(|_| MemoryError::ChannelClosed("Embedding reply channel closed".into()))?
}

async fn query_semantic_memory_impl(
    state: &mut MemoryActorState,
    query_text: &str,
    limit: usize,
) -> Result<Vec<SemanticMemoryRecord>, MemoryError> {
    let query_vector = get_embedding(state, query_text).await?;

    let vector_store = state
        .vector_store
        .as_ref()
        .ok_or_else(|| MemoryError::NotInitialized("Vector store not configured".into()))?;

    vector_store.search(&query_vector, limit).await
}

async fn store_embedding_impl(
    state: &mut MemoryActorState,
    text: &str,
    metadata: &str,
) -> Result<(), MemoryError> {
    let vector = get_embedding(state, text).await?;

    let vector_store = state
        .vector_store
        .as_ref()
        .ok_or_else(|| MemoryError::NotInitialized("Vector store not configured".into()))?;

    let id = Uuid::new_v4();
    vector_store.insert(id, vector, text, metadata).await?;
    Ok(())
}
