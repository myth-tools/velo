use thiserror::Error;

#[derive(Debug, Error)]
pub enum MemoryError {
    #[error("Database error: {0}")]
    Database(String),

    #[error("SQLite error: {0}")]
    Sqlite(#[from] sqlx::Error),

    #[error("Vector store error: {0}")]
    VectorStore(String),

    #[error("LanceDB error: {0}")]
    LanceDb(#[from] lancedb::error::Error),

    #[error("Arrow error: {0}")]
    Arrow(#[from] arrow::error::ArrowError),

    #[error("Embedding API error: {0}")]
    Embedding(String),

    #[error("HTTP request error: {0}")]
    Http(#[from] reqwest::Error),

    #[error("Serialization error: {0}")]
    Serialization(#[from] serde_json::Error),

    #[error("Actor communication error: {0}")]
    Actor(String),

    #[error("Channel closed: {0}")]
    ChannelClosed(String),

    #[error("Operation timed out: {0}")]
    Timeout(String),

    #[error("Not found: {0}")]
    NotFound(String),

    #[error("Configuration error: {0}")]
    Config(String),

    #[error("Storage backend not initialized: {0}")]
    NotInitialized(String),

    #[error("Session buffer full: {0}")]
    SessionBufferFull(String),
}
