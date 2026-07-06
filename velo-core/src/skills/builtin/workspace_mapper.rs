use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use serde_json::Value;

use crate::memory::StorageBackend;
use crate::skills::{
    AgentSkill, BundledScript, ExecutionContext, ReferenceDoc, ResourceFile, SkillError,
    SkillPriority, TriggerExample, ValidationReport,
};

pub struct WorkspaceMapper;

#[async_trait]
impl AgentSkill for WorkspaceMapper {
    fn name(&self) -> &'static str {
        "workspace_mapper"
    }

    fn version(&self) -> &'static str {
        "1.0.0"
    }

    fn author(&self) -> Option<&'static str> {
        Some("Velo Core")
    }

    fn tags(&self) -> Vec<&'static str> {
        vec!["filesystem", "project-structure", "analysis", "codebase"]
    }

    fn description(&self) -> String {
        "Scans a local directory and produces a structured map of the project, \
         listing top-level files, directories, key manifests, and architectural \
         targets. This skill should be used when the user asks to 'map a workspace', \
         'analyze project structure', 'understand codebase layout', 'list project \
         files', 'scan directory structure', or before making structural changes \
         to understand the codebase layout."
            .to_string()
    }

    fn trigger_examples(&self) -> Vec<TriggerExample> {
        vec![
            TriggerExample {
                pattern: "map the workspace",
                description: "User wants a structural overview of the current project",
            },
            TriggerExample {
                pattern: "understand the project layout",
                description:
                    "User needs to understand directory organization before making changes",
            },
            TriggerExample {
                pattern: "analyze codebase structure",
                description: "User wants to identify architectural patterns and file organization",
            },
        ]
    }

    fn parameters(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "path": {
                    "type": "string",
                    "description": "Absolute path to the workspace root directory"
                },
                "max_depth": {
                    "type": "integer",
                    "description": "Maximum directory traversal depth",
                    "default": 5,
                    "minimum": 1,
                    "maximum": 20
                },
                "include_hidden": {
                    "type": "boolean",
                    "description": "Whether to include hidden files and directories (dotfiles)",
                    "default": false
                },
                "include_patterns": {
                    "type": "array",
                    "items": { "type": "string" },
                    "description": "Optional glob-like patterns to filter included files (e.g. '*.rs')"
                }
            },
            "required": ["path"]
        })
    }

    fn localized_instruction(&self) -> Option<String> {
        Some(
            "You have scanned the workspace structure. Use the resulting map \
             to understand the project layout before making architectural \
             decisions. Pay attention to key manifests (Cargo.toml, package.json, \
             etc.) which indicate the type and structure of the project."
                .to_string(),
        )
    }

    fn reference_material(&self) -> Vec<ReferenceDoc> {
        vec![
            ReferenceDoc {
                name: "file-classification",
                description: "How files are classified by extension and manifest detection",
                content: "Files are classified into: manifest (Cargo.toml, package.json, Makefile, etc.), \
                          language-specific (rust, python, typescript, go, java, etc.), \
                          config (toml, yaml, json), documentation (markdown), and other.",
            },
            ReferenceDoc {
                name: "performance-notes",
                description: "Performance characteristics and limits of the scanner",
                content: "The scanner respects max_depth to avoid excessive traversal. \
                          Very large repositories (>10k files) may take several seconds. \
                          Hidden directories are skipped by default. Symlinks are not followed.",
            },
        ]
    }

    fn bundled_scripts(&self) -> Vec<BundledScript> {
        vec![BundledScript {
            name: "count_entries",
            language: "inline",
            description: "Counts the number of entries in a directory without traversal",
            code: "std::fs::read_dir(path).map(|rd| rd.count()).unwrap_or(0)",
        }]
    }

    fn default_timeout(&self) -> Duration {
        Duration::from_secs(120)
    }

    fn priority(&self) -> SkillPriority {
        SkillPriority::Normal
    }

    fn path_triggers(&self) -> Vec<&'static str> {
        vec!["*.rs", "Cargo.toml", "src/"]
    }

    fn aliases(&self) -> Vec<&'static str> {
        vec!["map_workspace", "project_map", "structuralscan"]
    }

    fn argument_hint(&self) -> Option<&'static str> {
        Some(r#"{"path": "/path/to/project", "max_depth": 3}"#)
    }

    fn allowed_tools(&self) -> Option<Vec<&'static str>> {
        Some(vec!["Bash", "Read", "Glob", "Grep"])
    }

    fn model_override(&self) -> Option<&'static str> {
        None
    }

    fn user_invocable(&self) -> bool {
        true
    }

    fn disable_model_invocation(&self) -> bool {
        false
    }

    fn execution_context(&self) -> ExecutionContext {
        ExecutionContext::Inline
    }

    fn agent_override(&self) -> Option<&'static str> {
        None
    }

    fn resource_files(&self) -> Vec<ResourceFile> {
        vec![ResourceFile {
            name: "workspace_mapper_summary_prompt",
            content: "You are analyzing a workspace structure. Use the file \
                       classification and entry count information to understand \
                       which directories are most relevant to the user's request.",
        }]
    }

    fn dependencies(&self) -> Vec<&'static str> {
        vec![]
    }

    fn validate(&self) -> ValidationReport {
        let mut report = ValidationReport::default();
        if self.description().len() < 20 {
            report = report.with_warning("Description should be more descriptive");
        }
        if self.trigger_examples().is_empty() {
            report = report.with_warning("No trigger examples defined");
        }
        report
    }

    async fn execute(&self, args: Value, _ctx: Arc<StorageBackend>) -> Result<Value, SkillError> {
        let path_str = args
            .get("path")
            .and_then(|v| v.as_str())
            .ok_or_else(|| SkillError::Validation("Missing required 'path' argument".into()))?;

        let max_depth = args.get("max_depth").and_then(|v| v.as_u64()).unwrap_or(5) as usize;

        let include_hidden = args
            .get("include_hidden")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);

        let root = std::path::Path::new(path_str);
        if !root.exists() {
            return Err(SkillError::Validation(format!(
                "Path does not exist: {path_str}"
            )));
        }
        if !root.is_dir() {
            return Err(SkillError::Validation(format!(
                "Path is not a directory: {path_str}"
            )));
        }

        let canonical_root = root
            .canonicalize()
            .map_err(|e| SkillError::Execution(format!("Failed to canonicalize path: {e}")))?
            .to_string_lossy()
            .to_string();

        let mut entries: Vec<Value> = Vec::new();
        let mut total_files: usize = 0;
        let mut total_dirs: usize = 0;
        let mut max_reached_depth: usize = 0;

        let walker = walkdir::WalkDir::new(root)
            .max_depth(max_depth)
            .follow_links(false)
            .into_iter()
            .filter_entry(move |entry| {
                if !include_hidden {
                    let file_name = entry.file_name().to_string_lossy();
                    if file_name.starts_with('.') && entry.depth() > 0 {
                        return !entry.file_type().is_dir();
                    }
                }
                true
            });

        for result in walker {
            let entry = result.map_err(|e| {
                SkillError::Io(std::io::Error::other(format!("WalkDir error: {e}")))
            })?;

            let depth = entry.depth();
            max_reached_depth = max_reached_depth.max(depth);

            let relative = entry
                .path()
                .strip_prefix(root)
                .unwrap_or(entry.path())
                .to_string_lossy()
                .to_string();

            if relative.is_empty() {
                continue;
            }

            let ft = entry.file_type();
            if ft.is_dir() {
                total_dirs += 1;
                let entry_count = count_dir_entries(entry.path());
                entries.push(serde_json::json!({
                    "type": "directory",
                    "path": relative,
                    "depth": depth,
                    "entry_count": entry_count,
                }));
            } else if ft.is_file() {
                total_files += 1;
                let meta = entry.metadata().ok();
                let size = meta.as_ref().map(|m| m.len()).unwrap_or(0);
                let kind = classify_file(entry.path());

                entries.push(serde_json::json!({
                    "type": "file",
                    "path": relative,
                    "size": size,
                    "kind": kind,
                    "depth": depth,
                }));
            }
        }

        Ok(serde_json::json!({
            "root": canonical_root,
            "total_files": total_files,
            "total_directories": total_dirs,
            "max_depth": max_reached_depth,
            "entries": entries,
            "metadata": {
                "scanner_version": self.version(),
                "include_hidden": include_hidden,
            }
        }))
    }
}

fn classify_file(path: &std::path::Path) -> &'static str {
    match path.file_name().and_then(|n| n.to_str()).unwrap_or("") {
        "Cargo.toml" | "Package.swift" | "go.mod" | "build.gradle" | "pom.xml" | "package.json"
        | "CMakeLists.txt" | "Makefile" | "composer.json" => "manifest",
        _ => match path.extension().and_then(|e| e.to_str()).unwrap_or("") {
            "rs" => "rust",
            "py" => "python",
            "ts" | "tsx" => "typescript",
            "js" | "jsx" => "javascript",
            "go" => "golang",
            "java" => "java",
            "toml" => "toml",
            "yaml" | "yml" => "yaml",
            "json" => "json",
            "md" => "markdown",
            "html" => "html",
            "css" | "scss" => "stylesheet",
            "sql" => "sql",
            "sh" | "bash" | "zsh" => "shell",
            "dockerfile" | "Dockerfile" => "docker",
            "proto" => "protobuf",
            "vue" => "vue",
            "svelte" => "svelte",
            _ => "other",
        },
    }
}

fn count_dir_entries(path: &std::path::Path) -> usize {
    std::fs::read_dir(path).map(|rd| rd.count()).unwrap_or(0)
}
