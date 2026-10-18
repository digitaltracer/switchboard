//! Hub: runner WebSocket endpoint, session service, MCP and auth. Implementation steps 6 and 7.

pub mod runners;

use std::path::PathBuf;

use anyhow::bail;

use crate::config::HubConfig;

pub async fn run(config: PathBuf) -> anyhow::Result<()> {
    let cfg = HubConfig::load(&config)?;
    tracing::info!(
        projects = cfg.projects.len(),
        machines = cfg.machines.len(),
        "hub config loaded"
    );
    bail!("hub is not implemented yet")
}
