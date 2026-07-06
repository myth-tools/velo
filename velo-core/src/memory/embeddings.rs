use std::sync::Arc;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use tokio::sync::mpsc;
use tokio::sync::oneshot;
use uuid::Uuid;

use super::error::MemoryError;

#[derive(Debug, Clone, Serialize, Deserialize)]
struct EmbeddingRequest {
    model: String,
    input: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct EmbeddingResponse {
    data: Vec<EmbeddingData>,
    model: String,
    usage: EmbeddingUsage,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct EmbeddingData {
    embedding: Vec<f32>,
    index: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct EmbeddingUsage {
    prompt_tokens: usize,
    total_tokens: usize,
}

#[derive(Debug)]
pub struct EmbeddingJob {
    pub id: Uuid,
    pub text: String,
    pub reply_tx: Option<oneshot::Sender<Result<Vec<f32>, MemoryError>>>,
}

#[derive(Debug, Clone)]
pub struct EmbeddingWorker {
    job_tx: mpsc::Sender<EmbeddingJob>,
    shutdown_tx: Option<Arc<tokio::sync::Notify>>,
}

impl EmbeddingWorker {
    pub fn new(api_base_url: String, api_key: String, model: String, max_retries: u32) -> Self {
        let (job_tx, job_rx) = mpsc::channel::<EmbeddingJob>(128);
        let shutdown = Arc::new(tokio::sync::Notify::new());
        let shutdown_clone = shutdown.clone();

        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(30))
            .build()
            .unwrap_or_else(|e| {
                tracing::error!("Failed to build HTTP client with timeout: {e}, using default");
                reqwest::Client::new()
            });

        tokio::spawn(async move {
            Self::run_worker(
                client,
                api_base_url,
                api_key,
                model,
                job_rx,
                max_retries,
                shutdown_clone,
            )
            .await;
        });

        Self {
            job_tx,
            shutdown_tx: Some(shutdown),
        }
    }

    async fn run_worker(
        client: reqwest::Client,
        api_base_url: String,
        api_key: String,
        model: String,
        mut job_rx: mpsc::Receiver<EmbeddingJob>,
        max_retries: u32,
        shutdown: Arc<tokio::sync::Notify>,
    ) {
        let embed_url = format!("{}/v1/embeddings", api_base_url.trim_end_matches('/'));

        loop {
            tokio::select! {
                _ = shutdown.notified() => {
                    tracing::info!("Embedding worker shutting down");
                    break;
                }
                Some(job) = job_rx.recv() => {
                    let result = Self::embed_with_retry(
                        &client,
                        &embed_url,
                        &api_key,
                        &model,
                        &job.text,
                        max_retries,
                    )
                    .await;

                    if let Some(tx) = job.reply_tx {
                        let _ = tx.send(result);
                    }
                }
                else => {
                    break;
                }
            }
        }

        tracing::info!("Embedding worker stopped");
    }

    async fn embed_with_retry(
        client: &reqwest::Client,
        url: &str,
        api_key: &str,
        model: &str,
        text: &str,
        max_retries: u32,
    ) -> Result<Vec<f32>, MemoryError> {
        let mut last_error = None;

        for attempt in 0..=max_retries {
            if attempt > 0 {
                let delay = Duration::from_millis(100 * 2u64.pow(attempt.saturating_sub(1)));
                tokio::time::sleep(delay).await;
                tracing::warn!(
                    "Retrying embedding request (attempt {}/{})",
                    attempt,
                    max_retries
                );
            }

            match Self::embed_single(client, url, api_key, model, text).await {
                Ok(vector) => return Ok(vector),
                Err(e) => {
                    last_error = Some(e);
                }
            }
        }

        Err(last_error.unwrap_or(MemoryError::Embedding("Max retries exhausted".into())))
    }

    async fn embed_single(
        client: &reqwest::Client,
        url: &str,
        api_key: &str,
        model: &str,
        text: &str,
    ) -> Result<Vec<f32>, MemoryError> {
        let request = EmbeddingRequest {
            model: model.to_string(),
            input: vec![text.to_string()],
        };

        let response = client
            .post(url)
            .header("Authorization", format!("Bearer {}", api_key))
            .json(&request)
            .send()
            .await?;

        if !response.status().is_success() {
            let status = response.status();
            let body = match response.text().await {
                Ok(b) => b,
                Err(_) => "Failed to read response body".to_string(),
            };
            return Err(MemoryError::Embedding(format!(
                "NIM API returned {status}: {body}"
            )));
        }

        let embed_response: EmbeddingResponse = response.json().await?;

        embed_response
            .data
            .into_iter()
            .next()
            .map(|d| d.embedding)
            .ok_or_else(|| MemoryError::Embedding("Empty embedding response".into()))
    }

    pub fn submit(&self, text: String) -> Result<(), MemoryError> {
        let job = EmbeddingJob {
            id: Uuid::new_v4(),
            text,
            reply_tx: None,
        };
        self.job_tx
            .try_send(job)
            .map_err(|e| MemoryError::ChannelClosed(format!("Embedding job channel error: {e}")))
    }

    pub fn submit_with_reply(
        &self,
        text: String,
    ) -> Result<oneshot::Receiver<Result<Vec<f32>, MemoryError>>, MemoryError> {
        let (tx, rx) = oneshot::channel();
        let job = EmbeddingJob {
            id: Uuid::new_v4(),
            text,
            reply_tx: Some(tx),
        };
        self.job_tx
            .try_send(job)
            .map(|_| rx)
            .map_err(|e| MemoryError::ChannelClosed(format!("Embedding job channel error: {e}")))
    }

    pub fn shutdown(&self) -> Result<(), MemoryError> {
        if let Some(notify) = &self.shutdown_tx {
            notify.notify_one();
        }
        Ok(())
    }
}
