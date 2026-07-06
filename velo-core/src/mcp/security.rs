use std::collections::HashMap;

use super::types::McpServerConfig;

const SENSITIVE_ENV_PREFIXES: &[&str] = &[
    "ANTHROPIC_",
    "OPENAI_",
    "AWS_SECRET",
    "AWS_ACCESS",
    "AZURE_",
    "GCP_",
    "GOOGLE_",
    "TOGETHER_",
];

pub fn substitute_config_env_vars(config: &mut McpServerConfig) {
    let mut env = std::env::vars().collect::<HashMap<_, _>>();

    if let Ok(plugin_root) = std::env::var("VELO_PLUGIN_ROOT") {
        env.entry("VELO_PLUGIN_ROOT".to_string())
            .or_insert_with(|| plugin_root.clone());
        if let Some(ref cmd) = config.command {
            if cmd.contains("${VELO_PLUGIN_ROOT}") {
                env.insert("VELO_PLUGIN_ROOT".to_string(), plugin_root);
            }
        }
    }

    if let Some(ref mut cmd) = config.command {
        *cmd = substitute_vars(cmd, &env);
    }

    for arg in &mut config.args {
        *arg = substitute_vars(arg, &env);
    }

    for value in config.env.values_mut() {
        *value = substitute_vars(value, &env);
    }

    for value in config.headers.values_mut() {
        *value = substitute_vars(value, &env);
    }

    if let Some(ref mut url) = config.url {
        *url = substitute_vars(url, &env);
    }

    if let Some(ref mut helper) = config.headers_helper {
        *helper = substitute_vars(helper, &env);
    }

    substitute_inherited_env_vars(&mut config.env, &env);
}

fn substitute_vars(input: &str, env: &HashMap<String, String>) -> String {
    let mut result = String::with_capacity(input.len());
    let s = input;
    let mut i = 0;

    while i < s.len() {
        if s[i..].starts_with("${") {
            i += 2;
            let mut var_name = String::new();
            let mut default_value: Option<String> = None;

            let remaining = &s[i..];
            if let Some(end) = remaining.find(['}', ':']) {
                let prefix = &remaining[..end];
                if remaining.as_bytes().get(end) == Some(&b':') {
                    // Check for :- default
                    if remaining.as_bytes().get(end + 1) == Some(&b'-') {
                        let after_default = &remaining[end + 2..];
                        if let Some(delim) = after_default.find('}') {
                            var_name = prefix.to_string();
                            default_value = Some(after_default[..delim].to_string());
                            i += end + 2 + delim + 1;
                        } else {
                            var_name.push_str(prefix);
                            var_name.push(':');
                            i += end + 1;
                        }
                    } else {
                        var_name.push_str(prefix);
                        var_name.push(':');
                        i += end + 1;
                    }
                } else {
                    var_name = prefix.to_string();
                    i += end + 1;
                }
            } else {
                result.push_str("${");
                result.push_str(&var_name);
                i = s.len();
                continue;
            }

            if let Some(default) = default_value {
                result.push_str(env.get(&var_name).unwrap_or(&default));
            } else {
                result.push_str(env.get(&var_name).map(|s| s.as_str()).unwrap_or(""));
            }
        } else {
            result.push(s[i..].chars().next().unwrap());
            i += s[i..].chars().next().unwrap().len_utf8();
        }
    }

    result
}

fn substitute_inherited_env_vars(
    env: &mut HashMap<String, String>,
    system_env: &HashMap<String, String>,
) {
    let inherited_keys: Vec<String> = env
        .iter()
        .filter(|(_, v)| v.is_empty())
        .map(|(k, _)| k.clone())
        .collect();

    for key in inherited_keys {
        if let Some(val) = system_env.get(&key) {
            env.insert(key, val.clone());
        }
    }
}

pub fn scrub_subprocess_env(env: &HashMap<String, String>) -> HashMap<String, String> {
    let mut clean = HashMap::new();

    for (key, value) in env {
        let should_scrub = SENSITIVE_ENV_PREFIXES
            .iter()
            .any(|prefix| key.to_uppercase().starts_with(prefix));

        if should_scrub {
            clean.insert(key.clone(), "[REDACTED]".to_string());
        } else {
            clean.insert(key.clone(), value.clone());
        }
    }

    clean
}

pub fn validate_mcp_url(url: &str) -> Result<(), McpSecurityError> {
    let lower = url.to_lowercase();

    if !lower.starts_with("https://") && !lower.starts_with("wss://") {
        return Err(McpSecurityError::InsecureUrl(url.to_string()));
    }

    Ok(())
}

#[derive(Debug)]
pub enum McpSecurityError {
    InsecureUrl(String),
}

impl std::fmt::Display for McpSecurityError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InsecureUrl(u) => {
                write!(
                    f,
                    "Insecure MCP URL '{u}': HTTPS/WSS required for remote servers"
                )
            }
        }
    }
}

impl std::error::Error for McpSecurityError {}

pub fn resolve_env_var(name: &str) -> Option<String> {
    let name = name.strip_prefix("${")?.strip_suffix('}')?;
    let (var, _default) = name.split_once(":-").unwrap_or((name, ""));
    std::env::var(var).ok()
}
