use anyhow::{Context, bail};

use crate::cli::AdminCommand;
use crate::config::{HubConfig, RunnerConfig};

pub fn run(command: AdminCommand) -> anyhow::Result<()> {
    match command {
        AdminCommand::CheckConfig { hub, runner } => {
            if hub.is_none() && runner.is_none() {
                bail!("pass --hub and/or --runner");
            }
            if let Some(path) = hub {
                HubConfig::load(&path).with_context(|| format!("{}", path.display()))?;
                println!("{}: ok", path.display());
            }
            if let Some(path) = runner {
                RunnerConfig::load(&path).with_context(|| format!("{}", path.display()))?;
                println!("{}: ok", path.display());
            }
            Ok(())
        }
        other => bail!("admin {other:?} is not implemented yet"),
    }
}
