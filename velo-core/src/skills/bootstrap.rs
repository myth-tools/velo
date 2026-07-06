use std::collections::HashSet;
use std::path::{Path, PathBuf};

use serde::Deserialize;
use tracing::{debug, info, warn};

use super::{register_default_skills, SkillError, SkillManagerHandle, SkillSource};

const AGENT_PROJECT_SKILL_DIRS: &[&str] = &[".velo/skills", ".agents/skills"];

const AGENT_SKIP_DIRS: &[&str] = &[
    "node_modules",
    ".git",
    "dist",
    "build",
    "__pycache__",
    ".DS_Store",
    "target",
];

const COMMANDS_DIRS: &[&str] = &[".velo/commands"];

#[derive(Debug)]
pub struct DiscoveredLocations {
    pub user_skill_dir: Option<PathBuf>,
    pub project_skill_dirs: Vec<PathBuf>,
    pub plugin_dirs: Vec<PathBuf>,
    pub commands_dirs: Vec<PathBuf>,
}

pub fn discover_skill_directories() -> DiscoveredLocations {
    let user_skill_dir = find_user_skill_dir();
    let project_skill_dirs = find_project_skill_dirs();
    let plugin_dirs = find_plugin_dirs();
    let commands_dirs = find_commands_dirs();

    DiscoveredLocations {
        user_skill_dir,
        project_skill_dirs,
        plugin_dirs,
        commands_dirs,
    }
}

fn find_user_skill_dir() -> Option<PathBuf> {
    let home = match std::env::var("HOME").or_else(|_| std::env::var("USERPROFILE")) {
        Ok(h) => PathBuf::from(h),
        Err(_) => {
            warn!("Cannot determine home directory, skipping user skills");
            return None;
        }
    };

    let candidates = [
        home.join(".velo").join("skills"),
        home.join(".agents").join("skills"),
    ];

    for candidate in &candidates {
        match candidate.try_exists() {
            Ok(true) if candidate.is_dir() => {
                info!("Found user skills directory: {}", candidate.display());
                return Some(candidate.clone());
            }
            Ok(_) => {}
            Err(e) => debug!("Cannot access {}: {e}", candidate.display()),
        }
    }

    debug!("No user skills directory found under {}", home.display());
    None
}

fn find_project_skill_dirs() -> Vec<PathBuf> {
    let cwd = match std::env::current_dir() {
        Ok(d) => d,
        Err(e) => {
            warn!("Cannot determine current directory: {e}");
            return Vec::new();
        }
    };

    let home = std::env::var("HOME")
        .or_else(|_| std::env::var("USERPROFILE"))
        .ok()
        .map(PathBuf::from);

    let mut all_found: Vec<(usize, PathBuf)> = Vec::new();
    let mut current = Some(cwd.as_path());
    let mut depth: u32 = 0;
    const MAX_WALK_DEPTH: u32 = 64;

    while let Some(dir) = current {
        if depth >= MAX_WALK_DEPTH {
            debug!("Reached max walk depth {MAX_WALK_DEPTH}, stopping upward search");
            break;
        }

        for skill_dir_rel in AGENT_PROJECT_SKILL_DIRS {
            let candidate = dir.join(skill_dir_rel);
            match candidate.try_exists() {
                Ok(true) if candidate.is_dir() => {
                    info!(
                        "Found project skills directory: {} (from {skill_dir_rel})",
                        candidate.display()
                    );
                    all_found.push((depth as usize, candidate));
                }
                Ok(_) => {}
                Err(e) => debug!("Cannot access {}: {e}", candidate.display()),
            }
        }

        for cmd_dir in COMMANDS_DIRS {
            let commands_candidate = dir.join(cmd_dir);
            if matches!(commands_candidate.try_exists(), Ok(true) if commands_candidate.is_dir()) {
                debug!("Found commands directory: {}", commands_candidate.display());
            }
        }

        if let Some(ref home) = home {
            if dir == home.as_path() {
                break;
            }
        }

        current = dir.parent();
        depth += 1;
    }

    all_found.sort_by_key(|(d, _)| *d);
    all_found.dedup_by_key(|(_, p)| p.clone());

    let dirs: Vec<PathBuf> = all_found.into_iter().map(|(_, p)| p).collect();
    dirs
}

fn find_plugin_dirs() -> Vec<PathBuf> {
    let cwd = match std::env::current_dir() {
        Ok(d) => d,
        Err(e) => {
            warn!("Cannot determine current directory for plugin search: {e}");
            return Vec::new();
        }
    };

    let mut dirs = Vec::new();
    let home = std::env::var("HOME")
        .or_else(|_| std::env::var("USERPROFILE"))
        .ok()
        .map(PathBuf::from);

    let mut current = Some(cwd.as_path());
    let mut depth: u32 = 0;
    const MAX_WALK_DEPTH: u32 = 64;

    while let Some(dir) = current {
        if depth >= MAX_WALK_DEPTH {
            break;
        }

        let marketplace = dir.join(".velo-plugin").join("marketplace.json");
        if marketplace.try_exists().unwrap_or(false) {
            if let Some(plugin_skill_dirs) = parse_marketplace_manifest(&marketplace) {
                for psd in plugin_skill_dirs {
                    if psd.try_exists().unwrap_or(false) && psd.is_dir() {
                        info!("Found plugin skill dir: {}", psd.display());
                        dirs.push(psd);
                    }
                }
            }
        }

        let plugin_json = dir.join(".velo-plugin").join("plugin.json");
        if plugin_json.try_exists().unwrap_or(false) {
            if let Some(plugin_skill_dirs) = parse_plugin_manifest(&plugin_json) {
                for psd in plugin_skill_dirs {
                    if psd.try_exists().unwrap_or(false) && psd.is_dir() {
                        info!("Found plugin skill dir: {}", psd.display());
                        dirs.push(psd);
                    }
                }
            }
        }

        if let Some(ref home) = home {
            if dir == home.as_path() {
                break;
            }
        }

        current = dir.parent();
        depth += 1;
    }

    dirs
}

#[derive(Debug, Deserialize)]
struct MarketplaceManifest {
    plugins: Vec<PluginEntry>,
}

#[derive(Debug, Deserialize)]
struct PluginEntry {
    source: String,
    #[serde(default)]
    plugin_root: Option<String>,
}

fn parse_marketplace_manifest(path: &Path) -> Option<Vec<PathBuf>> {
    let content = std::fs::read_to_string(path).ok()?;
    let manifest: MarketplaceManifest = serde_json::from_str(&content).ok()?;

    let base_dir = path.parent()?.parent()?;
    let plugin_dir = path.parent()?;

    let mut dirs = Vec::new();
    for plugin in &manifest.plugins {
        let plugin_path = if let Some(ref root) = plugin.plugin_root {
            base_dir.join(root).join(&plugin.source)
        } else {
            base_dir.join(&plugin.source)
        };

        let skills_dir = plugin_path.join("skills");
        if skills_dir.is_dir() {
            dirs.push(skills_dir);
        }

        let single_skill = plugin_path.join("SKILL.md");
        if single_skill.is_file() {
            dirs.push(plugin_path);
        }
    }

    if plugin_dir.join("skills").is_dir() {
        dirs.push(plugin_dir.join("skills"));
    }

    Some(dirs)
}

#[derive(Debug, Deserialize)]
struct PluginManifest {
    #[serde(default)]
    skills: Vec<String>,
    #[serde(default)]
    plugin_root: Option<String>,
}

fn parse_plugin_manifest(path: &Path) -> Option<Vec<PathBuf>> {
    let content = std::fs::read_to_string(path).ok()?;
    let manifest: PluginManifest = serde_json::from_str(&content).ok()?;

    let base = path.parent()?;
    let mut dirs = Vec::new();

    for skill_path in &manifest.skills {
        let full_path = if let Some(ref root) = manifest.plugin_root {
            base.join(root).join(skill_path)
        } else {
            base.join(skill_path)
        };
        dirs.push(full_path);
    }

    Some(dirs)
}

fn find_commands_dirs() -> Vec<PathBuf> {
    let cwd = match std::env::current_dir() {
        Ok(d) => d,
        Err(e) => {
            warn!("Cannot determine current directory for commands search: {e}");
            return Vec::new();
        }
    };

    let home = std::env::var("HOME")
        .or_else(|_| std::env::var("USERPROFILE"))
        .ok()
        .map(PathBuf::from);

    let mut dirs = Vec::new();
    let mut current = Some(cwd.as_path());
    let mut depth: u32 = 0;
    const MAX_WALK_DEPTH: u32 = 64;

    while let Some(dir) = current {
        if depth >= MAX_WALK_DEPTH {
            break;
        }

        for cmd_dir in COMMANDS_DIRS {
            let candidate = dir.join(cmd_dir);
            if matches!(candidate.try_exists(), Ok(true) if candidate.is_dir()) {
                dirs.push(candidate);
            }
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

pub fn has_skill_md(dir: &Path) -> bool {
    dir.join("SKILL.md").is_file()
}

pub fn parse_skill_md(skill_md_path: &Path) -> Option<ParsedSkill> {
    let content = std::fs::read_to_string(skill_md_path).ok()?;
    let frontmatter = parse_yaml_frontmatter(&content)?;

    let name = frontmatter.get("name")?;
    let description = frontmatter.get("description")?;

    let is_internal = frontmatter
        .get("metadata")
        .map(|m| {
            if let Some(inner) = m
                .strip_prefix('{')
                .and_then(|stripped| stripped.strip_suffix('}'))
            {
                inner.contains("internal") && inner.contains("true")
            } else {
                false
            }
        })
        .unwrap_or(false);

    if is_internal {
        let env_var = std::env::var("INSTALL_INTERNAL_SKILLS").unwrap_or_default();
        if env_var != "true" && env_var != "1" {
            debug!("Skipping internal skill: {}", name);
            return None;
        }
    }

    let version = frontmatter.get("version").cloned();
    let license = frontmatter.get("license").cloned();

    Some(ParsedSkill {
        name: name.to_string(),
        description: description.to_string(),
        path: skill_md_path.parent()?.to_path_buf(),
        raw_content: content,
        version,
        license,
        is_internal,
    })
}

fn parse_yaml_frontmatter(content: &str) -> Option<HashMap<String, String>> {
    let trimmed = content.trim_start();
    if !trimmed.starts_with("---") {
        return None;
    }
    let end = trimmed[3..].find("---")?;
    let frontmatter = &trimmed[3..3 + end];
    let mut map = HashMap::new();
    let mut current_key: Option<String> = None;
    let mut current_value = String::new();

    for line in frontmatter.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        if let Some(eq) = line.find(':') {
            if let Some(prev_key) = current_key.take() {
                let trimmed_val = current_value.trim().to_string();
                if !trimmed_val.is_empty() {
                    map.insert(prev_key, trimmed_val);
                }
                current_value.clear();
            }
            let key = line[..eq].trim().to_string();
            let value = line[eq + 1..].trim().to_string();
            current_key = Some(key);
            current_value = value;
        } else {
            if !current_value.is_empty() {
                current_value.push(' ');
            }
            current_value.push_str(line);
        }
    }

    if let Some(key) = current_key {
        let trimmed_val = current_value.trim().to_string();
        if !trimmed_val.is_empty() {
            map.insert(key, trimmed_val);
        }
    }

    Some(map)
}

#[derive(Debug, Clone)]
pub struct ParsedSkill {
    pub name: String,
    pub description: String,
    pub path: PathBuf,
    pub raw_content: String,
    pub version: Option<String>,
    pub license: Option<String>,
    pub is_internal: bool,
}

pub fn find_skill_dirs_recursive(
    root: &Path,
    current_depth: usize,
    max_depth: usize,
    skip_dirs: &HashSet<String>,
) -> Vec<PathBuf> {
    let mut results = Vec::new();

    if current_depth > max_depth {
        return results;
    }

    if current_depth > 0 && has_skill_md(root) {
        results.push(root.to_path_buf());
        return results;
    }

    let read_dir = match root.read_dir() {
        Ok(rd) => rd,
        Err(_) => return results,
    };

    for entry in read_dir.flatten() {
        let path = entry.path();
        let file_name = entry.file_name().to_string_lossy().to_string();

        if skip_dirs.contains(&file_name) {
            continue;
        }

        let ft = match entry.file_type() {
            Ok(t) => t,
            Err(_) => continue,
        };

        if ft.is_dir() {
            results.extend(find_skill_dirs_recursive(
                &path,
                current_depth + 1,
                max_depth,
                skip_dirs,
            ));
        }
    }

    results
}

fn collect_skip_dirs() -> HashSet<String> {
    AGENT_SKIP_DIRS.iter().map(|s| s.to_string()).collect()
}

pub fn discover_skills_in_dir(dir: &Path) -> Vec<ParsedSkill> {
    let mut skills = Vec::new();
    let read_dir = match dir.read_dir() {
        Ok(rd) => rd,
        Err(e) => {
            debug!("Cannot read skill directory {}: {e}", dir.display());
            return skills;
        }
    };

    for entry in read_dir.flatten() {
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }
        let skill_md = path.join("SKILL.md");
        if !skill_md.is_file() {
            continue;
        }
        if let Some(parsed) = parse_skill_md(&skill_md) {
            skills.push(parsed);
        }
    }

    skills
}

pub async fn bootstrap_skills(
    handle: &SkillManagerHandle,
    bundle_first: bool,
) -> Result<(), SkillError> {
    if bundle_first {
        let builtin_handle = handle.clone();
        tokio::task::spawn_blocking(move || {
            register_default_skills(&builtin_handle);
        })
        .await
        .map_err(|e| SkillError::Internal(format!("Bundled skill registration failed: {e}")))?;
    }

    let discovery = discover_skill_directories();

    if let Some(user_dir) = &discovery.user_skill_dir {
        let discovered = discover_skills_in_dir(user_dir);
        for skill in &discovered {
            load_parsed_skill(handle, skill, SkillSource::User).await;
        }
        if !discovered.is_empty() {
            info!(
                "Loaded {} user skills from {}",
                discovered.len(),
                user_dir.display()
            );
        }
    }

    for project_dir in &discovery.project_skill_dirs {
        let discovered = discover_skills_in_dir(project_dir);
        for skill in &discovered {
            load_parsed_skill(handle, skill, SkillSource::Project).await;
        }
        if !discovered.is_empty() {
            info!(
                "Loaded {} project skills from {}",
                discovered.len(),
                project_dir.display()
            );
        }
    }

    for plugin_dir in &discovery.plugin_dirs {
        let discovered = discover_skills_in_dir(plugin_dir);
        for skill in &discovered {
            load_parsed_skill(handle, skill, SkillSource::Plugin).await;
        }
        if !discovered.is_empty() {
            info!(
                "Loaded {} plugin skills from {}",
                discovered.len(),
                plugin_dir.display()
            );
        }
    }

    let should_fallback = discovery.user_skill_dir.is_none()
        && discovery.project_skill_dirs.is_empty()
        && discovery.plugin_dirs.is_empty();
    if should_fallback {
        let cwd = match std::env::current_dir() {
            Ok(d) => d,
            Err(_) => return Ok(()),
        };
        let skip_dirs = collect_skip_dirs();
        let found = find_skill_dirs_recursive(&cwd, 0, 5, &skip_dirs);
        if !found.is_empty() {
            info!(
                "Fallback recursive search found {} skill directories",
                found.len()
            );
            for skill_dir in found {
                let skill_md = skill_dir.join("SKILL.md");
                if let Some(parsed) = parse_skill_md(&skill_md) {
                    load_parsed_skill(handle, &parsed, SkillSource::Project).await;
                }
            }
        }
    }

    for commands_dir in &discovery.commands_dirs {
        let loaded = load_commands_from_directory(handle, commands_dir).await;
        if loaded > 0 {
            info!("Loaded {loaded} commands from {}", commands_dir.display());
        }
    }

    if !bundle_first {
        let builtin_handle = handle.clone();
        tokio::task::spawn_blocking(move || {
            register_default_skills(&builtin_handle);
        })
        .await
        .map_err(|e| SkillError::Internal(format!("Bundled skill registration failed: {e}")))?;
    }

    Ok(())
}

async fn load_parsed_skill(handle: &SkillManagerHandle, parsed: &ParsedSkill, source: SkillSource) {
    let path_str = parsed.path.to_string_lossy().to_string();
    match handle.load_skills_from_directory(&path_str, source).await {
        Ok(names) => {
            if names.is_empty() {
                debug!(
                    "No skills loaded from {} (parsed but not loaded?)",
                    path_str
                );
            }
        }
        Err(e) => {
            warn!(
                "Failed to load skill '{}' from {}: {e}",
                parsed.name, path_str
            );
        }
    }
}

pub async fn load_skills_from_directory_robust(
    handle: &SkillManagerHandle,
    path: &Path,
    source: SkillSource,
) -> Vec<String> {
    let path_str = path.to_string_lossy().to_string();
    match handle.load_skills_from_directory(&path_str, source).await {
        Ok(names) => {
            if !names.is_empty() {
                info!(
                    "Loaded {} skills from {} ({:?})",
                    names.len(),
                    path_str,
                    source
                );
            }
            names
        }
        Err(e) => {
            warn!("Skipping skill directory {}: {e}", path_str);
            Vec::new()
        }
    }
}

pub async fn load_commands_from_directory(handle: &SkillManagerHandle, dir: &Path) -> usize {
    let read_dir = match dir.read_dir() {
        Ok(rd) => rd,
        Err(e) => {
            warn!("Cannot read commands directory {}: {e}", dir.display());
            return 0;
        }
    };

    let mut count: usize = 0;
    for entry in read_dir.flatten() {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("md") {
            continue;
        }

        let content = match std::fs::read_to_string(&path) {
            Ok(c) => c,
            Err(e) => {
                warn!("Cannot read command file {}: {e}", path.display());
                continue;
            }
        };

        let command_name = path
            .file_stem()
            .and_then(|s| s.to_str())
            .map(|s| s.to_string())
            .unwrap_or_default();

        if command_name.is_empty() {
            continue;
        }

        let frontmatter = parse_yaml_frontmatter(&content);
        let description = frontmatter
            .as_ref()
            .and_then(|m| m.get("description").cloned())
            .unwrap_or_default();

        let command_skill = CommandSkill {
            name: command_name.clone(),
            description,
            content,
        };

        handle.register_with_source(std::sync::Arc::new(command_skill), SkillSource::Project);
        count += 1;
    }

    count
}

use std::collections::HashMap;
use std::sync::Arc;

use async_trait::async_trait;
use serde_json::Value;

use crate::memory::StorageBackend;
use crate::skills::trait_def::AgentSkill;

struct CommandSkill {
    name: String,
    description: String,
    content: String,
}

#[async_trait]
impl AgentSkill for CommandSkill {
    fn name(&self) -> &'static str {
        Box::leak(self.name.clone().into_boxed_str())
    }

    fn description(&self) -> String {
        self.description.clone()
    }

    fn parameters(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "properties": {}
        })
    }

    fn user_invocable(&self) -> bool {
        true
    }

    fn disable_model_invocation(&self) -> bool {
        false
    }

    fn execution_context(&self) -> super::ExecutionContext {
        super::ExecutionContext::Inline
    }

    async fn execute(&self, _args: Value, _ctx: Arc<StorageBackend>) -> Result<Value, SkillError> {
        Ok(serde_json::json!({
            "command": self.name,
            "content": self.content,
        }))
    }
}
