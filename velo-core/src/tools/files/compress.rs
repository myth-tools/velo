use std::fs::File;
use std::io::{BufReader, Write};
use std::path::{Path, PathBuf};

use async_trait::async_trait;
use autoagents::core::tool::{ToolCallError, ToolInputT, ToolRuntime, ToolT};
use autoagents_derive::tool;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use zip::write::SimpleFileOptions;
use zip::ZipWriter;

use crate::sandbox::fs::{check_read_access, check_write_access};
use crate::tools::{exec_err, ToolOutput};

#[derive(Serialize, Deserialize, Debug)]
pub struct CompressArgs {
    pub action: String,
    pub archive: String,
    pub files: Option<Vec<String>>,
    pub output_dir: Option<String>,
}

impl ToolInputT for CompressArgs {
    fn io_schema() -> &'static str {
        r#"{"type":"object","properties":{"action":{"type":"string","description":"Action: 'create' — pack files into a zip; 'extract' — unpack a zip; 'list' — show archive contents without extracting."},"archive":{"type":"string","description":"Path to the zip archive file (e.g. '/home/user/project.zip')."},"files":{"type":"array","items":{"type":"string"},"description":"List of files/directories to add. Required for 'create' action. Directories are added recursively preserving structure."},"output_dir":{"type":"string","description":"Directory to extract files into. Only for 'extract' action. Default: directory named after the archive (without .zip extension)."}}}"#
    }
}

#[tool(name = "compress", description = "Create, extract, or list contents of zip archives. Supports directories and nested paths. Built-in protection against path traversal attacks (ZIP Slip). BEST FOR: packaging files for transfer, extracting downloaded archives, inspecting zip contents. Use the shell tool with tar/gzip for non-zip formats (.tar.gz, .7z, .rar).", input = CompressArgs)]
#[derive(Default, Clone)]
pub struct CompressTool;

#[async_trait]
impl ToolRuntime for CompressTool {
    async fn execute(&self, args: Value) -> Result<Value, ToolCallError> {
        let a: CompressArgs = serde_json::from_value(args)?;
        let action = a.action.to_lowercase();
        let archive_path = a.archive;

        match action.as_str() {
            "create" => {
                check_write_access(Path::new(&archive_path)).map_err(exec_err)?;
                let files = a
                    .files
                    .ok_or_else(|| exec_err("files required for create"))?;
                if files.is_empty() {
                    return Err(exec_err("At least one file required"));
                }
                let result = tokio::task::spawn_blocking(move || -> Result<String, String> {
                    let path = Path::new(&archive_path);
                    if let Some(parent) = path.parent() {
                        std::fs::create_dir_all(parent)
                            .map_err(|e| format!("Cannot create dir: {e}"))?;
                    }

                    let file =
                        File::create(path).map_err(|e| format!("Cannot create archive: {e}"))?;
                    let mut zip = ZipWriter::new(file);
                    let options = SimpleFileOptions::default()
                        .compression_method(zip::CompressionMethod::Deflated);

                    for file_path in &files {
                        let src = Path::new(file_path);
                        if !src.exists() {
                            return Err(format!("File not found: {file_path}"));
                        }
                        if src.is_dir() {
                            add_dir_to_zip(&mut zip, src, src, &options)?;
                        } else {
                            let mut f = BufReader::new(
                                File::open(src)
                                    .map_err(|e| format!("Cannot open {file_path}: {e}"))?,
                            );
                            let name = src
                                .file_name()
                                .unwrap_or_default()
                                .to_string_lossy()
                                .to_string();
                            zip.start_file(&name, options)
                                .map_err(|e| format!("Zip error: {e}"))?;
                            std::io::copy(&mut f, &mut zip)
                                .map_err(|e| format!("Write error {file_path}: {e}"))?;
                        }
                    }

                    zip.finish().map_err(|e| format!("Zip finish: {e}"))?;
                    Ok(format!("Created archive: {archive_path}"))
                })
                .await
                .map_err(|e| exec_err(format!("Spawn: {e}")))?
                .map_err(exec_err)?;

                Ok(ToolOutput::ok(result).into())
            }
            "extract" => {
                check_read_access(Path::new(&archive_path)).map_err(exec_err)?;
                let out_dir = a.output_dir.unwrap_or_else(|| {
                    let p = Path::new(&archive_path);
                    p.file_stem()
                        .map(|s| s.to_string_lossy().to_string())
                        .unwrap_or_else(|| "extracted".into())
                });
                let result = tokio::task::spawn_blocking(move || -> Result<String, String> {
                    let file = File::open(&archive_path)
                        .map_err(|e| format!("Cannot open archive: {e}"))?;
                    let mut archive_reader =
                        zip::ZipArchive::new(file).map_err(|e| format!("Invalid zip: {e}"))?;

                    let out_dir = PathBuf::from(&out_dir);
                    let canonical_out = out_dir.canonicalize().unwrap_or_else(|_| out_dir.clone());

                    std::fs::create_dir_all(&out_dir)
                        .map_err(|e| format!("Cannot create dir: {e}"))?;

                    let mut extracted = 0usize;
                    for i in 0..archive_reader.len() {
                        let mut entry = archive_reader
                            .by_index(i)
                            .map_err(|e| format!("Entry error: {e}"))?;
                        let name = entry.name().to_string();
                        let entry_path = out_dir.join(&name);
                        let canonical_entry = entry_path
                            .canonicalize()
                            .unwrap_or_else(|_| entry_path.clone());

                        // ZIP Slip protection: reject entries that escape the target directory
                        if !canonical_entry.starts_with(&canonical_out) {
                            return Err(format!(
                                "Blocked path traversal: '{name}' would escape '{}'",
                                out_dir.display()
                            ));
                        }

                        if let Some(parent) = entry_path.parent() {
                            std::fs::create_dir_all(parent)
                                .map_err(|e| format!("Cannot create dir {parent:?}: {e}"))?;
                        }

                        if !entry.is_dir() {
                            let mut outfile = File::create(&entry_path)
                                .map_err(|e| format!("Cannot create {name}: {e}"))?;
                            std::io::copy(&mut entry, &mut outfile)
                                .map_err(|e| format!("Extract error {name}: {e}"))?;
                            extracted += 1;
                        }
                    }

                    Ok(format!(
                        "Extracted {extracted} files to {}",
                        out_dir.display()
                    ))
                })
                .await
                .map_err(|e| exec_err(format!("Spawn: {e}")))?
                .map_err(exec_err)?;

                Ok(ToolOutput::ok(result).into())
            }
            "list" => {
                check_read_access(Path::new(&archive_path)).map_err(exec_err)?;
                let result = tokio::task::spawn_blocking(move || -> Result<String, String> {
                    let file = File::open(&archive_path)
                        .map_err(|e| format!("Cannot open archive: {e}"))?;
                    let mut reader =
                        zip::ZipArchive::new(file).map_err(|e| format!("Invalid zip: {e}"))?;

                    let mut out =
                        format!("Contents of {} ({} entries):\n", archive_path, reader.len());
                    for i in 0..reader.len() {
                        let entry = reader
                            .by_index(i)
                            .map_err(|e| format!("Entry error: {e}"))?;
                        let name = entry.name();
                        let size = entry.size();
                        let compressed = entry.compressed_size();
                        let ratio = if size > 0 {
                            (compressed as f64 / size as f64 * 100.0) as u32
                        } else {
                            0
                        };
                        let dir = if entry.is_dir() { "DIR" } else { "FILE" };
                        out.push_str(&format!(
                            "  [{dir}] {name} ({size} bytes → {compressed}, {ratio}%)\n"
                        ));
                    }
                    Ok(out)
                })
                .await
                .map_err(|e| exec_err(format!("Spawn: {e}")))?
                .map_err(exec_err)?;

                Ok(ToolOutput::ok(result).into())
            }
            other => Err(exec_err(format!("Unknown action '{other}'"))),
        }
    }
}

fn add_dir_to_zip<W: Write + std::io::Seek>(
    zip: &mut ZipWriter<W>,
    base: &Path,
    dir: &Path,
    options: &SimpleFileOptions,
) -> Result<(), String> {
    for entry in std::fs::read_dir(dir).map_err(|e| format!("Read dir error: {e}"))? {
        let entry = entry.map_err(|e| format!("Entry error: {e}"))?;
        let path = entry.path();
        let relative = path
            .strip_prefix(base)
            .unwrap_or(&path)
            .to_string_lossy()
            .to_string();

        if path.is_dir() {
            zip.add_directory(&relative, *options)
                .map_err(|e| format!("Zip dir error: {e}"))?;
            add_dir_to_zip(zip, base, &path, options)?;
        } else {
            let mut f = BufReader::new(
                File::open(&path).map_err(|e| format!("Cannot open {}: {e}", path.display()))?,
            );
            zip.start_file(&relative, *options)
                .map_err(|e| format!("Zip error: {e}"))?;
            std::io::copy(&mut f, zip).map_err(|e| format!("Write error: {e}"))?;
        }
    }
    Ok(())
}
