pub mod agent;
pub mod audio;
pub mod builder;
pub mod config;
pub mod error;
pub mod events;
pub mod handle;
pub mod hooks;
pub mod mcp;
pub mod memory;
pub mod sandbox;
pub mod skills;
pub mod snapshot;
pub mod sub_agent;
pub mod tools;

pub use config::VeloConfig;
pub use error::VeloError;

pub fn init_tracing() {
    use tracing_subscriber::{fmt, prelude::*, EnvFilter};

    let filter =
        EnvFilter::try_from_env("VELO_LOG").unwrap_or_else(|_| EnvFilter::new("velo=info,warn"));

    tracing_subscriber::registry()
        .with(fmt::layer().with_target(true).compact())
        .with(filter)
        .init();
}
