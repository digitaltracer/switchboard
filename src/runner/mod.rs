//! Runner: hub WebSocket client, Herdr operations and the hook endpoint. Implementation step 5.

use std::path::PathBuf;

use anyhow::bail;

use crate::config::RunnerConfig;

pub async fn run(config: PathBuf) -> anyhow::Result<()> {
    let cfg = RunnerConfig::load(&config)?;
    tracing::info!(machine = %cfg.machine_id, "runner config loaded");
    bail!("runner is not implemented yet")
}
