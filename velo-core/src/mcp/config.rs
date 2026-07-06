use std::collections::HashMap;
use std::path::{Path, PathBuf};

use serde::Deserialize;
use serde_json::Value;
use tracing::{debug, info, warn};

use super::security::substitute_config_env_vars;
use super::types::{McpScope, McpServerConfig};

#[derive(Debug, Clone, Deserialize)]
struct McpConfigFile {
    #[serde(default)]
    mcp_servers: HashMap<String, McpServerConfig>,
}

pub fn load_mcp_config_file(
    path: &Path,
) -> Result<HashMap<String, McpServerConfig>, McpConfigError> {
    if !path.exists() {
        return Ok(HashMap::new());
    }

    let content = std::fs::read_to_string(path)
        .map_err(|e| McpConfigError::Io(format!("Cannot read {}: {e}", path.display())))?;

    let config: McpConfigFile = serde_json::from_str(&content)
        .map_err(|e| McpConfigError::Parse(format!("Cannot parse {}: {e}", path.display())))?;

    info!(
        "Loaded MCP config from {} ({} servers)",
        path.display(),
        config.mcp_servers.len()
    );
    Ok(config.mcp_servers)
}

pub fn discover_mcp_config_files(cwd: &Path, extra_paths: &[PathBuf]) -> Vec<(McpScope, PathBuf)> {
    let mut results: Vec<(McpScope, PathBuf)> = Vec::new();

    for extra in extra_paths {
        if extra.exists() {
            results.push((McpScope::Cli, extra.clone()));
        }
    }

    let project_dirs = find_project_mcp_configs(cwd);
    for dir in project_dirs {
        results.push((McpScope::Project, dir));
    }

    if let Some(user_config) = find_user_mcp_config() {
        results.push((McpScope::User, user_config));
    }

    let plugin_dirs = find_plugin_mcp_configs(cwd);
    for dir in plugin_dirs {
        results.push((McpScope::Plugin, dir));
    }

    results.dedup_by(|a, b| a.1 == b.1);
    results
}

fn find_user_mcp_config() -> Option<PathBuf> {
    let home_str = std::env::var("HOME")
        .or_else(|_| std::env::var("USERPROFILE"))
        .ok()?;
    let home = PathBuf::from(&home_str);
    let candidates = [home.join(".velo").join(".mcp.json"), home.join(".mcp.json")];
    for c in &candidates {
        if c.exists() {
            debug!("Found user MCP config at {}", c.display());
            return Some(c.clone());
        }
    }

    None
}

fn find_project_mcp_configs(cwd: &Path) -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    let home = std::env::var("HOME")
        .or_else(|_| std::env::var("USERPROFILE"))
        .ok()
        .map(PathBuf::from);

    let mut current = Some(cwd);
    let mut depth: u32 = 0;
    const MAX_WALK_DEPTH: u32 = 64;

    while let Some(dir) = current {
        if depth >= MAX_WALK_DEPTH {
            break;
        }

        let candidate = dir.join(".mcp.json");
        match candidate.try_exists() {
            Ok(true) if candidate.is_file() => {
                debug!("Found project MCP config at {}", candidate.display());
                dirs.push(candidate);
            }
            _ => {}
        }

        if let Some(ref home) = home {
            if dir == home.as_path() {
                break;
            }
        }

        current = dir.parent();
        depth += 1;
    }

    dirs.reverse();
    dirs
}

pub fn find_plugin_mcp_configs(project_root: &Path) -> Vec<PathBuf> {
    let mut configs = Vec::new();

    let marketplace = project_root.join(".velo-plugin").join("marketplace.json");
    if marketplace.exists() {
        if let Some(dirs) = extract_plugin_mcp_paths(&marketplace) {
            configs.extend(dirs);
        }
    }

    let plugin_root = project_root.join("plugins");
    if plugin_root.is_dir() {
        if let Ok(read_dir) = plugin_root.read_dir() {
            for entry in read_dir.flatten() {
                let plugin_dir = entry.path();
                if !plugin_dir.is_dir() {
                    continue;
                }

                let plugin_json = plugin_dir.join("plugin.json");
                if plugin_json.exists() {
                    if let Some(mcp_path) = read_plugin_mcp_servers_path(&plugin_json) {
                        let full_path = if let Some(relative) = mcp_path.strip_prefix("./") {
                            plugin_dir.join(relative)
                        } else {
                            plugin_dir.join(&mcp_path)
                        };
                        if full_path.exists() {
                            debug!("Found plugin MCP config at {}", full_path.display());
                            configs.push(full_path);
                        }
                    }
                }

                let plugin_mcp = plugin_dir.join(".mcp.json");
                if plugin_mcp.exists() {
                    debug!("Found plugin MCP config at {}", plugin_mcp.display());
                    configs.push(plugin_mcp);
                }
            }
        }
    }

    configs
}

fn read_plugin_mcp_servers_path(plugin_json_path: &Path) -> Option<String> {
    let content = std::fs::read_to_string(plugin_json_path).ok()?;
    let value: Value = serde_json::from_str(&content).ok()?;

    match value.get("mcpServers") {
        Some(Value::String(s)) => Some(s.clone()),
        Some(Value::Object(_)) => {
            let inline_path = plugin_json_path.parent()?.join(".mcp.json");
            if inline_path.exists() {
                return Some(inline_path.to_string_lossy().to_string());
            }
            None
        }
        _ => None,
    }
}

fn extract_plugin_mcp_paths(marketplace_path: &Path) -> Option<Vec<PathBuf>> {
    #[derive(Deserialize)]
    struct MarketplaceManifest {
        plugins: Vec<MarketplacePlugin>,
    }

    #[derive(Deserialize)]
    struct MarketplacePlugin {
        source: String,
        #[serde(default)]
        plugin_root: Option<String>,
    }

    let content = std::fs::read_to_string(marketplace_path).ok()?;
    let manifest: MarketplaceManifest = serde_json::from_str(&content).ok()?;

    let base = marketplace_path.parent()?.parent()?;
    let mut paths = Vec::new();

    for plugin in &manifest.plugins {
        let plugin_dir = if let Some(ref root) = plugin.plugin_root {
            base.join(root).join(&plugin.source)
        } else {
            base.join(&plugin.source)
        };

        let mcp_json = plugin_dir.join(".mcp.json");
        if mcp_json.exists() {
            paths.push(mcp_json);
        }

        let plugin_json = plugin_dir.join("plugin.json");
        if plugin_json.exists() {
            if let Some(mcp_path) = read_plugin_mcp_servers_path(&plugin_json) {
                let full = if let Some(relative) = mcp_path.strip_prefix("./") {
                    plugin_dir.join(relative)
                } else {
                    plugin_dir.join(&mcp_path)
                };
                if full.exists() && !paths.contains(&full) {
                    paths.push(full);
                }
            }
        }
    }

    Some(paths)
}

pub fn merge_mcp_configs(
    sources: Vec<(McpScope, HashMap<String, McpServerConfig>)>,
) -> Vec<(McpScope, String, McpServerConfig)> {
    let mut seen_keys: HashMap<String, McpScope> = HashMap::new();
    let mut merged: Vec<(McpScope, String, McpServerConfig)> = Vec::new();

    let scope_priority: HashMap<McpScope, u8> = [
        (McpScope::Plugin, 0),
        (McpScope::User, 1),
        (McpScope::Project, 2),
        (McpScope::Cli, 3),
    ]
    .iter()
    .cloned()
    .collect();

    for (scope, configs) in sources {
        for (name, mut config) in configs {
            substitute_config_env_vars(&mut config);

            if name == "workspace" {
                warn!("Skipping MCP server 'workspace' (reserved name)");
                continue;
            }

            if let Some(existing_scope) = seen_keys.get(&name) {
                let existing_priority = scope_priority.get(existing_scope).copied().unwrap_or(99);
                let new_priority = scope_priority.get(&scope).copied().unwrap_or(99);

                if new_priority > existing_priority {
                    debug!("MCP server '{name}' from {scope:?} overrides {existing_scope:?}");
                    merged.retain(|(_, n, _)| n != &name);
                } else {
                    debug!(
                        "MCP server '{name}' from {scope:?} kept (existing {existing_scope:?} has priority)"
                    );
                    continue;
                }
            }

            seen_keys.insert(name.clone(), scope);
            merged.push((scope, name, config));
        }
    }

    merged
}

pub fn discover_all_servers(extra_paths: &[PathBuf]) -> Vec<(McpScope, String, McpServerConfig)> {
    let cwd = match std::env::current_dir() {
        Ok(d) => d,
        Err(e) => {
            warn!("Cannot determine current directory for MCP discovery: {e}");
            return Vec::new();
        }
    };

    let config_files = discover_mcp_config_files(&cwd, extra_paths);

    let mut sources: Vec<(McpScope, HashMap<String, McpServerConfig>)> = Vec::new();
    for (scope, path) in config_files {
        match load_mcp_config_file(&path) {
            Ok(configs) => {
                if !configs.is_empty() {
                    sources.push((scope, configs));
                }
            }
            Err(e) => {
                warn!("Skipping MCP config {}: {e}", path.display());
            }
        }
    }

    merge_mcp_configs(sources)
}

#[derive(Debug)]
pub enum McpConfigError {
    Io(String),
    Parse(String),
}

impl std::fmt::Display for McpConfigError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Io(msg) => write!(f, "IO error: {msg}"),
            Self::Parse(msg) => write!(f, "Parse error: {msg}"),
        }
    }
}

impl std::error::Error for McpConfigError {}
