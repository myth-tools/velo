use chrono::{DateTime, Utc};
use serde_json::Value;
use sqlx::sqlite::{SqliteConnectOptions, SqlitePoolOptions};
use sqlx::{Executor, Sqlite, SqlitePool};
use uuid::Uuid;

use super::error::MemoryError;

const MIGRATIONS: &[&str] = &[
    "CREATE TABLE IF NOT EXISTS tasks (
        id          TEXT PRIMARY KEY NOT NULL,
        parent_id   TEXT,
        title       TEXT NOT NULL,
        status      TEXT NOT NULL DEFAULT 'Pending'
                    CHECK(status IN ('Pending','InProgress','Success','Failed')),
        created_at  TEXT NOT NULL,
        updated_at  TEXT NOT NULL,
        FOREIGN KEY (parent_id) REFERENCES tasks(id)
    )",
    "CREATE TABLE IF NOT EXISTS execution_logs (
        id             TEXT PRIMARY KEY NOT NULL,
        task_id        TEXT NOT NULL,
        tool_name      TEXT NOT NULL,
        input_payload  TEXT NOT NULL DEFAULT '{}',
        output_payload TEXT NOT NULL DEFAULT '{}',
        exit_code      INTEGER NOT NULL DEFAULT 0,
        timestamp      TEXT NOT NULL,
        FOREIGN KEY (task_id) REFERENCES tasks(id)
    )",
    "CREATE TABLE IF NOT EXISTS user_preferences (
        key   TEXT PRIMARY KEY NOT NULL,
        value TEXT NOT NULL DEFAULT '{}'
    )",
];

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum TaskStatus {
    Pending,
    InProgress,
    Success,
    Failed,
}

impl TaskStatus {
    pub fn as_str(&self) -> &'static str {
        match self {
            TaskStatus::Pending => "Pending",
            TaskStatus::InProgress => "InProgress",
            TaskStatus::Success => "Success",
            TaskStatus::Failed => "Failed",
        }
    }
}

pub struct StorageBackend {
    pool: SqlitePool,
}

impl StorageBackend {
    pub async fn connect(database_url: &str) -> Result<Self, MemoryError> {
        let connect_options = SqliteConnectOptions::new()
            .filename(database_url)
            .create_if_missing(true)
            .pragma("journal_mode", "WAL");

        let pool = SqlitePoolOptions::new()
            .max_connections(4)
            .connect_with(connect_options)
            .await?;

        let backend = Self { pool };
        backend.run_migrations().await?;
        Ok(backend)
    }

    async fn run_migrations(&self) -> Result<(), MemoryError> {
        let mut conn = self.pool.acquire().await?;
        for migration in MIGRATIONS {
            conn.execute(*migration).await?;
        }
        tracing::info!("SQLite migrations applied successfully");
        Ok(())
    }

    pub async fn insert_task(
        &self,
        id: Uuid,
        parent_id: Option<Uuid>,
        title: &str,
        status: &str,
    ) -> Result<(), MemoryError> {
        let now = Utc::now().to_rfc3339();
        sqlx::query(
            "INSERT INTO tasks (id, parent_id, title, status, created_at, updated_at)
             VALUES (?, ?, ?, ?, ?, ?)",
        )
        .bind(id.to_string())
        .bind(parent_id.map(|p| p.to_string()))
        .bind(title)
        .bind(status)
        .bind(&now)
        .bind(&now)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    pub async fn update_task_status(&self, id: Uuid, status: &str) -> Result<(), MemoryError> {
        let now = Utc::now().to_rfc3339();
        sqlx::query("UPDATE tasks SET status = ?, updated_at = ? WHERE id = ?")
            .bind(status)
            .bind(&now)
            .bind(id.to_string())
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    pub async fn get_task(&self, id: Uuid) -> Result<Option<TaskRecord>, MemoryError> {
        let row = sqlx::query_as::<Sqlite, TaskRow>(
            "SELECT id, parent_id, title, status, created_at, updated_at FROM tasks WHERE id = ?",
        )
        .bind(id.to_string())
        .fetch_optional(&self.pool)
        .await?;

        Ok(row.map(TaskRecord::from))
    }

    pub async fn list_tasks(
        &self,
        status: Option<&str>,
        limit: i64,
    ) -> Result<Vec<TaskRecord>, MemoryError> {
        let rows = if let Some(s) = status {
            sqlx::query_as::<Sqlite, TaskRow>(
                "SELECT id, parent_id, title, status, created_at, updated_at FROM tasks WHERE status = ? ORDER BY created_at DESC LIMIT ?",
            )
            .bind(s)
            .bind(limit)
            .fetch_all(&self.pool)
            .await?
        } else {
            sqlx::query_as::<Sqlite, TaskRow>(
                "SELECT id, parent_id, title, status, created_at, updated_at FROM tasks ORDER BY created_at DESC LIMIT ?",
            )
            .bind(limit)
            .fetch_all(&self.pool)
            .await?
        };

        Ok(rows.into_iter().map(TaskRecord::from).collect())
    }

    pub async fn insert_execution_log(
        &self,
        id: Uuid,
        task_id: Uuid,
        tool_name: &str,
        input_payload: &Value,
        output_payload: &Value,
        exit_code: i32,
    ) -> Result<(), MemoryError> {
        let timestamp = Utc::now().to_rfc3339();
        sqlx::query(
            "INSERT INTO execution_logs (id, task_id, tool_name, input_payload, output_payload, exit_code, timestamp)
             VALUES (?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(id.to_string())
        .bind(task_id.to_string())
        .bind(tool_name)
        .bind(input_payload.to_string())
        .bind(output_payload.to_string())
        .bind(exit_code)
        .bind(&timestamp)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    pub async fn get_execution_logs(
        &self,
        task_id: Uuid,
        limit: i64,
    ) -> Result<Vec<ExecutionLogRecord>, MemoryError> {
        let rows = sqlx::query_as::<Sqlite, ExecutionLogRow>(
            "SELECT id, task_id, tool_name, input_payload, output_payload, exit_code, timestamp
             FROM execution_logs WHERE task_id = ?
             ORDER BY timestamp DESC LIMIT ?",
        )
        .bind(task_id.to_string())
        .bind(limit)
        .fetch_all(&self.pool)
        .await?;

        Ok(rows.into_iter().map(ExecutionLogRecord::from).collect())
    }

    pub async fn set_preference(&self, key: &str, value: &Value) -> Result<(), MemoryError> {
        sqlx::query("INSERT OR REPLACE INTO user_preferences (key, value) VALUES (?, ?)")
            .bind(key)
            .bind(value.to_string())
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    pub async fn get_preference(&self, key: &str) -> Result<Option<Value>, MemoryError> {
        let row: Option<(String,)> =
            sqlx::query_as("SELECT value FROM user_preferences WHERE key = ?")
                .bind(key)
                .fetch_optional(&self.pool)
                .await?;

        match row {
            Some((val_str,)) => {
                let val: Value = serde_json::from_str(&val_str)?;
                Ok(Some(val))
            }
            None => Ok(None),
        }
    }

    pub async fn close(&self) -> Result<(), MemoryError> {
        self.pool.close().await;
        Ok(())
    }
}

#[derive(Debug, Clone, sqlx::FromRow)]
struct TaskRow {
    id: String,
    parent_id: Option<String>,
    title: String,
    status: String,
    created_at: String,
    updated_at: String,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct TaskRecord {
    pub id: Uuid,
    pub parent_id: Option<Uuid>,
    pub title: String,
    pub status: TaskStatus,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

impl From<TaskRow> for TaskRecord {
    fn from(row: TaskRow) -> Self {
        Self {
            id: Uuid::parse_str(&row.id).unwrap_or_default(),
            parent_id: row.parent_id.and_then(|p| Uuid::parse_str(&p).ok()),
            title: row.title,
            status: match row.status.as_str() {
                "Pending" => TaskStatus::Pending,
                "InProgress" => TaskStatus::InProgress,
                "Success" => TaskStatus::Success,
                "Failed" => TaskStatus::Failed,
                _ => {
                    tracing::warn!(
                        "Unknown task status '{}', defaulting to Pending",
                        row.status
                    );
                    TaskStatus::Pending
                }
            },
            created_at: DateTime::parse_from_rfc3339(&row.created_at)
                .map(|dt| dt.to_utc())
                .unwrap_or_else(|_| {
                    tracing::warn!("Failed to parse created_at '{}', using now", row.created_at);
                    Utc::now()
                }),
            updated_at: DateTime::parse_from_rfc3339(&row.updated_at)
                .map(|dt| dt.to_utc())
                .unwrap_or_else(|_| {
                    tracing::warn!("Failed to parse updated_at '{}', using now", row.updated_at);
                    Utc::now()
                }),
        }
    }
}

#[derive(Debug, Clone, sqlx::FromRow)]
struct ExecutionLogRow {
    id: String,
    task_id: String,
    tool_name: String,
    input_payload: String,
    output_payload: String,
    exit_code: i32,
    timestamp: String,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct ExecutionLogRecord {
    pub id: Uuid,
    pub task_id: Uuid,
    pub tool_name: String,
    pub input_payload: Value,
    pub output_payload: Value,
    pub exit_code: i32,
    pub timestamp: DateTime<Utc>,
}

impl From<ExecutionLogRow> for ExecutionLogRecord {
    fn from(row: ExecutionLogRow) -> Self {
        Self {
            id: Uuid::parse_str(&row.id).unwrap_or_default(),
            task_id: Uuid::parse_str(&row.task_id).unwrap_or_default(),
            tool_name: row.tool_name,
            input_payload: serde_json::from_str(&row.input_payload).unwrap_or_default(),
            output_payload: serde_json::from_str(&row.output_payload).unwrap_or_default(),
            exit_code: row.exit_code,
            timestamp: DateTime::parse_from_rfc3339(&row.timestamp)
                .map(|dt| dt.to_utc())
                .unwrap_or_else(|_| {
                    tracing::warn!("Failed to parse timestamp '{}', using now", row.timestamp);
                    Utc::now()
                }),
        }
    }
}
