//! Runner: hub WebSocket client, Herdr operations and the hook endpoint.

pub mod agents;
pub mod codex;
pub mod herdrs;
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
use herdrs::{Herdrs, Starter};
use hooks::HookHub;
use sessions::{Sessions, SessionsConfig};

/// An executable's absolute path: from `PATH`, then where installers usually put it, since a
/// service manager's `PATH` is short.
pub fn find_bin(name: &str, home: &std::path::Path) -> Option<PathBuf> {
    use std::os::unix::fs::PermissionsExt;
    let path = std::env::var_os("PATH").unwrap_or_default();
    std::env::split_paths(&path)
        .chain([
            home.join(".local/bin"),
            home.join(".cargo/bin"),
            PathBuf::from("/opt/homebrew/bin"),
            PathBuf::from("/usr/local/bin"),
        ])
        .filter(|d| d.is_absolute())
        .map(|d| d.join(name))
        .find(|p| {
            std::fs::metadata(p).is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
        })
}

/// 128 random bits as lowercase hex, for approval ids and hook tokens.
pub fn random_id() -> String {
    let mut buf = [0u8; 16];
    getrandom::fill(&mut buf).expect("OS random source");
    hex::encode(buf)
}

/// A runner on another machine: dials the hub over WebSocket with its token.
pub async fn run(config: PathBuf) -> anyhow::Result<()> {
    let cfg = RunnerConfig::load(&config)?;
    let _ = rustls::crypto::ring::default_provider().install_default();
    let token = std::fs::read_to_string(&cfg.token_file)
        .with_context(|| format!("reading token file {}", cfg.token_file.display()))?
        .trim()
        .to_owned();
    let started = start(Settings {
        machine_id: cfg.machine_id.clone(),
        state_dir: cfg.state_dir.clone(),
        claude_config: cfg.claude_config.clone(),
        herdr_session: cfg.herdr_session.clone(),
    })
    .await?;
    let link = link::LinkConfig {
        url: cfg.hub_url.clone(),
        token,
        machine_id: cfg.machine_id.clone(),
        herdr_version: started.herdr_version,
    };
    link::run(link, started.sessions, started.events).await;
    Ok(())
}

/// A runner's own settings, whether it runs alone or inside the hub.
pub struct Settings {
    pub machine_id: String,
    pub state_dir: PathBuf,
    pub claude_config: Option<PathBuf>,
    pub herdr_session: Option<String>,
}

/// A runner that is up: its sessions, the events they raise for the hub, and the Herdr version.
pub struct Started {
    pub sessions: std::sync::Arc<Sessions>,
    pub events: mpsc::Receiver<Event>,
    pub herdr_version: Option<String>,
}

/// Finds Herdr, starts the hook endpoint, reloads tasks and starts watching them.
pub async fn start(cfg: Settings) -> anyhow::Result<Started> {
    let home = PathBuf::from(std::env::var_os("HOME").context("HOME is not set")?);

    // Herdr keeps its sessions under $XDG_CONFIG_HOME/herdr, normally ~/.config/herdr.
    let xdg = std::env::var_os("XDG_CONFIG_HOME")
        .filter(|v| !v.is_empty())
        .map(PathBuf::from);
    let config_dir = xdg
        .clone()
        .unwrap_or_else(|| home.join(".config"))
        .join("herdr");
    // Tasks from before each had its own session live in the server the CLI would pick.
    let legacy = find_socket(&config_dir, cfg.herdr_session.as_deref(), |k| {
        std::env::var(k).ok()
    });
    let herdr_bin = find_bin("herdr", &home).context(
        "herdr is not installed, or not on PATH, ~/.local/bin, ~/.cargo/bin or Homebrew's bin",
    )?;
    let herdrs = Herdrs::multi(
        config_dir.clone(),
        legacy,
        Starter::Herdr {
            bin: herdr_bin,
            xdg,
            log_dir: cfg.state_dir.join("herdr-logs"),
        },
    );
    let mut herdr_version = None;
    for (name, client) in herdrs.running().await {
        match client.check_protocol().await {
            Ok(v) => herdr_version = Some(v),
            Err(e) => tracing::warn!(herdr_session = ?name, error = %e, "unsupported Herdr server"),
        }
    }
    tracing::info!(
        machine = %cfg.machine_id,
        herdr = ?herdr_version,
        herdr_dir = %config_dir.display(),
        sessions = herdrs.known().len(),
        "runner starting"
    );

    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .context("binding hook endpoint")?;
    let hook_addr = listener.local_addr()?;
    let (events_tx, events_rx) = mpsc::channel(QUEUE_CAPACITY);
    let hooks = HookHub::new(events_tx.clone(), APPROVAL_DEADLINE);
    tokio::spawn(hooks.clone().serve(listener));

    let sessions = Sessions::with_herdrs(
        herdrs,
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
    let undone = sessions.undo_interrupted_starts().await;
    tracing::info!(reloaded, undone, %hook_addr, "hook endpoint listening");
    tokio::spawn(watch::watch(sessions.clone()));
    tokio::spawn(forward_all_named_agents(
        sessions.clone(),
        agents::RECONCILE,
    ));
    Ok(Started {
        sessions,
        events: events_rx,
        herdr_version,
    })
}

/// Named agents in every running Herdr session, as `<session>/<agent>` unless they are a task's.
/// Sessions are looked for again as often as agents are reconciled.
pub async fn forward_all_named_agents(
    sessions: std::sync::Arc<Sessions>,
    reconcile_every: Duration,
) {
    let mut followers: std::collections::HashMap<Option<String>, tokio::task::JoinHandle<()>> =
        std::collections::HashMap::new();
    loop {
        let running = sessions.herdrs.running().await;
        let live: std::collections::HashSet<Option<String>> =
            running.iter().map(|(n, _)| n.clone()).collect();
        followers.retain(|name, task| {
            let keep = live.contains(name) && !task.is_finished();
            if !keep {
                task.abort();
            }
            keep
        });
        for (name, client) in running {
            if followers.contains_key(&name) {
                continue;
            }
            let (sessions, server) = (sessions.clone(), name.clone());
            followers.insert(
                name,
                tokio::spawn(async move {
                    let (tx, mut rx) = mpsc::channel(256);
                    // Aborting this follower stops its watcher too.
                    let _watcher = AbortOnDrop(tokio::spawn(agents::watch_every(
                        client,
                        tx,
                        reconcile_every,
                    )));
                    let mut forward = StatusForwarder::default();
                    while let Some((agent, status)) = rx.recv().await {
                        let own = sessions.all().iter().any(|s| s.agent_name == agent);
                        let agent_name = if own || !sessions.herdrs.is_multi() {
                            agent
                        } else {
                            herdrs::qualified(server.as_deref(), &agent)
                        };
                        forward.forward(&sessions, agent_name, agents::status_str(status));
                    }
                }),
            );
        }
        tokio::time::sleep(reconcile_every).await;
    }
}

/// Every named agent's status goes to the hub, which hands it to the WhatsApp bridge.
pub async fn forward_named_agents(
    herdr: Client,
    sessions: std::sync::Arc<Sessions>,
    reconcile_every: Duration,
) {
    let (tx, mut rx) = mpsc::channel(256);
    let _watcher = AbortOnDrop(tokio::spawn(agents::watch_every(
        herdr,
        tx,
        reconcile_every,
    )));
    let mut forward = StatusForwarder::default();
    while let Some((agent_name, status)) = rx.recv().await {
        forward.forward(&sessions, agent_name, agents::status_str(status));
    }
}

struct AbortOnDrop(tokio::task::JoinHandle<()>);

impl Drop for AbortOnDrop {
    fn drop(&mut self) {
        self.0.abort();
    }
}

/// How often an unchanged status is sent again, so a restarted hub learns it.
const STATUS_REFRESH: Duration = Duration::from_secs(60);

/// Sends an outside agent's status when it changes, rather than on every 5-second snapshot.
#[derive(Default)]
struct StatusForwarder {
    last: std::collections::HashMap<String, (String, tokio::time::Instant)>,
}

impl StatusForwarder {
    fn forward(&mut self, sessions: &Sessions, agent_name: String, agent_status: String) {
        let now = tokio::time::Instant::now();
        if let Some((s, at)) = self.last.get(&agent_name)
            && *s == agent_status
            && now.duration_since(*at) < STATUS_REFRESH
        {
            return;
        }
        self.last
            .insert(agent_name.clone(), (agent_status.clone(), now));
        sessions.emit_status(Event::NamedAgentStatus {
            agent_name,
            agent_status,
        });
    }
}
