use std::sync::Arc;

use autoagents::core::agent::memory::SlidingWindowMemory;
use autoagents::core::agent::prebuilt::executor::ReActAgent;
use autoagents::core::agent::{AgentBuilder, DirectAgent};
use autoagents::llm::backends::openai::OpenAI;
use autoagents::llm::builder::LLMBuilder;
use autoagents::llm::LLMProvider;
use autoagents::protocol::Event;
use futures_util::StreamExt;
use tokio::sync::{broadcast, mpsc, Mutex};
use uuid::Uuid;

use crate::agent::{VeloAgent, VeloAgentHooks};
use crate::config::VeloConfig;
use crate::error::VeloError;
use crate::handle::VeloAgentHandle;
use crate::memory::MemoryManagerHandle;
use crate::sub_agent::default_sub_agents;
use crate::tools::{init_global_config, init_sub_agent_registry, init_task_tool_description};

pub type ApprovalSender = mpsc::UnboundedSender<(Uuid, mpsc::UnboundedSender<bool>)>;

pub struct VeloAgentBuilder {
    config: VeloConfig,
    event_tx: Option<mpsc::UnboundedSender<(String, serde_json::Value)>>,
    approval_tx: Option<ApprovalSender>,
    memory_size: usize,
}

impl VeloAgentBuilder {
    pub fn new(config: VeloConfig) -> Self {
        Self {
            config,
            event_tx: None,
            approval_tx: None,
            memory_size: 20,
        }
    }

    pub fn with_event_channel(
        mut self,
        tx: mpsc::UnboundedSender<(String, serde_json::Value)>,
    ) -> Self {
        self.event_tx = Some(tx);
        self
    }

    pub fn with_approval_channel(mut self, tx: ApprovalSender) -> Self {
        self.approval_tx = Some(tx);
        self
    }

    pub fn memory_size(mut self, size: usize) -> Self {
        self.memory_size = size;
        self
    }

    pub async fn build(self) -> Result<VeloAgentHandle, VeloError> {
        init_global_config(self.config.clone());
        let sub_agents = default_sub_agents();
        init_sub_agent_registry(sub_agents.clone());
        init_task_tool_description(&sub_agents);

        let llm: Arc<dyn LLMProvider> = LLMBuilder::<OpenAI>::new()
            .api_key(&self.config.nvidia_api_key)
            .base_url(&self.config.nim_base_url)
            .model(&self.config.nim_model)
            .max_tokens(self.config.max_tokens)
            .temperature(self.config.temperature)
            .timeout_seconds(self.config.shell_timeout_secs * 2)
            .build()
            .map_err(|e| VeloError::Other(anyhow::anyhow!("LLM build: {e}")))?;

        let event_tx = self.event_tx.unwrap_or_else(|| {
            let (tx, _) = mpsc::unbounded_channel();
            tx
        });

        let approval_tx = self.approval_tx.unwrap_or_else(|| {
            let (tx, _) = mpsc::unbounded_channel();
            tx
        });

        let data_dir = format!(
            "{}/.velo/memory",
            std::env::var("HOME").unwrap_or_else(|_| "/tmp".into())
        );
        let _ = std::fs::create_dir_all(&data_dir);

        let memory_handle = match MemoryManagerHandle::new(&self.config, &data_dir).await {
            Ok(m) => {
                tracing::info!("Memory system initialized at {data_dir}");
                Some(m)
            }
            Err(e) => {
                tracing::warn!("Failed to initialize memory system, continuing without: {e}");
                None
            }
        };

        let hooks = Arc::new(VeloAgentHooks {
            event_tx,
            approval_tx,
            pending_task_id: Arc::new(Mutex::new(None)),
            memory: memory_handle.clone(),
        });

        let agent = VeloAgent { hooks: Some(hooks) };

        let react_agent = ReActAgent::new(agent);

        let mut handle = AgentBuilder::<_, DirectAgent>::new(react_agent)
            .llm(llm)
            .memory(Box::new(SlidingWindowMemory::new(self.memory_size)))
            .build()
            .await
            .map_err(|e| VeloError::Other(anyhow::anyhow!("Agent build: {e}")))?;

        let (event_broadcast, _) = broadcast::channel::<Event>(256);
        let raw_events = handle.subscribe_events();
        let bs_tx = event_broadcast.clone();
        tokio::spawn(async move {
            tokio::pin!(raw_events);
            while let Some(event) = raw_events.next().await {
                let _ = bs_tx.send(event);
            }
        });

        Ok(VeloAgentHandle {
            inner: Arc::new(handle),
            event_broadcast,
            memory: memory_handle,
        })
    }
}
