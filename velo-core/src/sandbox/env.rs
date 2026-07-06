use std::collections::HashMap;

const SENSITIVE_ENV_PREFIXES: &[&str] = &[
    "ANTHROPIC_",
    "OPENAI_",
    "AWS_SECRET",
    "AWS_ACCESS",
    "AZURE_",
    "GCP_",
    "GOOGLE_",
    "TOGETHER_",
    "NVIDIA_API_KEY",
    "NIM_API_KEY",
    "GEMINI_API_KEY",
    "STT_API_KEY",
    "HF_TOKEN",
    "HUGGING_FACE_",
    "GITHUB_TOKEN",
    "GITLAB_TOKEN",
    "DOCKER_",
    "KUBERNETES_",
    "VAULT_",
    "PGPASSWORD",
    "DB_PASSWORD",
    "DATABASE_URL",
    "REDIS_",
];

pub fn scrub_env(env: &HashMap<String, String>) -> HashMap<String, String> {
    let mut clean = HashMap::with_capacity(env.len());

    for (key, value) in env {
        if is_sensitive(key) {
            clean.insert(key.clone(), "[REDACTED]".to_string());
        } else {
            clean.insert(key.clone(), value.clone());
        }
    }

    clean
}

pub fn build_sandboxed_env(extra_env: Option<&HashMap<String, String>>) -> HashMap<String, String> {
    let mut env: HashMap<String, String> = std::env::vars().collect();
    if let Some(extra) = extra_env {
        for (k, v) in extra {
            env.insert(k.clone(), v.clone());
        }
    }
    scrub_env(&env)
}

pub fn is_sensitive(key: &str) -> bool {
    let upper = key.to_uppercase();
    SENSITIVE_ENV_PREFIXES
        .iter()
        .any(|prefix| upper.starts_with(prefix) || upper == *prefix)
}

pub fn redact_sensitive(value: &str) -> String {
    if value.len() <= 8 {
        "[REDACTED]".to_string()
    } else {
        let prefix: String = value.chars().take(4).collect();
        let suffix: String = value
            .chars()
            .rev()
            .take(4)
            .collect::<Vec<_>>()
            .into_iter()
            .rev()
            .collect();
        format!("{prefix}...{suffix}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_scrub_anthropic_key() {
        let mut env = HashMap::new();
        env.insert("ANTHROPIC_API_KEY".to_string(), "sk-ant-12345".to_string());
        env.insert("PATH".to_string(), "/usr/bin".to_string());
        let cleaned = scrub_env(&env);
        assert_eq!(cleaned.get("ANTHROPIC_API_KEY").unwrap(), "[REDACTED]");
        assert_eq!(cleaned.get("PATH").unwrap(), "/usr/bin");
    }

    #[test]
    fn test_scrub_nvidia_key() {
        let mut env = HashMap::new();
        env.insert("NVIDIA_API_KEY".to_string(), "nvapi-secret".to_string());
        let cleaned = scrub_env(&env);
        assert_eq!(cleaned.get("NVIDIA_API_KEY").unwrap(), "[REDACTED]");
    }

    #[test]
    fn test_scrub_case_insensitive() {
        let mut env = HashMap::new();
        env.insert("aws_secret_access_key".to_string(), "secret".to_string());
        let cleaned = scrub_env(&env);
        assert_eq!(cleaned.get("aws_secret_access_key").unwrap(), "[REDACTED]");
    }

    #[test]
    fn test_build_sandboxed_env() {
        let result = build_sandboxed_env(None);
        // If present, sensitive vars are redacted, not removed
        if let Some(v) = result.get("NVIDIA_API_KEY") {
            assert_eq!(v, "[REDACTED]");
        }
        assert!(result.contains_key("PATH"));
    }
}
