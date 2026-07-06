pub mod actor;
pub mod embeddings;
pub mod error;
pub mod relational;
pub mod session;
pub mod vector;

use std::sync::Arc;
use std::time::Duration;

use ractor::{rpc::CallResult, Actor, ActorRef, RpcReplyPort};
use tokio::sync::watch;
use uuid::Uuid;

pub use actor::{MemoryManagerActor, WorkingState};
pub use embeddings::EmbeddingWorker;
pub use error::MemoryError;
pub use relational::{ExecutionLogRecord, StorageBackend, TaskRecord, TaskStatus};
pub use vector::{SemanticMemoryRecord, VectorStore};

use crate::config::VeloConfig;

#[derive(Debug)]
pub enum MemoryMessage {
    UpdateWorkingState {
        task_id: Option<Uuid>,
        description: Option<String>,
        tool_call: Option<String>,
        tool_result: Option<String>,
    },
    AppendTerminalLog {
        line: String,
    },
    AppendSessionMessage {
        role: String,
        content: String,
    },
    FlushSession,
    GetWorkingState {
        reply: RpcReplyPort<Result<WorkingState, MemoryError>>,
    },
    SubscribeWorkingState {
        reply: RpcReplyPort<watch::Receiver<WorkingState>>,
    },
    GetSessionContext {
        reply: RpcReplyPort<Result<session::SessionContext, MemoryError>>,
    },
    StoreTask {
        id: Uuid,
        parent_id: Option<Uuid>,
        title: String,
        status: String,
        reply: RpcReplyPort<Result<(), MemoryError>>,
    },
    UpdateTaskStatus {
        id: Uuid,
        status: String,
        reply: RpcReplyPort<Result<(), MemoryError>>,
    },
    StoreExecutionLog {
        id: Uuid,
        task_id: Uuid,
        tool_name: String,
        input_payload: String,
        output_payload: String,
        exit_code: i32,
        reply: RpcReplyPort<Result<(), MemoryError>>,
    },
    QuerySemanticMemory {
        query_text: String,
        limit: usize,
        reply: RpcReplyPort<Result<Vec<SemanticMemoryRecord>, MemoryError>>,
    },
    StoreEmbedding {
        text: String,
        metadata: String,
        reply: RpcReplyPort<Result<(), MemoryError>>,
    },
    SetPreference {
        key: String,
        value: String,
        reply: RpcReplyPort<Result<(), MemoryError>>,
    },
    GetPreference {
        key: String,
        reply: RpcReplyPort<Result<Option<String>, MemoryError>>,
    },
}

#[derive(Clone)]
pub struct MemoryManagerHandle {
    pub actor_ref: ActorRef<MemoryMessage>,
}

impl MemoryManagerHandle {
    pub async fn new(config: &VeloConfig, data_dir: &str) -> Result<Self, MemoryError> {
        let sqlite_path = format!("{data_dir}/memory.db");
        let lancedb_uri = format!("{data_dir}/lancedb");

        let storage = Arc::new(StorageBackend::connect(&sqlite_path).await?);

        let embedding_worker = EmbeddingWorker::new(
            config.nim_base_url.clone(),
            config.nvidia_api_key.clone(),
            config.nim_embedding_model.clone(),
            3,
        );

        let mut vector_store =
            VectorStore::new(lancedb_uri, config.nim_embedding_dimension as usize);
        vector_store.open_or_create().await?;

        let actor = MemoryManagerActor::new(data_dir.to_string())
            .with_storage(storage)
            .with_embedding_worker(embedding_worker)
            .with_vector_store(vector_store);

        let (actor_ref, _handle) = Actor::spawn(Some("memory-manager".into()), actor, ())
            .await
            .map_err(|e| MemoryError::Actor(format!("Failed to spawn MemoryManagerActor: {e}")))?;

        Ok(Self { actor_ref })
    }

    pub async fn new_with_backend(
        data_dir: &str,
        storage: Arc<StorageBackend>,
        embedding_worker: EmbeddingWorker,
        vector_store: VectorStore,
    ) -> Result<Self, MemoryError> {
        let actor = MemoryManagerActor::new(data_dir.to_string())
            .with_storage(storage)
            .with_embedding_worker(embedding_worker)
            .with_vector_store(vector_store);

        let (actor_ref, _handle) = Actor::spawn(Some("memory-manager".into()), actor, ())
            .await
            .map_err(|e| MemoryError::Actor(format!("Failed to spawn MemoryManagerActor: {e}")))?;

        Ok(Self { actor_ref })
    }

    pub fn update_working_state(
        &self,
        task_id: Option<Uuid>,
        description: Option<String>,
        tool_call: Option<String>,
        tool_result: Option<String>,
    ) -> Result<(), MemoryError> {
        self.actor_ref
            .cast(MemoryMessage::UpdateWorkingState {
                task_id,
                description,
                tool_call,
                tool_result,
            })
            .map_err(|e| MemoryError::Actor(format!("Failed to send UpdateWorkingState: {e}")))
    }

    pub fn append_terminal_log(&self, line: String) -> Result<(), MemoryError> {
        self.actor_ref
            .cast(MemoryMessage::AppendTerminalLog { line })
            .map_err(|e| MemoryError::Actor(format!("Failed to send AppendTerminalLog: {e}")))
    }

    pub fn append_session_message(&self, role: String, content: String) -> Result<(), MemoryError> {
        self.actor_ref
            .cast(MemoryMessage::AppendSessionMessage { role, content })
            .map_err(|e| MemoryError::Actor(format!("Failed to send AppendSessionMessage: {e}")))
    }

    pub fn flush_session(&self) -> Result<(), MemoryError> {
        self.actor_ref
            .cast(MemoryMessage::FlushSession)
            .map_err(|e| MemoryError::Actor(format!("Failed to send FlushSession: {e}")))
    }

    pub async fn get_working_state(&self) -> Result<WorkingState, MemoryError> {
        let result = self
            .actor_ref
            .call(
                |reply| MemoryMessage::GetWorkingState { reply },
                Some(Duration::from_secs(5)),
            )
            .await
            .map_err(|e| MemoryError::Actor(format!("Actor call failed: {e}")))?;

        match result {
            CallResult::Success(Ok(state)) => Ok(state),
            CallResult::Success(Err(e)) => Err(e),
            CallResult::Timeout => Err(MemoryError::Timeout("GetWorkingState".into())),
            CallResult::SenderError => {
                Err(MemoryError::Actor("GetWorkingState sender error".into()))
            }
        }
    }

    pub async fn subscribe_working_state(
        &self,
    ) -> Result<watch::Receiver<WorkingState>, MemoryError> {
        let result = self
            .actor_ref
            .call(
                |reply| MemoryMessage::SubscribeWorkingState { reply },
                Some(Duration::from_secs(5)),
            )
            .await
            .map_err(|e| MemoryError::Actor(format!("Actor call failed: {e}")))?;

        match result {
            CallResult::Success(rx) => Ok(rx),
            CallResult::Timeout => Err(MemoryError::Timeout("SubscribeWorkingState".into())),
            CallResult::SenderError => Err(MemoryError::Actor(
                "SubscribeWorkingState sender error".into(),
            )),
        }
    }

    pub async fn get_session_context(&self) -> Result<session::SessionContext, MemoryError> {
        let result = self
            .actor_ref
            .call(
                |reply| MemoryMessage::GetSessionContext { reply },
                Some(Duration::from_secs(5)),
            )
            .await
            .map_err(|e| MemoryError::Actor(format!("Actor call failed: {e}")))?;

        match result {
            CallResult::Success(Ok(ctx)) => Ok(ctx),
            CallResult::Success(Err(e)) => Err(e),
            CallResult::Timeout => Err(MemoryError::Timeout("GetSessionContext".into())),
            CallResult::SenderError => {
                Err(MemoryError::Actor("GetSessionContext sender error".into()))
            }
        }
    }

    pub async fn store_task(
        &self,
        id: Uuid,
        parent_id: Option<Uuid>,
        title: String,
        status: String,
    ) -> Result<(), MemoryError> {
        let result = self
            .actor_ref
            .call(
                |reply| MemoryMessage::StoreTask {
                    id,
                    parent_id,
                    title,
                    status,
                    reply,
                },
                Some(Duration::from_secs(10)),
            )
            .await
            .map_err(|e| MemoryError::Actor(format!("Actor call failed: {e}")))?;

        match result {
            CallResult::Success(r) => r,
            CallResult::Timeout => Err(MemoryError::Timeout("StoreTask".into())),
            CallResult::SenderError => Err(MemoryError::Actor("StoreTask sender error".into())),
        }
    }

    pub async fn update_task_status(&self, id: Uuid, status: String) -> Result<(), MemoryError> {
        let result = self
            .actor_ref
            .call(
                |reply| MemoryMessage::UpdateTaskStatus { id, status, reply },
                Some(Duration::from_secs(10)),
            )
            .await
            .map_err(|e| MemoryError::Actor(format!("Actor call failed: {e}")))?;

        match result {
            CallResult::Success(r) => r,
            CallResult::Timeout => Err(MemoryError::Timeout("UpdateTaskStatus".into())),
            CallResult::SenderError => {
                Err(MemoryError::Actor("UpdateTaskStatus sender error".into()))
            }
        }
    }

    pub async fn store_execution_log(
        &self,
        id: Uuid,
        task_id: Uuid,
        tool_name: String,
        input_payload: String,
        output_payload: String,
        exit_code: i32,
    ) -> Result<(), MemoryError> {
        let result = self
            .actor_ref
            .call(
                |reply| MemoryMessage::StoreExecutionLog {
                    id,
                    task_id,
                    tool_name,
                    input_payload,
                    output_payload,
                    exit_code,
                    reply,
                },
                Some(Duration::from_secs(10)),
            )
            .await
            .map_err(|e| MemoryError::Actor(format!("Actor call failed: {e}")))?;

        match result {
            CallResult::Success(r) => r,
            CallResult::Timeout => Err(MemoryError::Timeout("StoreExecutionLog".into())),
            CallResult::SenderError => {
                Err(MemoryError::Actor("StoreExecutionLog sender error".into()))
            }
        }
    }

    pub async fn query_semantic_memory(
        &self,
        query_text: String,
        limit: usize,
    ) -> Result<Vec<SemanticMemoryRecord>, MemoryError> {
        let result = self
            .actor_ref
            .call(
                |reply| MemoryMessage::QuerySemanticMemory {
                    query_text,
                    limit,
                    reply,
                },
                Some(Duration::from_secs(60)),
            )
            .await
            .map_err(|e| MemoryError::Actor(format!("Actor call failed: {e}")))?;

        match result {
            CallResult::Success(r) => r,
            CallResult::Timeout => Err(MemoryError::Timeout("QuerySemanticMemory".into())),
            CallResult::SenderError => Err(MemoryError::Actor(
                "QuerySemanticMemory sender error".into(),
            )),
        }
    }

    pub async fn store_embedding(&self, text: String, metadata: String) -> Result<(), MemoryError> {
        let result = self
            .actor_ref
            .call(
                |reply| MemoryMessage::StoreEmbedding {
                    text,
                    metadata,
                    reply,
                },
                Some(Duration::from_secs(60)),
            )
            .await
            .map_err(|e| MemoryError::Actor(format!("Actor call failed: {e}")))?;

        match result {
            CallResult::Success(r) => r,
            CallResult::Timeout => Err(MemoryError::Timeout("StoreEmbedding".into())),
            CallResult::SenderError => {
                Err(MemoryError::Actor("StoreEmbedding sender error".into()))
            }
        }
    }

    pub async fn set_preference(&self, key: String, value: String) -> Result<(), MemoryError> {
        let result = self
            .actor_ref
            .call(
                |reply| MemoryMessage::SetPreference { key, value, reply },
                Some(Duration::from_secs(10)),
            )
            .await
            .map_err(|e| MemoryError::Actor(format!("Actor call failed: {e}")))?;

        match result {
            CallResult::Success(r) => r,
            CallResult::Timeout => Err(MemoryError::Timeout("SetPreference".into())),
            CallResult::SenderError => Err(MemoryError::Actor("SetPreference sender error".into())),
        }
    }

    pub async fn get_preference(&self, key: String) -> Result<Option<String>, MemoryError> {
        let result = self
            .actor_ref
            .call(
                |reply| MemoryMessage::GetPreference { key, reply },
                Some(Duration::from_secs(10)),
            )
            .await
            .map_err(|e| MemoryError::Actor(format!("Actor call failed: {e}")))?;

        match result {
            CallResult::Success(r) => r,
            CallResult::Timeout => Err(MemoryError::Timeout("GetPreference".into())),
            CallResult::SenderError => Err(MemoryError::Actor("GetPreference sender error".into())),
        }
    }

    pub fn actor_ref(&self) -> ActorRef<MemoryMessage> {
        self.actor_ref.clone()
    }
}
