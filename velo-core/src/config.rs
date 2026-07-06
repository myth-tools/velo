use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;

use crate::error::VeloError;
use crate::sandbox::fs::FsSandboxConfig;

// ── Agent Identity ────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentIdentity {
    pub name: String,
    pub developer: String,
    pub version: String,
    pub description: String,
    pub homepage: Option<String>,
    pub email: Option<String>,
    pub repository: Option<String>,
}

impl AgentIdentity {
    pub fn summary(&self) -> String {
        let mut s = format!(
            "{} v{} by {} — {}",
            self.name, self.version, self.developer, self.description
        );
        if let Some(homepage) = &self.homepage {
            use std::fmt::Write;
            let _ = write!(s, " | {}", homepage);
        }
        s
    }
}

// ── Public config (flat, backward-compatible) ─────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VeloConfig {
    pub identity: AgentIdentity,
    pub nvidia_api_key: String,
    pub nim_base_url: String,
    pub nim_model: String,
    pub nim_embedding_model: String,
    pub nim_embedding_dimension: u32,
    pub vision_model: String,
    pub max_tokens: u32,
    pub temperature: f32,
    pub stt_base_url: String,
    pub stt_model: String,
    pub stt_api_key: String,
    pub clipboard_poll_ms: u64,
    pub shell_timeout_secs: u64,
    pub snapshot_dir: PathBuf,
    /// Tools that always require user approval before execution.
    #[serde(default)]
    pub permission_ask: Vec<String>,
    /// Tools that are completely blocked.
    #[serde(default)]
    pub permission_deny: Vec<String>,
    /// Path to hooks configuration file.
    #[serde(default)]
    pub hooks_file: Option<PathBuf>,
    /// Sandbox configuration (filesystem access controls, etc.).
    #[serde(default)]
    pub sandbox: SandboxConfig,
}

impl VeloConfig {
    pub fn load() -> Result<Self, VeloError> {
        let yaml_path = Self::yaml_path();
        if yaml_path.exists() {
            let data = std::fs::read_to_string(&yaml_path)
                .map_err(|e| VeloError::Yaml(format!("Cannot read {yaml_path:?}: {e}")))?;
            let config_file: ConfigFile =
                serde_yaml::from_str(&data).map_err(|e| VeloError::Yaml(e.to_string()))?;
            return Self::from_config_file(config_file);
        }

        Self::from_env()
    }

    fn yaml_path() -> PathBuf {
        std::env::var("VELO_CONFIG_PATH")
            .map(PathBuf::from)
            .unwrap_or_else(|_| {
                let home = std::env::var("HOME").unwrap_or_else(|_| "/tmp".into());
                PathBuf::from(home).join(".velo").join("config.yaml")
            })
    }

    fn from_config_file(cf: ConfigFile) -> Result<Self, VeloError> {
        let react = cf.llms.get("react").ok_or_else(|| {
            VeloError::MissingConfig("yaml: llms.react section is required".into())
        })?;
        let vision = cf.llms.get("vision").ok_or_else(|| {
            VeloError::MissingConfig("yaml: llms.vision section is required".into())
        })?;
        let stt = cf
            .llms
            .get("stt")
            .ok_or_else(|| VeloError::MissingConfig("yaml: llms.stt section is required".into()))?;
        let embedding = cf.llms.get("embedding").ok_or_else(|| {
            VeloError::MissingConfig("yaml: llms.embedding section is required".into())
        })?;
        let identity = cf.identity.unwrap_or_else(|| AgentIdentity {
            name: std::env::var("VELO_NAME").unwrap_or_else(|_| "Velo".into()),
            developer: std::env::var("VELO_DEVELOPER")
                .unwrap_or_else(|_| "Velo Contributors".into()),
            version: std::env::var("VELO_VERSION").unwrap_or_else(|_| "0.1.0".into()),
            description: std::env::var("VELO_DESCRIPTION")
                .unwrap_or_else(|_| "An autonomous desktop agent".into()),
            homepage: std::env::var("VELO_HOMEPAGE").ok(),
            email: std::env::var("VELO_EMAIL").ok(),
            repository: std::env::var("VELO_REPOSITORY").ok(),
        });
        let settings = cf.settings.unwrap_or_default();

        let nvidia_api_key = react
            .api_key
            .clone()
            .or_else(|| std::env::var("NVIDIA_API_KEY").ok())
            .ok_or_else(|| {
                VeloError::MissingConfig("NVIDIA_API_KEY env var or llms.react.api_key".into())
            })?;

        let stt_api_key = stt
            .api_key
            .clone()
            .or_else(|| std::env::var("GEMINI_API_KEY").ok())
            .or_else(|| std::env::var("STT_API_KEY").ok())
            .unwrap_or_else(|| nvidia_api_key.clone());

        let permission_ask = if settings.permission_ask.is_empty() {
            std::env::var("VELO_PERMISSION_ASK")
                .ok()
                .map(|s| s.split(',').map(|p| p.trim().to_string()).collect())
                .unwrap_or_default()
        } else {
            settings.permission_ask.clone()
        };

        let permission_deny = if settings.permission_deny.is_empty() {
            std::env::var("VELO_PERMISSION_DENY")
                .ok()
                .map(|s| s.split(',').map(|p| p.trim().to_string()).collect())
                .unwrap_or_default()
        } else {
            settings.permission_deny.clone()
        };

        let hooks_file = settings
            .hooks_file
            .or_else(|| std::env::var("VELO_HOOKS_FILE").ok().map(PathBuf::from));

        Ok(Self {
            identity,
            nvidia_api_key,
            nim_base_url: react
                .base_url
                .clone()
                .or_else(|| std::env::var("NIM_BASE_URL").ok())
                .unwrap_or_else(|| "https://integrate.api.nvidia.com/v1".into()),
            nim_model: react.model.clone().ok_or_else(|| {
                VeloError::MissingConfig("yaml: llms.react.model is required".into())
            })?,
            nim_embedding_model: embedding.model.clone().ok_or_else(|| {
                VeloError::MissingConfig("yaml: llms.embedding.model is required".into())
            })?,
            nim_embedding_dimension: embedding.dimension.ok_or_else(|| {
                VeloError::MissingConfig("yaml: llms.embedding.dimension is required".into())
            })?,
            vision_model: vision.model.clone().ok_or_else(|| {
                VeloError::MissingConfig("yaml: llms.vision.model is required".into())
            })?,
            max_tokens: react
                .max_tokens
                .or_else(|| {
                    std::env::var("NIM_MAX_TOKENS")
                        .ok()
                        .and_then(|v| v.parse().ok())
                })
                .ok_or_else(|| {
                    VeloError::MissingConfig("yaml: llms.react.max_tokens is required".into())
                })?,
            temperature: react
                .temperature
                .or_else(|| {
                    std::env::var("NIM_TEMPERATURE")
                        .ok()
                        .and_then(|v| v.parse().ok())
                })
                .ok_or_else(|| {
                    VeloError::MissingConfig("yaml: llms.react.temperature is required".into())
                })?,
            stt_base_url: stt
                .base_url
                .clone()
                .or_else(|| std::env::var("STT_BASE_URL").ok())
                .unwrap_or_else(|| "https://generativelanguage.googleapis.com/v1beta".into()),
            stt_model: stt.model.clone().ok_or_else(|| {
                VeloError::MissingConfig("yaml: llms.stt.model is required".into())
            })?,
            stt_api_key,
            clipboard_poll_ms: settings.clipboard_poll_ms.unwrap_or(200),
            shell_timeout_secs: settings.shell_timeout_secs.unwrap_or(60),
            snapshot_dir: settings
                .snapshot_dir
                .or_else(|| std::env::var("VELO_SNAPSHOT_DIR").ok().map(PathBuf::from))
                .unwrap_or_else(|| {
                    let home = std::env::var("HOME").unwrap_or_else(|_| "/tmp".into());
                    PathBuf::from(home).join(".velo").join("snapshots")
                }),
            permission_ask,
            permission_deny,
            hooks_file,
            sandbox: settings.sandbox.unwrap_or_default(),
        })
    }

    pub fn from_env() -> Result<Self, VeloError> {
        let api_key = std::env::var("NVIDIA_API_KEY")
            .map_err(|_| VeloError::MissingConfig("NVIDIA_API_KEY".into()))?;

        if api_key.trim().is_empty() {
            return Err(VeloError::MissingConfig(
                "NVIDIA_API_KEY is set but empty".into(),
            ));
        }

        Ok(Self {
            identity: AgentIdentity {
                name: std::env::var("VELO_NAME").unwrap_or_else(|_| "Velo".into()),
                developer: std::env::var("VELO_DEVELOPER")
                    .unwrap_or_else(|_| "Velo Contributors".into()),
                version: std::env::var("VELO_VERSION").unwrap_or_else(|_| "0.1.0".into()),
                description: std::env::var("VELO_DESCRIPTION")
                    .unwrap_or_else(|_| "An autonomous desktop agent".into()),
                homepage: std::env::var("VELO_HOMEPAGE").ok(),
                email: std::env::var("VELO_EMAIL").ok(),
                repository: std::env::var("VELO_REPOSITORY").ok(),
            },
            nvidia_api_key: api_key.clone(),
            nim_base_url: std::env::var("NIM_BASE_URL")
                .unwrap_or_else(|_| "https://integrate.api.nvidia.com/v1".into()),
            nim_model: std::env::var("NIM_MODEL").map_err(|_| {
                VeloError::MissingConfig("NIM_MODEL env var is required when no yaml".into())
            })?,
            nim_embedding_model: std::env::var("NIM_EMBEDDING_MODEL").map_err(|_| {
                VeloError::MissingConfig(
                    "NIM_EMBEDDING_MODEL env var is required when no yaml".into(),
                )
            })?,
            nim_embedding_dimension: std::env::var("NIM_EMBEDDING_DIMENSION")
                .ok()
                .and_then(|v| v.parse().ok())
                .ok_or_else(|| {
                    VeloError::MissingConfig(
                        "NIM_EMBEDDING_DIMENSION env var is required when no yaml".into(),
                    )
                })?,
            vision_model: std::env::var("NIM_VISION_MODEL").map_err(|_| {
                VeloError::MissingConfig("NIM_VISION_MODEL env var is required when no yaml".into())
            })?,
            max_tokens: std::env::var("NIM_MAX_TOKENS")
                .ok()
                .and_then(|v| v.parse().ok())
                .ok_or_else(|| {
                    VeloError::MissingConfig(
                        "NIM_MAX_TOKENS env var is required when no yaml".into(),
                    )
                })?,
            temperature: std::env::var("NIM_TEMPERATURE")
                .ok()
                .and_then(|v| v.parse().ok())
                .ok_or_else(|| {
                    VeloError::MissingConfig(
                        "NIM_TEMPERATURE env var is required when no yaml".into(),
                    )
                })?,
            stt_base_url: std::env::var("STT_BASE_URL")
                .unwrap_or_else(|_| "https://generativelanguage.googleapis.com/v1beta".into()),
            stt_model: std::env::var("STT_MODEL").map_err(|_| {
                VeloError::MissingConfig("STT_MODEL env var is required when no yaml".into())
            })?,
            stt_api_key: std::env::var("GEMINI_API_KEY")
                .ok()
                .or_else(|| std::env::var("STT_API_KEY").ok())
                .unwrap_or(api_key),
            clipboard_poll_ms: 200,
            shell_timeout_secs: 60,
            snapshot_dir: {
                let home = std::env::var("HOME").unwrap_or_else(|_| "/tmp".into());
                PathBuf::from(home).join(".velo").join("snapshots")
            },
            permission_ask: std::env::var("VELO_PERMISSION_ASK")
                .ok()
                .map(|s| s.split(',').map(|p| p.trim().to_string()).collect())
                .unwrap_or_default(),
            permission_deny: std::env::var("VELO_PERMISSION_DENY")
                .ok()
                .map(|s| s.split(',').map(|p| p.trim().to_string()).collect())
                .unwrap_or_default(),
            hooks_file: std::env::var("VELO_HOOKS_FILE").ok().map(PathBuf::from),
            sandbox: SandboxConfig::default(),
        })
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SandboxConfig {
    /// Filesystem sandbox configuration.
    #[serde(default)]
    pub filesystem: FsSandboxConfig,
}

// ── YAML config file types ────────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
struct ConfigFile {
    llms: HashMap<String, LlmItem>,
    #[serde(default)]
    identity: Option<AgentIdentity>,
    #[serde(default)]
    settings: Option<SettingsDef>,
}

#[derive(Debug, Deserialize)]
struct LlmItem {
    model: Option<String>,
    #[serde(default)]
    base_url: Option<String>,
    #[serde(default)]
    api_key: Option<String>,
    #[serde(default)]
    max_tokens: Option<u32>,
    #[serde(default)]
    temperature: Option<f32>,
    #[serde(default)]
    dimension: Option<u32>,
}

#[derive(Debug, Default, Deserialize)]
struct SettingsDef {
    #[serde(default)]
    clipboard_poll_ms: Option<u64>,
    #[serde(default)]
    shell_timeout_secs: Option<u64>,
    #[serde(default)]
    snapshot_dir: Option<PathBuf>,
    /// Tools that always require user approval.
    #[serde(default)]
    permission_ask: Vec<String>,
    /// Tools that are completely blocked.
    #[serde(default)]
    permission_deny: Vec<String>,
    /// Path to hooks configuration file.
    #[serde(default)]
    hooks_file: Option<PathBuf>,
    /// Sandbox configuration.
    #[serde(default)]
    sandbox: Option<SandboxConfig>,
}
