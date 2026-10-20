//! Runner: hub WebSocket client, Herdr operations and the hook endpoint.

pub mod agents;
pub mod codex;
pub mod hooks;
pub mod link;
pub mod sessions;
pub mod watch;

use std::path::PathBuf;
use std::time::Duration;

use anyhow::Context;
use tokio::net::TcpListener;
use tokio::sync::mpsc;

use crate::agents::APPROVAL_DEADLINE;
use crate::config::RunnerConfig;
use crate::herdr::{Client, find_socket};
use crate::proto::Event;
use crate::proto::conn::QUEUE_CAPACITY;
use hooks::HookHub;
use sessions::{Sessions, SessionsConfig};

/// 128 random bits as lowercase hex, for approval ids and hook tokens.
pub fn random_id() -> String {
    let mut buf = [0u8; 16];
    getrandom::fill(&mut buf).expect("OS random source");
    hex::encode(buf)
}

pub async fn run(config: PathBuf) -> anyhow::Result<()> {
    let cfg = RunnerConfig::load(&config)?;
    let _ = rustls::crypto::ring::default_provider().install_default();
    let token = std::fs::read_to_string(&cfg.token_file)
        .with_context(|| format!("reading token file {}", cfg.token_file.display()))?
        .trim()
        .to_owned();
    let home = PathBuf::from(std::env::var_os("HOME").context("HOME is not set")?);

    let socket = find_socket(
        &home.join(".config/herdr"),
        cfg.herdr_session.as_deref(),
        |k| std::env::var(k).ok(),
    );
    let herdr = Client::new(&socket);
    let herdr_version = herdr
        .check_protocol()
        .await
        .with_context(|| format!("Herdr server at {}", socket.display()))?;
    tracing::info!(machine = %cfg.machine_id, herdr = %herdr_version, socket = %socket.display(), "runner starting");

    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .context("binding hook endpoint")?;
    let hook_addr = listener.local_addr()?;
    let (events_tx, events_rx) = mpsc::channel(QUEUE_CAPACITY);
    let hooks = HookHub::new(events_tx.clone(), APPROVAL_DEADLINE);
    tokio::spawn(hooks.clone().serve(listener));

    let sessions = Sessions::new(
        herdr,
        hooks,
        events_tx,
        SessionsConfig {
            state_dir: cfg.state_dir.clone(),
            claude_config: cfg
                .claude_config
                .clone()
                .unwrap_or_else(|| home.join(".claude.json")),
            exe: std::env::current_exe().context("locating the switchboard binary")?,
            hook_addr,
            codex_version_cmd: vec!["codex".into(), "--version".into()],
            liveness_timeout: Duration::from_secs(15),
            startup_timeout: Duration::from_secs(20),
        },
    );
    let reloaded = sessions.load().context("reloading sessions")?;
    tracing::info!(reloaded, %hook_addr, "hook endpoint listening");
    tokio::spawn(watch::watch(sessions.clone()));
    tokio::spawn(forward_named_agents(
        Client::new(&socket),
        sessions.clone(),
        agents::RECONCILE,
    ));

    let link = link::LinkConfig {
        url: cfg.hub_url.clone(),
        token,
        machine_id: cfg.machine_id.clone(),
        herdr_version: Some(herdr_version),
    };
    link::run(link, sessions, events_rx).await;
    Ok(())
}

/// Every named agent's status goes to the hub, which hands it to the WhatsApp bridge.
pub async fn forward_named_agents(
    herdr: Client,
    sessions: std::sync::Arc<Sessions>,
    reconcile_every: Duration,
) {
    let (tx, mut rx) = mpsc::channel(256);
    tokio::spawn(agents::watch_every(herdr, tx, reconcile_every));
    while let Some((agent_name, status)) = rx.recv().await {
        sessions.emit(Event::NamedAgentStatus {
            agent_name,
            agent_status: agents::status_str(status),
        });
    }
}
