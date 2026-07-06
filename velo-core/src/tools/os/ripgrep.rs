use regex::Regex;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use async_trait::async_trait;
use autoagents::core::tool::{ToolCallError, ToolInputT, ToolRuntime, ToolT};
use autoagents_derive::tool;

use crate::tools::{exec_err, ToolOutput};
use walkdir::WalkDir;

const MATCH_LIMIT: usize = 100;

const BINARY_EXTENSIONS: &[&str] = &[
    "png", "jpg", "jpeg", "gif", "bmp", "ico", "webp", "mp3", "mp4", "avi", "mov", "mkv", "wav",
    "flac", "ogg", "pdf", "doc", "docx", "xls", "xlsx", "ppt", "pptx", "zip", "tar", "gz", "bz2",
    "xz", "7z", "rar", "exe", "dll", "so", "dylib", "bin", "dat", "class", "pyc", "pyo", "o",
    "obj", "lib", "a", "deb", "rpm", "iso", "img", "ttf", "otf", "woff", "woff2", "eot", "psd",
    "ai", "svgz",
];

static ALLOWED_FILE_TYPES: &[(&str, &[&str])] = &[
    ("rust", &["rs"]),
    ("python", &["py", "pyi"]),
    ("javascript", &["js", "mjs", "cjs"]),
    ("typescript", &["ts", "tsx"]),
    ("json", &["json"]),
    ("yaml", &["yaml", "yml"]),
    ("markdown", &["md"]),
    ("html", &["html", "htm"]),
    ("css", &["css"]),
    ("toml", &["toml"]),
    ("shell", &["sh", "bash", "zsh"]),
    ("c", &["c", "h"]),
    ("cpp", &["cpp", "hpp", "cc", "hh"]),
    ("go", &["go"]),
    ("java", &["java"]),
    ("ruby", &["rb"]),
    ("php", &["php"]),
    ("swift", &["swift"]),
    ("kotlin", &["kt", "kts"]),
    ("scala", &["scala"]),
    ("lua", &["lua"]),
    ("sql", &["sql"]),
];

#[derive(Serialize, Deserialize, Debug)]
pub struct RipgrepArgs {
    pub pattern: String,
    pub path: Option<String>,
    pub file_type: Option<String>,
    pub max_matches: Option<usize>,
}

impl ToolInputT for RipgrepArgs {
    fn io_schema() -> &'static str {
        r#"{"type":"object","properties":{"pattern":{"type":"string","description":"Regex pattern to search for in file contents. Uses Rust regex syntax. Examples: 'fn main', 'TODO:', '\\d{3}-\\d{4}', 'import\\s+React'."},"path":{"type":"string","description":"Directory or file to search (recursive). Defaults to current working directory."},"file_type":{"type":"string","description":"Filter by file type/language: rust, python, javascript, typescript, json, yaml, markdown, html, css, toml, shell, c, cpp, go, java, ruby, php, swift, kotlin, scala, lua, sql."},"max_matches":{"type":"integer","description":"Maximum number of matches to return. Default: 100, Max: 500."}}}"#
    }
}

#[tool(name = "ripgrep", description = "Regex search across file contents (not filenames). Supports file type filtering (e.g. only .rs or .py files) and automatic binary file skipping. Max 500 results. BEST FOR: finding code patterns, searching logs, grep-like text search. Use find_file to search by FILENAME. Use read_file to read a specific file's content. Use shell (grep) for more complex piping or chaining.", input = RipgrepArgs)]
#[derive(Default, Clone)]
pub struct RipgrepTool;

#[async_trait]
impl ToolRuntime for RipgrepTool {
    async fn execute(&self, args: Value) -> Result<Value, ToolCallError> {
        let a: RipgrepArgs = serde_json::from_value(args)?;
        let path = a.path.unwrap_or_else(|| ".".into());
        let max_matches = a.max_matches.unwrap_or(MATCH_LIMIT).min(500);

        let re = Regex::new(&a.pattern).map_err(|e| exec_err(format!("Invalid regex: {e}")))?;

        let allowed_exts: Option<Vec<&str>> = a.file_type.as_ref().map(|ft| {
            ALLOWED_FILE_TYPES
                .iter()
                .filter(|(name, _)| *name == ft)
                .flat_map(|(_, exts)| exts.iter().copied())
                .collect()
        });

        let walker = WalkDir::new(&path)
            .follow_links(false)
            .into_iter()
            .filter_entry(|e| {
                let name = e.file_name().to_str().unwrap_or("");
                !name.starts_with('.')
            });

        let mut all_results = Vec::new();
        let mut count = 0usize;

        for entry in walker {
            if count >= max_matches {
                break;
            }

            let entry = match entry {
                Ok(e) => e,
                Err(_) => continue,
            };

            let file_path = entry.path();
            if !file_path.is_file() {
                continue;
            }

            let ext = file_path.extension().and_then(|e| e.to_str()).unwrap_or("");
            if BINARY_EXTENSIONS.contains(&ext) {
                continue;
            }

            if let Some(ref exts) = allowed_exts {
                if !exts.contains(&ext) {
                    continue;
                }
            }

            let Ok(content) = std::fs::read_to_string(file_path) else {
                continue;
            };

            let mut file_count = 0usize;
            for (line_idx, line) in content.lines().enumerate() {
                if file_count >= 20 || count >= max_matches {
                    break;
                }
                if re.is_match(line) {
                    let trimmed = if line.len() > 200 {
                        format!("{}...", &line[..200])
                    } else {
                        line.to_string()
                    };
                    all_results.push(format!(
                        "  {}:{} {}",
                        file_path.display(),
                        line_idx + 1,
                        trimmed
                    ));
                    file_count += 1;
                    count += 1;
                }
            }
        }

        if all_results.is_empty() {
            Ok(ToolOutput::ok(format!("No matches found for pattern: {}", a.pattern)).into())
        } else {
            let out = format!("{} matches:\n{}", all_results.len(), all_results.join("\n"));
            Ok(ToolOutput::ok(out).into())
        }
    }
}
