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
    /// Run the hub (MCP server, runner endpoint, OAuth, and the WhatsApp bridge if configured).
    Hub {
        #[arg(long, env = "SWITCHBOARD_HUB_CONFIG", default_value = "hub.toml")]
        config: PathBuf,
        /// Link WhatsApp with a pair code for this number instead of a QR code.
        #[arg(long)]
        whatsapp_pair_phone: Option<String>,
    },
    /// Run the relay that phones and hubs dial out to; it forwards and sends pushes.
    Relay {
        #[arg(long, env = "SWITCHBOARD_RELAY_CONFIG", default_value = "relay.toml")]
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
    /// Mint a runner token into a new private file and print its hash for hub.toml.
    MintRunnerToken {
        #[arg(long)]
        machine: String,
        /// Where the runner reads it (runner.toml's token_file); never overwritten.
        #[arg(long)]
        out: PathBuf,
    },
    /// Print `[[projects]]` for every git repo under the given folders, for hub.toml.
    ScanProjects {
        /// The machine whose paths these are.
        #[arg(long)]
        machine: String,
        #[arg(required = true)]
        roots: Vec<PathBuf>,
    },
    /// Pair a phone: shows a single-use QR code, then asks you to confirm the phone's fingerprint.
    PairDevice {
        #[arg(long, env = "SWITCHBOARD_HUB_CONFIG", default_value = "hub.toml")]
        config: PathBuf,
    },
    /// List paired devices.
    ListDevices {
        #[arg(long, env = "SWITCHBOARD_HUB_CONFIG", default_value = "hub.toml")]
        config: PathBuf,
    },
    /// Revoke a paired device; its next request is refused.
    RevokeDevice {
        #[arg(long, env = "SWITCHBOARD_HUB_CONFIG", default_value = "hub.toml")]
        config: PathBuf,
        /// The device id from `list-devices`.
        device: String,
    },
    /// Print the app API's OpenAPI spec, for `api/openapi.json`.
    Openapi,
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
                Command::Hub {
                    config,
                    whatsapp_pair_phone,
                } => block_on(hub::run(config, whatsapp_pair_phone)),
                Command::Runner { config } => block_on(runner::run(config)),
                Command::Relay { config } => block_on(async move {
                    let cfg = crate::config::RelayConfig::load(&config)?;
                    crate::relay::server::run(cfg).await
                }),
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
