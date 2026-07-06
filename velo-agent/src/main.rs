//! Velo Agent — binary entry point.
//!
//! Thin wrapper that calls into `velo_agent::run()`.

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    velo_agent::run().await
}
