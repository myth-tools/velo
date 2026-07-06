use std::sync::OnceLock;

use tokio::sync::Mutex;

use crate::config::VeloConfig;

static GLOBAL_CONFIG: OnceLock<VeloConfig> = OnceLock::new();
static HTTP_CLIENT: OnceLock<reqwest::Client> = OnceLock::new();
static SNAPSHOT_MGR: OnceLock<Mutex<Option<crate::snapshot::manager::SnapshotManager>>> =
    OnceLock::new();

pub fn init_global_config(config: VeloConfig) {
    GLOBAL_CONFIG
        .set(config)
        .unwrap_or_else(|_| panic!("init_global_config called more than once"));
}

pub(crate) fn config() -> &'static VeloConfig {
    GLOBAL_CONFIG.get().expect("VeloConfig not initialised")
}

pub(crate) fn http_client() -> &'static reqwest::Client {
    HTTP_CLIENT.get_or_init(|| {
        reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(60))
            .connect_timeout(std::time::Duration::from_secs(15))
            .pool_idle_timeout(std::time::Duration::from_secs(120))
            .pool_max_idle_per_host(32)
            .user_agent("Velo/1.0")
            .build()
            .expect("Failed to build reqwest Client")
    })
}

pub(crate) async fn snapshot_manager(
) -> &'static Mutex<Option<crate::snapshot::manager::SnapshotManager>> {
    SNAPSHOT_MGR.get_or_init(|| {
        Mutex::new(Some(crate::snapshot::manager::SnapshotManager::new(
            config().snapshot_dir.clone(),
        )))
    });
    SNAPSHOT_MGR.get().unwrap()
}

pub(crate) fn exec_err(msg: impl Into<String>) -> autoagents::core::tool::ToolCallError {
    autoagents::core::tool::ToolCallError::RuntimeError(anyhow::anyhow!("{}", msg.into()).into())
}
