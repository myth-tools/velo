//! SnapshotManager — captures lightweight SHA-256 file manifests before
//! destructive operations, and provides a reverse-replay undo path.

use chrono::{DateTime, Utc};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};
use tokio::fs;
use tracing::{info, warn};
use uuid::Uuid;

use crate::error::VeloError;

// ── Manifest types ─────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileSnapshot {
    pub path: PathBuf,
    pub hash: String,
    /// Base64-encoded original content (only for files ≤ 512 KB).
    pub content_b64: Option<String>,
    pub size_bytes: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SnapshotManifest {
    pub id: Uuid,
    pub created_at: DateTime<Utc>,
    pub label: String,
    pub files: Vec<FileSnapshot>,
}

// ── Manager ────────────────────────────────────────────────────────────────────

pub struct SnapshotManager {
    snapshot_dir: PathBuf,
}

/// Maximum file size to embed content for instant undo (512 KB).
const EMBED_LIMIT: u64 = 512 * 1024;

impl SnapshotManager {
    pub fn new(snapshot_dir: PathBuf) -> Self {
        Self { snapshot_dir }
    }

    /// Ensure the snapshot directory exists.
    async fn ensure_dir(&self) -> Result<(), VeloError> {
        fs::create_dir_all(&self.snapshot_dir)
            .await
            .map_err(VeloError::FileOp)
    }

    /// Snapshot a single file.
    pub async fn snapshot_file(&self, path: impl AsRef<Path>) -> Result<Uuid, VeloError> {
        let path = path.as_ref();
        self.ensure_dir().await?;

        let snap = Self::build_file_snapshot(path).await?;
        let id = Uuid::new_v4();
        let manifest = SnapshotManifest {
            id,
            created_at: Utc::now(),
            label: format!("file:{}", path.display()),
            files: vec![snap],
        };

        self.save_manifest(&manifest).await?;
        info!(id = %id, path = %path.display(), "Snapshot created");
        Ok(id)
    }

    /// Snapshot an entire path (file or directory tree).
    pub async fn snapshot_path(&self, path: impl AsRef<Path>) -> Result<Uuid, VeloError> {
        let path = path.as_ref();
        self.ensure_dir().await?;

        let mut files = Vec::new();
        collect_files(path, &mut files).await?;

        let id = Uuid::new_v4();
        let manifest = SnapshotManifest {
            id,
            created_at: Utc::now(),
            label: format!("path:{}", path.display()),
            files,
        };

        self.save_manifest(&manifest).await?;
        info!(id = %id, path = %path.display(), files = manifest.files.len(), "Path snapshot created");
        Ok(id)
    }

    /// Restore all files from a snapshot manifest by ID.
    pub async fn restore(&self, snapshot_id: Uuid) -> Result<usize, VeloError> {
        let manifest_path = self.snapshot_dir.join(format!("{snapshot_id}.json"));
        let data = fs::read_to_string(&manifest_path)
            .await
            .map_err(|_| VeloError::Snapshot(format!("Snapshot {snapshot_id} not found")))?;

        let manifest: SnapshotManifest =
            serde_json::from_str(&data).map_err(|e| VeloError::Snapshot(e.to_string()))?;

        let mut restored = 0;
        for snap in &manifest.files {
            if let Some(ref b64) = snap.content_b64 {
                use base64::Engine;
                match base64::engine::general_purpose::STANDARD.decode(b64) {
                    Ok(bytes) => {
                        if let Some(parent) = snap.path.parent() {
                            let _ = fs::create_dir_all(parent).await;
                        }
                        fs::write(&snap.path, &bytes)
                            .await
                            .map_err(VeloError::FileOp)?;
                        restored += 1;
                    }
                    Err(e) => warn!(path = %snap.path.display(), "Could not decode snapshot: {e}"),
                }
            } else {
                warn!(
                    path = %snap.path.display(),
                    "File was too large to embed in snapshot; cannot restore automatically"
                );
            }
        }

        info!(snapshot_id = %snapshot_id, restored, "Snapshot restored");
        Ok(restored)
    }

    /// List all available snapshot manifests (newest first).
    pub async fn list_snapshots(&self) -> Result<Vec<SnapshotManifest>, VeloError> {
        self.ensure_dir().await?;
        let mut entries = fs::read_dir(&self.snapshot_dir)
            .await
            .map_err(VeloError::FileOp)?;

        let mut manifests = Vec::new();
        while let Some(entry) = entries.next_entry().await.map_err(VeloError::FileOp)? {
            let p = entry.path();
            if p.extension().and_then(|e| e.to_str()) == Some("json") {
                if let Ok(data) = fs::read_to_string(&p).await {
                    if let Ok(m) = serde_json::from_str::<SnapshotManifest>(&data) {
                        manifests.push(m);
                    }
                }
            }
        }

        manifests.sort_by_key(|b| std::cmp::Reverse(b.created_at));
        Ok(manifests)
    }

    // ── Private helpers ────────────────────────────────────────────────────────

    async fn save_manifest(&self, manifest: &SnapshotManifest) -> Result<(), VeloError> {
        let path = self.snapshot_dir.join(format!("{}.json", manifest.id));
        let json = serde_json::to_string_pretty(manifest)
            .map_err(|e| VeloError::Snapshot(e.to_string()))?;
        fs::write(&path, json).await.map_err(VeloError::FileOp)?;
        Ok(())
    }

    async fn build_file_snapshot(path: &Path) -> Result<FileSnapshot, VeloError> {
        let bytes = fs::read(path).await.map_err(VeloError::FileOp)?;
        let size_bytes = bytes.len() as u64;

        let mut hasher = Sha256::new();
        hasher.update(&bytes);
        let hash = format!("{:x}", hasher.finalize());

        let content_b64 = if size_bytes <= EMBED_LIMIT {
            use base64::Engine;
            Some(base64::engine::general_purpose::STANDARD.encode(&bytes))
        } else {
            None
        };

        Ok(FileSnapshot {
            path: path.to_path_buf(),
            hash,
            content_b64,
            size_bytes,
        })
    }
}

// ── Recursive file collector ───────────────────────────────────────────────────

async fn collect_files(path: &Path, out: &mut Vec<FileSnapshot>) -> Result<(), VeloError> {
    let meta = fs::metadata(path).await.map_err(VeloError::FileOp)?;

    if meta.is_file() {
        out.push(SnapshotManager::build_file_snapshot(path).await?);
    } else if meta.is_dir() {
        let mut dir = fs::read_dir(path).await.map_err(VeloError::FileOp)?;
        while let Some(entry) = dir.next_entry().await.map_err(VeloError::FileOp)? {
            // Use Box::pin to avoid infinite future size
            Box::pin(collect_files(&entry.path(), out)).await?;
        }
    }
    Ok(())
}
