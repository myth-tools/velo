use std::sync::Arc;

use autoagents::core::agent::prebuilt::executor::ReActAgent;
use autoagents::core::agent::task::Task;
use autoagents::core::agent::DirectAgentHandle;
use autoagents::protocol::Event;
use futures_util::{Stream, StreamExt};
use std::pin::Pin;
use tokio::sync::broadcast;
use tokio_stream::wrappers::BroadcastStream;

use crate::agent::{VeloAgent, SYSTEM_PROMPT};
use crate::error::VeloError;
use crate::memory::MemoryManagerHandle;

#[derive(Clone)]
pub struct VeloAgentHandle {
    pub(crate) inner: Arc<DirectAgentHandle<ReActAgent<VeloAgent>>>,
    pub(crate) event_broadcast: broadcast::Sender<Event>,
    pub memory: Option<MemoryManagerHandle>,
}

impl VeloAgentHandle {
    async fn build_system_prompt(&self, description: &str) -> String {
        let mut prompt = SYSTEM_PROMPT.to_string();

        if let Some(memory) = &self.memory {
            match memory
                .query_semantic_memory(description.to_string(), 3)
                .await
            {
                Ok(results) if !results.is_empty() => {
                    prompt.push_str("\n\n## Context from Past Sessions\n");
                    for (i, record) in results.iter().enumerate() {
                        prompt.push_str(&format!("{}. {}\n", i + 1, record.text_content));
                    }
                }
                Ok(_) => {}
                Err(e) => {
                    tracing::warn!("Semantic memory query failed: {e}");
                }
            }
        }

        prompt
    }

    pub async fn run(&self, description: &str) -> Result<String, VeloError> {
        let system_prompt = self.build_system_prompt(description).await;
        let task = Task::new(description).with_system_prompt(&system_prompt);
        self.inner
            .agent
            .run(task)
            .await
            .map(|out| out.response)
            .map_err(|e| VeloError::Other(anyhow::anyhow!("Agent run: {e}")))
    }

    pub async fn run_stream(
        &self,
        description: &str,
    ) -> Result<Pin<Box<dyn Stream<Item = Result<String, VeloError>> + Send>>, VeloError> {
        let system_prompt = self.build_system_prompt(description).await;
        let task = Task::new(description).with_system_prompt(&system_prompt);
        let stream = self
            .inner
            .agent
            .run_stream(task)
            .await
            .map_err(|e| VeloError::Other(anyhow::anyhow!("Agent stream: {e}")))?;
        let stream = stream.map(|result| {
            result
                .map(|out| out.response)
                .map_err(|e| VeloError::Other(anyhow::anyhow!("Agent stream: {e}")))
        });
        Ok(Box::pin(stream))
    }

    pub fn subscribe_events(&self) -> Pin<Box<dyn Stream<Item = Event> + Send>> {
        let rx = self.event_broadcast.subscribe();
        Box::pin(BroadcastStream::new(rx).filter_map(|r| futures_util::future::ready(r.ok())))
    }
}
