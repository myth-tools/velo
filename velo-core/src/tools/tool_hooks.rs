use std::sync::Arc;

use async_trait::async_trait;
use regex::Regex;
use serde::Deserialize;
use serde_json::Value;
use tokio::sync::RwLock;

use super::output::ToolOutput;

// ── Permission decision ───────────────────────────────────────────────────

/// Result of a permission or hook check.
#[derive(Debug, Clone, PartialEq)]
pub enum PermissionDecision {
    /// Tool may proceed.
    Allow,
    /// Tool is blocked entirely.
    Deny,
    /// Requires user approval (front-end modal).
    Ask,
}

impl PermissionDecision {
    pub fn is_allowed(&self) -> bool {
        matches!(self, Self::Allow)
    }

    pub fn label(&self) -> &'static str {
        match self {
            Self::Allow => "allowed",
            Self::Deny => "denied",
            Self::Ask => "ask",
        }
    }
}

// ── Hook matchers ─────────────────────────────────────────────────────────

/// Pattern used to match a hook against incoming tool calls.
#[derive(Debug, Clone)]
pub enum HookMatcher {
    /// Exact tool name match: `"Bash"`, `"ReadFile"`.
    Exact(String),
    /// Regex pattern over the full tool name: `"Read|Write|Edit"`.
    Pattern(Regex),
    /// Bash subcommand pattern: `"Bash(git commit:*)"`.
    BashSubcommand(BashPattern),
    /// Catches everything (`"*"`).
    Wildcard,
}

impl HookMatcher {
    /// Returns `true` if this matcher applies to the given tool call.
    pub fn matches(&self, tool_name: &str, tool_args: &Value) -> bool {
        match self {
            Self::Exact(name) => tool_name == name,
            Self::Pattern(re) => re.is_match(tool_name),
            Self::BashSubcommand(p) => {
                if tool_name != "shell" {
                    return false;
                }
                let cmd = tool_args
                    .get("command")
                    .and_then(|v| v.as_str())
                    .unwrap_or("");
                p.matches(cmd)
            }
            Self::Wildcard => true,
        }
    }
}

// ── Bash subcommand pattern ───────────────────────────────────────────────

/// Parsed pattern like `Bash(git:commit:*)` that restricts the `shell` tool
/// to specific subcommands.
///
/// Syntax: `<base>:<subcommand>[:<subcommand>]*`
///
/// Examples:
/// - `Bash(git:*)`           → allow any `git ...` command
/// - `Bash(git:commit:*)`    → allow any `git commit ...` command
/// - `Bash(git:add:commit)`  → allow `git add` and `git commit`
#[derive(Debug, Clone)]
pub struct BashPattern {
    /// The base command (e.g. "git")
    pub base: String,
    /// Allowed subcommand prefixes (e.g. ["add", "commit"])
    pub subcommands: Vec<String>,
}

impl BashPattern {
    /// Parse a pattern string like `Bash(git:*)` or `Bash(git:commit:*)`.
    pub fn parse(input: &str) -> Option<Self> {
        let input = input.trim();
        if !input.starts_with("Bash(") || !input.ends_with(')') {
            return None;
        }
        let inner = &input[5..input.len() - 1];
        let colon_pos = inner.find(':')?;
        let base = inner[..colon_pos].trim().to_string();
        if base.is_empty() || base.contains(' ') {
            return None;
        }

        let sub_part = inner[colon_pos + 1..].trim();
        let subcommands: Vec<String> = sub_part
            .split(':')
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .collect();

        if subcommands.is_empty() {
            return None;
        }

        Some(Self { base, subcommands })
    }

    /// Check whether a shell command matches this pattern.
    pub fn matches(&self, command: &str) -> bool {
        let trimmed = command.trim();
        let first_word = match trimmed.split_whitespace().next() {
            Some(w) => w,
            None => return false,
        };
        if first_word != self.base {
            return false;
        }
        let rest = trimmed[first_word.len()..].trim();

        // "*" as the only subcommand means any command starting with base
        if self.subcommands.len() == 1 && self.subcommands[0] == "*" {
            return true;
        }

        // Check if rest starts with any allowed subcommand pattern
        self.subcommands.iter().any(|sub| {
            if sub == "*" {
                false
            } else if sub.ends_with('*') {
                let prefix = &sub[..sub.len() - 1];
                rest.starts_with(prefix)
            } else {
                rest.starts_with(sub)
                    && (rest.len() == sub.len() || rest[sub.len()..].starts_with(' '))
            }
        })
    }
}

/// Parse a string that may be a Bash scoped pattern or a plain tool name.
/// Returns `None` for plain tool names (handled elsewhere).
pub fn parse_bash_pattern(s: &str) -> Option<BashPattern> {
    BashPattern::parse(s)
}

/// Check whether a shell command matches any of the given Bash patterns.
/// If patterns is empty, all commands are allowed.
pub fn bash_command_allowed(command: &str, patterns: &[BashPattern]) -> bool {
    if patterns.is_empty() {
        return true;
    }
    patterns.iter().any(|p| p.matches(command))
}

// ── Tool hook trait ───────────────────────────────────────────────────────

/// A single hook/middleware that runs before or after tool execution.
///
/// Hooks run in **parallel** for the same event.
#[async_trait]
pub trait ToolHook: Send + Sync + std::fmt::Debug {
    /// Unique name for this hook (used in logging and dedup).
    fn name(&self) -> &str;

    /// Matchers that determine which tool calls this hook applies to.
    fn matchers(&self) -> Vec<HookMatcher>;

    /// Called **before** a tool executes.
    ///
    /// Return `Ok(HookMatchResult::allow())` to proceed,
    /// `Ok(HookMatchResult::deny(msg))` to block with a message,
    /// or `Err(...)` for an unexpected error (also blocks).
    async fn on_pretool(
        &self,
        tool_name: &str,
        tool_args: &Value,
    ) -> Result<HookMatchResult, String>;

    /// Called **after** a tool executes successfully.
    async fn on_posttool(&self, tool_name: &str, result: &ToolOutput) -> Result<(), String>;
}

/// Result returned by a PreToolUse hook.
#[derive(Debug, Clone)]
pub struct HookMatchResult {
    pub decision: PermissionDecision,
    /// Human-readable explanation sent back to the LLM as a system message.
    pub system_message: Option<String>,
}

impl HookMatchResult {
    pub fn allow() -> Self {
        Self {
            decision: PermissionDecision::Allow,
            system_message: None,
        }
    }

    pub fn deny(msg: impl Into<String>) -> Self {
        Self {
            decision: PermissionDecision::Deny,
            system_message: Some(msg.into()),
        }
    }

    pub fn ask() -> Self {
        Self {
            decision: PermissionDecision::Ask,
            system_message: None,
        }
    }
}

// ── Hook registry ─────────────────────────────────────────────────────────

/// Thread-safe registry of ToolHook instances.
///
/// Supports programmatic registration AND config-file loading
/// (`.velo/hooks.json` in the project root).
#[derive(Debug, Default, Clone)]
pub struct HookRegistry {
    hooks: Arc<RwLock<Vec<RegisteredHook>>>,
}

#[derive(Debug, Clone)]
struct RegisteredHook {
    hook: Arc<dyn ToolHook>,
}

impl HookRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    /// Register a hook programmatically.
    pub fn register(&self, hook: Arc<dyn ToolHook>) {
        let mut guard = self.hooks.blocking_write();
        guard.push(RegisteredHook { hook });
    }

    /// Register a hook from a config file entry.
    pub fn register_from_config(&self, hook: Arc<dyn ToolHook>) {
        self.register(hook);
    }

    /// Return all registered hooks.
    pub fn all(&self) -> Vec<Arc<dyn ToolHook>> {
        let guard = self.hooks.blocking_read();
        guard.iter().map(|r| r.hook.clone()).collect()
    }

    /// Run ALL PreToolUse hooks for a given tool call, collecting results.
    ///
    /// Hooks run in parallel (via `tokio::join!`).
    /// If ANY hook returns `Deny`, the tool is blocked.
    /// If ANY hook returns `Ask`, the tool requires approval.
    pub async fn run_pretool(&self, tool_name: &str, tool_args: &Value) -> Vec<HookMatchResult> {
        let hooks = {
            let guard = self.hooks.read().await;
            guard
                .iter()
                .filter(|r| {
                    r.hook
                        .matchers()
                        .iter()
                        .any(|m| m.matches(tool_name, tool_args))
                })
                .map(|r| r.hook.clone())
                .collect::<Vec<_>>()
        };

        if hooks.is_empty() {
            return vec![];
        }

        let mut results = Vec::with_capacity(hooks.len());
        for hook in &hooks {
            match hook.on_pretool(tool_name, tool_args).await {
                Ok(r) => results.push(r),
                Err(e) => {
                    results.push(HookMatchResult::deny(format!(
                        "Hook '{}' error: {e}",
                        hook.name()
                    )));
                }
            }
        }
        results
    }

    /// Run ALL PostToolUse hooks for a completed tool call.
    pub async fn run_posttool(&self, tool_name: &str, result: &ToolOutput) {
        let hooks = {
            let guard = self.hooks.read().await;
            guard.iter().map(|r| r.hook.clone()).collect::<Vec<_>>()
        };

        for hook in &hooks {
            if let Err(e) = hook.on_posttool(tool_name, result).await {
                tracing::warn!("PostToolUse hook '{}' error: {e}", hook.name());
            }
        }
    }

    /// Aggregate multiple HookMatchResults into a single decision.
    /// Deny > Ask > Allow.
    pub fn aggregate(results: &[HookMatchResult]) -> HookMatchResult {
        let mut decision = PermissionDecision::Allow;
        let mut messages = Vec::new();

        for r in results {
            match &r.decision {
                PermissionDecision::Deny => {
                    decision = PermissionDecision::Deny;
                    if let Some(msg) = &r.system_message {
                        messages.push(msg.clone());
                    }
                }
                PermissionDecision::Ask if decision != PermissionDecision::Deny => {
                    decision = PermissionDecision::Ask;
                }
                _ => {}
            }
        }

        HookMatchResult {
            decision,
            system_message: if messages.is_empty() {
                None
            } else {
                Some(messages.join("\n"))
            },
        }
    }

    /// Check tool permissions from settings (ask / deny / allow lists).
    /// This is independent of hooks – it enforces the user's settings.
    pub fn check_settings(
        tool_name: &str,
        ask_list: &[String],
        deny_list: &[String],
    ) -> PermissionDecision {
        if deny_list.iter().any(|p| glob_match(p, tool_name)) {
            return PermissionDecision::Deny;
        }
        if ask_list.iter().any(|p| glob_match(p, tool_name)) {
            return PermissionDecision::Ask;
        }
        PermissionDecision::Allow
    }
}

// ── Simple glob matching for permission lists ────────────────────────────

fn glob_match(pattern: &str, name: &str) -> bool {
    let re_pattern = format!(
        "^{}$",
        regex::escape(pattern)
            .replace(r"\?", ".")
            .replace(r"\*", ".*")
    );
    Regex::new(&re_pattern).is_ok_and(|re| re.is_match(name))
}

// ── Config-file loading (JSON-based, no extra deps) ──────────────────────

/// A hook definition parsed from a config file (`.velo/hooks.json`).
#[derive(Debug, Clone, Deserialize)]
pub struct HookConfigEntry {
    pub name: String,
    #[serde(rename = "type")]
    pub hook_type: String,
    pub command: Option<String>,
    pub prompt: Option<String>,
    pub matcher: Option<String>,
    #[serde(default)]
    pub timeout: Option<u64>,
    #[serde(default)]
    pub async_rewake: bool,
}

/// Load hook configurations from a JSON file.
/// Expected location: `.velo/hooks.json` or `~/.velo/hooks.json`.
pub fn load_hooks_from_config(path: &std::path::Path) -> Result<Vec<HookConfigEntry>, String> {
    let content = std::fs::read_to_string(path)
        .map_err(|e| format!("Cannot read hook config {path:?}: {e}"))?;
    let entries: Vec<HookConfigEntry> =
        serde_json::from_str(&content).map_err(|e| format!("Invalid hook config: {e}"))?;
    Ok(entries)
}

/// Build HookMatcher instances from a comma/pipe-separated matcher string.
pub fn parse_matchers(input: &str) -> Vec<HookMatcher> {
    let parts: Vec<&str> = input.split(',').map(|s| s.trim()).collect();
    let mut matchers = Vec::new();
    for part in parts {
        if part == "*" {
            matchers.push(HookMatcher::Wildcard);
        } else if part.starts_with("Bash(") && part.ends_with(')') {
            if let Some(p) = BashPattern::parse(part) {
                matchers.push(HookMatcher::BashSubcommand(p));
            }
        } else if part.contains('|') || part.contains('*') {
            if let Ok(re) = Regex::new(part) {
                matchers.push(HookMatcher::Pattern(re));
            } else {
                matchers.push(HookMatcher::Exact(part.to_string()));
            }
        } else {
            matchers.push(HookMatcher::Exact(part.to_string()));
        }
    }
    matchers
}

// ── Global singleton ─────────────────────────────────────────────────────

use std::sync::OnceLock;

static HOOK_REGISTRY: OnceLock<HookRegistry> = OnceLock::new();

/// Get or initialise the global hook registry.
pub fn global_hook_registry() -> &'static HookRegistry {
    HOOK_REGISTRY.get_or_init(HookRegistry::new)
}

/// Initialise the global hook registry, optionally loading config files.
pub fn init_global_hook_registry(registry: HookRegistry) {
    HOOK_REGISTRY
        .set(registry)
        .unwrap_or_else(|_| panic!("init_global_hook_registry called more than once"));
}

// ── Convient re-exports ─────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_bash_pattern_parse() {
        let p = BashPattern::parse("Bash(git:commit:*)").unwrap();
        assert_eq!(p.base, "git");
        assert_eq!(p.subcommands, vec!["commit", "*"]);

        let p = BashPattern::parse("Bash(git:add:commit)").unwrap();
        assert_eq!(p.base, "git");
        assert_eq!(p.subcommands, vec!["add", "commit"]);
    }

    #[test]
    fn test_bash_pattern_match() {
        let p = BashPattern::parse("Bash(git:commit:*)").unwrap();
        assert!(p.matches("git commit -m \"hello\""));
        assert!(p.matches("git commit --amend"));
        assert!(!p.matches("git push"));
        assert!(!p.matches("rm -rf /"));

        let p = BashPattern::parse("Bash(git:add:commit)").unwrap();
        assert!(p.matches("git add ."));
        assert!(p.matches("git commit -m \"test\""));
        assert!(!p.matches("git push"));
    }

    #[test]
    fn test_permission_decision_aggregation() {
        let allow = HookMatchResult::allow();
        let deny = HookMatchResult::deny("blocked");
        let ask = HookMatchResult::ask();

        // All allow -> allow
        let r = HookRegistry::aggregate(&[allow.clone(), allow.clone()]);
        assert_eq!(r.decision, PermissionDecision::Allow);

        // One deny -> deny
        let r = HookRegistry::aggregate(&[allow.clone(), deny.clone()]);
        assert_eq!(r.decision, PermissionDecision::Deny);

        // One ask -> ask
        let r = HookRegistry::aggregate(&[allow.clone(), ask.clone()]);
        assert_eq!(r.decision, PermissionDecision::Ask);

        // Deny overrides ask
        let r = HookRegistry::aggregate(&[ask.clone(), deny.clone()]);
        assert_eq!(r.decision, PermissionDecision::Deny);
    }

    #[test]
    fn test_settings_check() {
        let deny = vec!["WebSearch".to_string(), "WebFetch".to_string()];
        let ask = vec!["Bash".to_string(), "shell".to_string()];

        assert_eq!(
            HookRegistry::check_settings("shell", &ask, &deny),
            PermissionDecision::Ask
        );
        assert_eq!(
            HookRegistry::check_settings("WebSearch", &ask, &deny),
            PermissionDecision::Deny
        );
        assert_eq!(
            HookRegistry::check_settings("ReadFile", &ask, &deny),
            PermissionDecision::Allow
        );
    }

    #[test]
    fn test_parse_matchers() {
        let m = parse_matchers("Read|Write|Edit");
        assert_eq!(m.len(), 1);
        assert!(matches!(&m[0], HookMatcher::Pattern(_)));

        let m = parse_matchers("Bash");
        assert_eq!(m.len(), 1);
        assert!(matches!(&m[0], HookMatcher::Exact(_)));

        let m = parse_matchers("Bash(git:commit:*)");
        assert_eq!(m.len(), 1);
        assert!(matches!(&m[0], HookMatcher::BashSubcommand(_)));
    }
}
