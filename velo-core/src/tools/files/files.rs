use async_trait::async_trait;
use autoagents::core::tool::{ToolCallError, ToolInputT, ToolRuntime, ToolT};
use autoagents_derive::tool;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::path::Path;

use crate::sandbox::fs::{check_read_access, check_write_access};
use crate::tools::{exec_err, snapshot_manager, ToolOutput};

const MAX_READ_SIZE: u64 = 100 * 1024 * 1024; // 100 MB
const READ_TRUNCATE_AT: usize = 32 * 1024;
const LIST_MAX: usize = 500;
const FIND_MAX: usize = 200;

// ── Args ──────────────────────────────────────────────────────────────────

#[derive(Serialize, Deserialize, Debug)]
pub struct ReadFileArgs {
    pub path: String,
    pub start_line: Option<usize>,
    pub line_count: Option<usize>,
}

impl ToolInputT for ReadFileArgs {
    fn io_schema() -> &'static str {
        r#"{"type":"object","properties":{"path":{"type":"string","description":"Absolute or relative path to the file to read."},"start_line":{"type":"integer","description":"Line number to start reading from (1-indexed). Useful for reading large files in chunks."},"line_count":{"type":"integer","description":"Number of lines to read from start_line. Omit to read the whole file (truncated at 32KB display)."}}}"#
    }
}

#[derive(Serialize, Deserialize, Debug)]
pub struct WriteFileArgs {
    pub path: String,
    pub content: String,
}

impl ToolInputT for WriteFileArgs {
    fn io_schema() -> &'static str {
        r#"{"type":"object","properties":{"path":{"type":"string","description":"Destination file path. Parent directories are created automatically. Refuses to overwrite symlinks (security)."},"content":{"type":"string","description":"Full text content to write (UTF-8). For large files, use shell with heredocs/redirects."}}}"#
    }
}

#[derive(Serialize, Deserialize, Debug)]
pub struct ListDirArgs {
    pub path: String,
    pub glob: Option<String>,
    pub recursive: Option<bool>,
}

impl ToolInputT for ListDirArgs {
    fn io_schema() -> &'static str {
        r#"{"type":"object","properties":{"path":{"type":"string","description":"Directory to list. Shows file sizes (human-readable), permissions, and type (file/dir/symlink)."},"glob":{"type":"string","description":"Optional glob filter, e.g. '**/*.rs' for all Rust files.\nExamples: '*.json', 'src/**/*.ts', '*.{rs,toml}'."},"recursive":{"type":"boolean","description":"If true, recurse into subdirectories showing the full tree. Default: false. Max 500 entries total."}}}"#
    }
}

#[derive(Serialize, Deserialize, Debug)]
pub struct DeletePathArgs {
    pub path: String,
}

impl ToolInputT for DeletePathArgs {
    fn io_schema() -> &'static str {
        r#"{"type":"object","properties":{"path":{"type":"string","description":"File or directory to permanently delete. Directories are deleted recursively. A snapshot is saved for undo recovery."}}}"#
    }
}

#[derive(Serialize, Deserialize, Debug)]
pub struct CopyMoveArgs {
    pub source: String,
    pub destination: String,
}

impl ToolInputT for CopyMoveArgs {
    fn io_schema() -> &'static str {
        r#"{"type":"object","properties":{"source":{"type":"string","description":"Source file or directory to copy. Must exist."},"destination":{"type":"string","description":"Destination path. If source is a directory, contents are copied recursively. If destination ends with /, the source is copied inside. Overwrites existing files."}}}"#
    }
}

#[derive(Serialize, Deserialize, Debug)]
pub struct FindFileArgs {
    pub pattern: String,
    pub path: Option<String>,
    pub max_depth: Option<usize>,
    pub file_type: Option<String>,
}

impl ToolInputT for FindFileArgs {
    fn io_schema() -> &'static str {
        r#"{"type":"object","properties":{"pattern":{"type":"string","description":"Filename pattern to search for. Supports glob wildcards (*, ?, **) and plain substring matching. Examples: 'main.rs', '*.py', 'test_*', 'src/**/*.ts'."},"path":{"type":"string","description":"Root directory to start the search. Defaults to the current working directory."},"max_depth":{"type":"integer","description":"Maximum directory depth. Default: unlimited (searches all subdirectories). Set to 1 for current dir only."},"file_type":{"type":"string","description":"Filter results by file type: 'file', 'dir', or 'symlink'. Omit to find all types."}}}"#
    }
}

// ── Tool structs ──────────────────────────────────────────────────────────

#[tool(name = "read_file", description = "Read a text file's contents as UTF-8. Supports partial reads via start_line/line_count for large files. Max read: 100MB; display capped at 32KB. BEST FOR: viewing source code, config files, logs. Use ripgrep to SEARCH for a pattern across files. Use shell for binary files (xxd/od).", input = ReadFileArgs)]
#[derive(Default, Clone)]
pub struct ReadFileTool;

#[tool(name = "write_file", description = "Write text content to a file (UTF-8). Creates parent dirs automatically. Takes a snapshot before overwriting for undo. Refuses to overwrite symlinks (prevents hijacking). BEST FOR: creating/editing source files, configs, scripts. Use shell with redirects for binary content or very large files.", input = WriteFileArgs)]
#[derive(Default, Clone)]
pub struct WriteFileTool;

#[tool(name = "list_dir", description = "List directory contents showing sizes, permissions, and types. Supports glob filter and recursive mode. Max 500 entries. BEST FOR: exploring project structure, checking file sizes. Use find_file to locate files by name. Use ripgrep to find files by content.", input = ListDirArgs)]
#[derive(Default, Clone)]
pub struct ListDirTool;

#[tool(name = "delete_path", description = "PERMANENTLY delete a file or directory (recursive). A snapshot is saved before deletion for undo recovery. TRIGGERS confirmation prompt (High risk). BEST FOR: cleaning up temporary files, removing unwanted directories. Use shell (rm) for advanced deletion patterns requiring globbing or special flags.", input = DeletePathArgs)]
#[derive(Default, Clone)]
pub struct DeletePathTool;

#[tool(name = "copy_file", description = "Copy a file or directory (recursive) from source to destination. Preserves file metadata. Overwrites existing files silently. BEST FOR: backing up files, duplicating directories. Use shell (cp) if you need advanced options like symlink handling or --preserve=all.", input = CopyMoveArgs)]
#[derive(Default, Clone)]
pub struct CopyFileTool;

#[tool(name = "move_file", description = "Move or rename a file or directory. Works across filesystem boundaries (copy + delete if needed). Preserves metadata. BEST FOR: renaming files, moving data between directories. Use shell (mv) if you need atomic moves or advanced flags.", input = CopyMoveArgs)]
#[derive(Default, Clone)]
pub struct MoveFileTool;

#[tool(name = "find_file", description = "Find files and directories by name using glob or substring matching. Supports depth limits and type filtering (file/dir/symlink). Max 200 results. BEST FOR: locating files when you know the name but not the path. Use list_dir to browse directory structure. Use ripgrep to search file CONTENTS.", input = FindFileArgs)]
#[derive(Default, Clone)]
pub struct FindFileTool;

// ── Traits ────────────────────────────────────────────────────────────────

fn mime_type(path: &Path) -> &'static str {
    match path.extension().and_then(|e| e.to_str()).unwrap_or("") {
        "rs" => "text/rust",
        "py" => "text/python",
        "js" | "mjs" => "text/javascript",
        "ts" | "tsx" => "text/typescript",
        "json" => "application/json",
        "yaml" | "yml" => "application/yaml",
        "toml" => "application/toml",
        "md" => "text/markdown",
        "html" | "htm" => "text/html",
        "css" => "text/css",
        "xml" => "application/xml",
        "csv" => "text/csv",
        "sh" => "text/x-shellscript",
        "txt" | "" => "text/plain",
        "jpg" | "jpeg" => "image/jpeg",
        "png" => "image/png",
        "gif" => "image/gif",
        "svg" => "image/svg+xml",
        "pdf" => "application/pdf",
        _ => "application/octet-stream",
    }
}

#[async_trait]
impl ToolRuntime for ReadFileTool {
    async fn execute(&self, args: Value) -> Result<Value, ToolCallError> {
        let a: ReadFileArgs = serde_json::from_value(args)?;
        let path = Path::new(&a.path);
        check_read_access(path).map_err(exec_err)?;

        let meta = tokio::fs::metadata(path)
            .await
            .map_err(|e| exec_err(format!("Cannot stat: {e}")))?;

        if meta.len() > MAX_READ_SIZE {
            return Err(exec_err(format!(
                "File too large: {} ({} bytes, max {})",
                a.path,
                meta.len(),
                MAX_READ_SIZE
            )));
        }

        let mt = mime_type(path);
        let is_text = mt.starts_with("text/")
            || mt.contains("json")
            || mt.contains("yaml")
            || mt.contains("xml")
            || mt.contains("javascript")
            || mt.contains("typescript")
            || mt.contains("shell");

        // If line range requested, stream read lines instead of loading whole file
        if a.start_line.is_some() || a.line_count.is_some() {
            return read_file_lines(
                path,
                a.start_line.unwrap_or(1),
                a.line_count.unwrap_or(usize::MAX),
            )
            .await;
        }

        let bytes = tokio::fs::read(path)
            .await
            .map_err(|e| exec_err(format!("Read failed: {e}")))?;

        let content = if is_text {
            let s = String::from_utf8_lossy(&bytes);
            if s.len() > READ_TRUNCATE_AT {
                format!("{}\n\n[...truncated at 32KB]", &s[..READ_TRUNCATE_AT])
            } else {
                s.into_owned()
            }
        } else {
            format!(
                "[Binary file: {} ({} bytes, {})]",
                mt,
                meta.len(),
                sha256_preview(&bytes)
            )
        };

        Ok(ToolOutput::ok(format!("Type: {mt}\nSize: {} B\n\n{content}", meta.len())).into())
    }
}

async fn read_file_lines(path: &Path, start: usize, count: usize) -> Result<Value, ToolCallError> {
    use tokio::io::AsyncBufReadExt;
    let file = tokio::fs::File::open(path)
        .await
        .map_err(|e| exec_err(format!("Open failed: {e}")))?;
    let reader = tokio::io::BufReader::new(file);
    let mut lines = reader.lines();
    let mut out = String::new();
    let mut line_num = 0usize;
    let mut emitted = 0usize;
    let max_out = 32_768;

    while let Some(line) = lines
        .next_line()
        .await
        .map_err(|e| exec_err(e.to_string()))?
    {
        line_num += 1;
        if line_num < start {
            continue;
        }
        if emitted >= count || out.len() > max_out {
            break;
        }
        let l = format!("{:>6}: {}\n", line_num, line);
        if out.len() + l.len() > max_out {
            out.push_str("      ... [truncated]\n");
            break;
        }
        out.push_str(&l);
        emitted += 1;
    }

    if out.is_empty() {
        out = "(empty file or no lines in range)".into();
    }
    Ok(ToolOutput::ok(out).into())
}

fn sha256_preview(bytes: &[u8]) -> String {
    use sha2::Digest;
    let mut hasher = sha2::Sha256::new();
    hasher.update(bytes);
    let hash = format!("{:x}", hasher.finalize());
    format!("SHA256:{}", &hash[..16])
}

#[async_trait]
impl ToolRuntime for WriteFileTool {
    async fn execute(&self, args: Value) -> Result<Value, ToolCallError> {
        let a: WriteFileArgs = serde_json::from_value(args)?;
        let path = Path::new(&a.path);
        check_write_access(path).map_err(exec_err)?;

        if path.is_symlink() {
            return Err(exec_err(format!(
                "Refusing to overwrite symlink: {}",
                a.path
            )));
        }

        // Snapshot existing file
        if path.exists() {
            let mgr = snapshot_manager().await;
            let mut guard = mgr.lock().await;
            if let Some(ref mut mgr) = *guard {
                let _ = mgr.snapshot_file(&a.path).await;
            }
        }

        if let Some(parent) = path.parent() {
            tokio::fs::create_dir_all(parent)
                .await
                .map_err(|e| exec_err(format!("mkdir failed: {e}")))?;
        }

        tokio::fs::write(path, &a.content)
            .await
            .map_err(|e| exec_err(format!("Write failed: {e}")))?;

        Ok(ToolOutput::ok(format!("Written {} bytes to {}", a.content.len(), a.path)).into())
    }
}

#[async_trait]
impl ToolRuntime for ListDirTool {
    async fn execute(&self, args: Value) -> Result<Value, ToolCallError> {
        let a: ListDirArgs = serde_json::from_value(args)?;
        let path = Path::new(&a.path);
        check_read_access(path).map_err(exec_err)?;

        // If glob is set, use globwalk
        if let Some(glob) = &a.glob {
            return list_with_glob(path, glob, a.recursive.unwrap_or(false)).await;
        }

        let mut entries = tokio::fs::read_dir(path)
            .await
            .map_err(|e| exec_err(format!("Failed to list dir: {e}")))?;

        let mut items = Vec::new();
        while let Some(entry) = entries
            .next_entry()
            .await
            .map_err(|e| exec_err(e.to_string()))?
        {
            items.push(entry);
        }
        items.sort_by_key(|e| e.file_name());

        let mut out = format!("Contents of {} ({} items):\n", a.path, items.len());
        for (n, entry) in items.iter().enumerate() {
            if n >= LIST_MAX {
                out.push_str(&format!("  ... [truncated after {LIST_MAX} entries]\n"));
                break;
            }
            let name = entry.file_name().to_string_lossy().to_string();

            // Glob filter on name
            if let Some(ref g) = a.glob {
                if !glob_match(g, &name) {
                    continue;
                }
            }

            let meta = match entry.metadata().await {
                Ok(m) => m,
                Err(_) => {
                    out.push_str(&format!("  [ERR]   {name}\n"));
                    continue;
                }
            };

            let kind = if meta.is_dir() {
                "DIR"
            } else if meta.is_symlink() {
                "SYM"
            } else {
                "FILE"
            };
            let perms = unix_perms(&meta);
            let size = if meta.is_file() {
                human_size(meta.len())
            } else {
                "         -".into()
            };

            out.push_str(&format!("  [{kind}] {perms} {size}  {name}\n"));

            if a.recursive.unwrap_or(false) && meta.is_dir() {
                // Recurse using the same tool
                let sub = ListDirArgs {
                    path: format!("{}/{}", a.path.trim_end_matches('/'), name),
                    glob: a.glob.clone(),
                    recursive: Some(true),
                };
                let sub_value = serde_json::to_value(&sub).unwrap();
                if let Ok(v) = ListDirTool.execute(sub_value).await {
                    let display = v.get("display").and_then(|d| d.as_str()).unwrap_or("");
                    for line in display.lines().skip(1) {
                        out.push_str(&format!("  {line}\n"));
                    }
                }
            }
        }

        Ok(ToolOutput::ok(out.trim_end().to_string()).into())
    }
}

async fn list_with_glob(base: &Path, glob: &str, recursive: bool) -> Result<Value, ToolCallError> {
    let mut out = String::new();
    let max_depth = if recursive { usize::MAX } else { 1 };

    let walk = walkdir::WalkDir::new(base)
        .max_depth(max_depth)
        .follow_links(false);

    let mut count = 0usize;
    for entry in walk {
        if count >= LIST_MAX {
            out.push_str(&format!("  ... [truncated after {LIST_MAX} matches]\n"));
            break;
        }
        let entry = match entry {
            Ok(e) => e,
            Err(_) => continue,
        };

        let name = entry.file_name().to_string_lossy();
        if !glob_match(glob, &name) {
            continue;
        }

        let meta = match std::fs::metadata(entry.path()) {
            Ok(m) => m,
            Err(_) => continue,
        };
        let kind = if meta.is_dir() {
            "DIR"
        } else if meta.is_symlink() {
            "SYM"
        } else {
            "FILE"
        };
        out.push_str(&format!("  [{kind}] {}\n", entry.path().display()));
        count += 1;
    }

    if out.is_empty() {
        out = "No matches found.".into();
    }
    Ok(ToolOutput::ok(out).into())
}

fn glob_match(pattern: &str, name: &str) -> bool {
    // Simple glob matching using ? and *
    let re_pattern = format!(
        "^{}$",
        regex::escape(pattern)
            .replace(r"\?", ".") // ? -> any single char
            .replace(r"\*", ".*") // * -> any sequence
    );
    regex::Regex::new(&re_pattern).map_or(true, |re| re.is_match(name))
}

fn unix_perms(meta: &std::fs::Metadata) -> String {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = meta.permissions().mode();
        let mut s = String::with_capacity(9);
        s.push(if mode & 0o400 != 0 { 'r' } else { '-' });
        s.push(if mode & 0o200 != 0 { 'w' } else { '-' });
        s.push(if mode & 0o100 != 0 { 'x' } else { '-' });
        s.push(if mode & 0o040 != 0 { 'r' } else { '-' });
        s.push(if mode & 0o020 != 0 { 'w' } else { '-' });
        s.push(if mode & 0o010 != 0 { 'x' } else { '-' });
        s.push(if mode & 0o004 != 0 { 'r' } else { '-' });
        s.push(if mode & 0o002 != 0 { 'w' } else { '-' });
        s.push(if mode & 0o001 != 0 { 'x' } else { '-' });
        s
    }
    #[cfg(not(unix))]
    {
        let _ = meta;
        String::new()
    }
}

fn human_size(bytes: u64) -> String {
    const UNITS: &[&str] = &["B", "KB", "MB", "GB", "TB"];
    let mut size = bytes as f64;
    let mut unit = 0;
    while size >= 1024.0 && unit < UNITS.len() - 1 {
        size /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{:>7} {}", bytes, UNITS[unit])
    } else {
        format!("{:>6.1} {}", size, UNITS[unit])
    }
}

#[async_trait]
impl ToolRuntime for DeletePathTool {
    async fn execute(&self, args: Value) -> Result<Value, ToolCallError> {
        let a: DeletePathArgs = serde_json::from_value(args)?;
        let path = Path::new(&a.path);
        check_write_access(path).map_err(exec_err)?;

        if path.is_symlink() {
            return Err(exec_err(format!("Refusing to delete symlink: {}", a.path)));
        }

        {
            let mgr = snapshot_manager().await;
            let mut guard = mgr.lock().await;
            if let Some(ref mut mgr) = *guard {
                let _ = mgr.snapshot_path(&a.path).await;
            }
        }

        let meta = tokio::fs::metadata(path)
            .await
            .map_err(|e| exec_err(format!("Not found: {e}")))?;

        if meta.is_dir() {
            tokio::fs::remove_dir_all(path)
                .await
                .map_err(|e| exec_err(format!("rmdir failed: {e}")))?;
        } else {
            tokio::fs::remove_file(path)
                .await
                .map_err(|e| exec_err(format!("rm failed: {e}")))?;
        }

        Ok(ToolOutput::ok(format!("Deleted: {}", a.path)).into())
    }
}

#[async_trait]
impl ToolRuntime for CopyFileTool {
    async fn execute(&self, args: Value) -> Result<Value, ToolCallError> {
        let a: CopyMoveArgs = serde_json::from_value(args)?;
        let src = Path::new(&a.source);
        let dst = Path::new(&a.destination);
        check_read_access(src).map_err(exec_err)?;
        check_write_access(dst).map_err(exec_err)?;

        // Snapshot destination before overwriting
        if dst.exists() {
            let mgr = snapshot_manager().await;
            let mut guard = mgr.lock().await;
            if let Some(ref mut mgr) = *guard {
                let _ = mgr.snapshot_path(&a.destination).await;
            }
        }

        if let Some(parent) = dst.parent() {
            tokio::fs::create_dir_all(parent)
                .await
                .map_err(|e| exec_err(format!("mkdir failed: {e}")))?;
        }

        let meta = tokio::fs::metadata(src)
            .await
            .map_err(|e| exec_err(format!("Source not found: {e}")))?;

        if meta.is_dir() {
            copy_dir_recursive(src, dst).await?;
        } else {
            tokio::fs::copy(src, dst)
                .await
                .map_err(|e| exec_err(format!("Copy failed: {e}")))?;
        }

        Ok(ToolOutput::ok(format!("Copied {} → {}", a.source, a.destination)).into())
    }
}

async fn copy_dir_recursive(src: &Path, dst: &Path) -> Result<(), ToolCallError> {
    tokio::fs::create_dir_all(dst)
        .await
        .map_err(|e| exec_err(format!("mkdir failed: {e}")))?;

    let mut entries = tokio::fs::read_dir(src)
        .await
        .map_err(|e| exec_err(format!("readdir failed: {e}")))?;

    while let Some(entry) = entries
        .next_entry()
        .await
        .map_err(|e| exec_err(e.to_string()))?
    {
        let file_type = entry
            .file_type()
            .await
            .map_err(|e| exec_err(e.to_string()))?;
        let src_path = entry.path();
        let dst_path = dst.join(entry.file_name());

        if file_type.is_dir() {
            Box::pin(copy_dir_recursive(&src_path, &dst_path)).await?;
        } else {
            tokio::fs::copy(&src_path, &dst_path)
                .await
                .map_err(|e| exec_err(format!("Copy failed: {e}")))?;
        }
    }
    Ok(())
}

#[async_trait]
impl ToolRuntime for MoveFileTool {
    async fn execute(&self, args: Value) -> Result<Value, ToolCallError> {
        let a: CopyMoveArgs = serde_json::from_value(args)?;
        let src = Path::new(&a.source);
        let dst = Path::new(&a.destination);
        check_write_access(src).map_err(exec_err)?;
        check_write_access(dst).map_err(exec_err)?;

        // Snapshot destination before overwriting
        if dst.exists() {
            let mgr = snapshot_manager().await;
            let mut guard = mgr.lock().await;
            if let Some(ref mut mgr) = *guard {
                let _ = mgr.snapshot_path(&a.destination).await;
            }
        }

        if let Some(parent) = dst.parent() {
            tokio::fs::create_dir_all(parent)
                .await
                .map_err(|e| exec_err(format!("mkdir failed: {e}")))?;
        }

        tokio::fs::rename(src, dst)
            .await
            .map_err(|e| exec_err(format!("Move failed: {e}")))?;

        Ok(ToolOutput::ok(format!("Moved {} → {}", a.source, a.destination)).into())
    }
}

#[async_trait]
impl ToolRuntime for FindFileTool {
    async fn execute(&self, args: Value) -> Result<Value, ToolCallError> {
        let a: FindFileArgs = serde_json::from_value(args)?;
        let root_str = a.path.unwrap_or_else(|| ".".into());
        let root = Path::new(&root_str);
        check_read_access(root).map_err(exec_err)?;

        let mut out = String::new();
        let mut count = 0usize;

        let walk = walkdir::WalkDir::new(root)
            .max_depth(a.max_depth.unwrap_or(usize::MAX))
            .follow_links(false);

        for entry in walk {
            if count >= FIND_MAX {
                out.push_str(&format!("... [truncated after {FIND_MAX} matches]\n"));
                break;
            }
            let entry = match entry {
                Ok(e) => e,
                Err(_) => continue,
            };

            // Filter by file type
            if let Some(ref ft) = a.file_type {
                let is_match = match ft.as_str() {
                    "file" => entry.file_type().is_file(),
                    "dir" => entry.file_type().is_dir(),
                    "symlink" => entry.file_type().is_symlink(),
                    _ => true,
                };
                if !is_match {
                    continue;
                }
            }

            let name = entry.file_name().to_string_lossy();
            // Match pattern against filename
            if !name.contains(&a.pattern) && !glob_match(&a.pattern, &name) {
                continue;
            }

            out.push_str(&format!("  {}\n", entry.path().display()));
            count += 1;
        }

        if out.is_empty() {
            out = format!(
                "No files matching '{}' found in {}",
                a.pattern,
                root.display()
            );
        } else {
            out = format!(
                "Found {} result(s) for '{}' in {}:\n{}",
                count,
                a.pattern,
                root.display(),
                out
            );
        }

        Ok(ToolOutput::ok(out.trim().to_string()).into())
    }
}
