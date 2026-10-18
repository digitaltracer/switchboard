use std::path::PathBuf;
use std::process::ExitCode;

use clap::{Parser, Subcommand, ValueEnum};

use crate::{admin, hook, hub, runner};

#[derive(Debug, Parser)]
#[command(
    name = "switchboard",
    version,
    about = "Start and watch Herdr coding sessions from bots"
)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Command,
}

#[derive(Debug, Subcommand)]
pub enum Command {
    /// Run the hub (MCP server, runner endpoint, OAuth).
    Hub {
        #[arg(long, env = "SWITCHBOARD_HUB_CONFIG", default_value = "hub.toml")]
        config: PathBuf,
    },
    /// Run a runner that dials out to the hub and drives the local Herdr session.
    Runner {
        #[arg(long, env = "SWITCHBOARD_RUNNER_CONFIG", default_value = "runner.toml")]
        config: PathBuf,
    },
    /// Permission-hook helper invoked by an agent CLI. Always answers; denies on any error.
    Hook {
        #[arg(long, value_enum)]
        agent: Agent,
        /// File holding the per-session hook secret, outside the worktree.
        #[arg(long)]
        secret_file: Option<PathBuf>,
        /// Append each hook invocation as a JSON line to this file.
        #[arg(long)]
        log: Option<PathBuf>,
    },
    /// Administrative commands.
    Admin {
        #[command(subcommand)]
        command: AdminCommand,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Agent {
    Claude,
    Codex,
}

impl Agent {
    pub fn as_str(self) -> &'static str {
        match self {
            Agent::Claude => "claude",
            Agent::Codex => "codex",
        }
    }
}

#[derive(Debug, Subcommand)]
pub enum AdminCommand {
    /// Validate hub.toml and/or runner.toml and exit.
    CheckConfig {
        #[arg(long)]
        hub: Option<PathBuf>,
        #[arg(long)]
        runner: Option<PathBuf>,
    },
    /// Hash a consent passphrase read from stdin.
    HashPassphrase,
    /// Mint a runner token and print it with its hash for hub.toml.
    MintRunnerToken {
        #[arg(long)]
        machine: String,
    },
    /// Open the OAuth client registration window for ten minutes.
    OpenRegistration {
        #[arg(long, env = "SWITCHBOARD_HUB_CONFIG", default_value = "hub.toml")]
        config: PathBuf,
    },
    /// Write systemd units for the hub and VPS runner.
    InstallSystemd,
    /// Write a launchd plist for the Mac runner.
    InstallLaunchd,
    /// Download, verify and install a signed release.
    Upgrade,
}

pub fn run() -> ExitCode {
    let cli = Cli::parse();
    match cli.command {
        // The hook must never log to stdout or fail with a non-protocol answer.
        Command::Hook {
            agent,
            secret_file,
            log,
        } => hook::run(agent, secret_file, log),
        command => {
            init_tracing();
            let result = match command {
                Command::Hub { config } => block_on(hub::run(config)),
                Command::Runner { config } => block_on(runner::run(config)),
                Command::Admin { command } => admin::run(command),
                Command::Hook { .. } => unreachable!(),
            };
            match result {
                Ok(()) => ExitCode::SUCCESS,
                Err(err) => {
                    eprintln!("error: {err:#}");
                    ExitCode::FAILURE
                }
            }
        }
    }
}

fn init_tracing() {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()),
        )
        .with_writer(std::io::stderr)
        .init();
}

fn block_on<F: std::future::Future<Output = anyhow::Result<()>>>(fut: F) -> anyhow::Result<()> {
    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?
        .block_on(fut)
}
