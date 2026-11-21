//! The runner's own view of an agent server, so callers don't read either backend's types.
//! A `Server` is one Herdr session's server, or this machine's herdr-core.

use std::path::Path;
use std::sync::Arc;

use super::core_watch::{self, Core};
use crate::hc::HcError;
use crate::herdr::types::{AgentInfo, AgentPromptParams, AgentStatus, ReadSource, SessionSnapshot};
use crate::herdr::{self, HerdrError, check_agent_target};

#[derive(Clone)]
pub enum Server {
    /// A Herdr server; `session` is its session name, none for the legacy single server.
    Herdr {
        session: Option<String>,
        client: herdr::Client,
    },
    /// herdr-core, behind the Herdr app.
    Core(Arc<Core>),
}

/// What one agent looks like, whichever server it runs in.
#[derive(Debug, Clone, PartialEq)]
pub struct Facts {
    /// The Herdr session, or `hc`, as `Server::name` gives it.
    pub server: Option<String>,
    /// The pane id, or the core terminal's address.
    pub target: String,
    pub name: Option<String>,
    pub kind: Option<String>,
    pub status: AgentStatus,
    pub cwd: Option<String>,
    pub title: Option<String>,
    pub space: Option<String>,
    pub tab: Option<String>,
    /// Herdr's `terminal_id`, or `core:address:pid:created_ms`: a new one is a new process.
    pub identity: Option<String>,
    /// The conversation id Herdr's integration reports.
    pub reported_session: Option<String>,
    /// Herdr's `state_change_seq`, or the core's `turn` and `status_ms` together.
    pub seq: u64,
    /// Ready for a prompt.
    pub ready: bool,
    /// The processes the agent may be, known up front on core; Herdr reads them on demand.
    pub pids: Vec<u32>,
    /// What the core says a blocked agent waits for: `permission`, `question` or `auth`.
    pub blocked_kind: Option<String>,
    /// The core's `turn`.
    pub turn: Option<u64>,
    /// The core says the program stopped on an error; its status reads as done.
    pub error: bool,
}

impl Facts {
    /// Its name on its own server: the agent name, else the pane id.
    pub fn local_name(&self) -> String {
        self.name.clone().unwrap_or_else(|| self.target.clone())
    }

    /// Whether `target` names this agent, by name or pane.
    pub fn is(&self, target: &str) -> bool {
        self.name.as_deref() == Some(target) || self.target == target
    }
}

pub enum Read {
    Visible,
    /// The last lines; a backend that can't read history while an agent sits at a dialog gives
    /// the visible screen instead.
    Recent(u32),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Text {
    pub text: String,
    pub revision: u64,
    pub truncated: bool,
}

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error(transparent)]
    Herdr(#[from] HerdrError),
    #[error(transparent)]
    Hc(#[from] HcError),
}

impl Error {
    pub fn outcome_unknown(&self) -> bool {
        match self {
            Error::Herdr(e) => e.outcome_unknown(),
            Error::Hc(e) => e.outcome_unknown(),
        }
    }

    pub fn server_gone(&self) -> bool {
        match self {
            Error::Herdr(e) => e.server_gone(),
            Error::Hc(e) => e.server_gone(),
        }
    }

    /// The agent sits at a dialog, so the server refused to type.
    pub fn is_blocked(&self) -> bool {
        match self {
            Error::Herdr(e) => e.server_code() == Some("agent_blocked"),
            Error::Hc(e) => e.kind() == Some("blocked"),
        }
    }

    pub fn is_not_found(&self) -> bool {
        match self {
            Error::Herdr(e) => e.server_code() == Some("agent_not_found"),
            Error::Hc(e) => e.kind() == Some("not_found"),
        }
    }

    pub fn is_invalid_argument(&self) -> bool {
        matches!(
            self,
            Error::Herdr(HerdrError::InvalidArgument(_)) | Error::Hc(HcError::InvalidArgument(_))
        )
    }
}

pub type Result<T> = std::result::Result<T, Error>;

/// A Herdr agent as the runner sees it; `spaces` maps workspace ids to their labels.
pub fn herdr_facts(
    session: Option<&str>,
    a: AgentInfo,
    spaces: &std::collections::HashMap<String, String>,
) -> Facts {
    Facts {
        server: session.map(str::to_owned),
        space: spaces.get(&a.workspace_id).cloned(),
        tab: None,
        ready: a.interactive_ready && !a.launch_pending,
        target: a.pane_id,
        name: a.name,
        kind: a.agent,
        status: a.agent_status,
        cwd: a.cwd,
        title: a.terminal_title_stripped.filter(|t| !t.is_empty()),
        identity: a.terminal_id,
        reported_session: a.agent_session.map(|s| s.value),
        seq: a.state_change_seq,
        pids: Vec::new(),
        blocked_kind: None,
        turn: None,
        error: false,
    }
}

/// Every agent in one Herdr snapshot.
pub fn snapshot_facts(session: Option<&str>, snap: SessionSnapshot) -> Vec<Facts> {
    let spaces = snap
        .workspaces
        .into_iter()
        .filter(|w| !w.label.is_empty())
        .map(|w| (w.workspace_id, w.label))
        .collect();
    snap.agents
        .into_iter()
        .map(|a| herdr_facts(session, a, &spaces))
        .collect()
}

impl Server {
    pub fn herdr(session: Option<String>, client: herdr::Client) -> Self {
        Server::Herdr { session, client }
    }

    /// The Herdr session name.
    pub fn name(&self) -> Option<&str> {
        match self {
            Server::Herdr { session, .. } => session.as_deref(),
            Server::Core(_) => Some(core_watch::SERVER),
        }
    }

    pub fn is_core(&self) -> bool {
        matches!(self, Server::Core(_))
    }

    /// The server's socket, which keys what is cached about its panes.
    pub fn socket(&self) -> &Path {
        match self {
            Server::Herdr { client, .. } => client.socket(),
            Server::Core(core) => core.socket(),
        }
    }

    /// The Herdr client, for what only Herdr does.
    pub fn as_herdr(&self) -> Option<&herdr::Client> {
        match self {
            Server::Herdr { client, .. } => Some(client),
            Server::Core(_) => None,
        }
    }

    pub async fn agents(&self) -> Result<Vec<Facts>> {
        match self {
            Server::Herdr { session, client } => Ok(snapshot_facts(
                session.as_deref(),
                client.session_snapshot().await?,
            )),
            Server::Core(core) => Ok(core.agents().await?),
        }
    }

    /// One agent, by name or pane; on core by address, then terminal or tab name.
    pub async fn agent(&self, target: &str) -> Result<Facts> {
        match self {
            Server::Herdr { session, client } => Ok(herdr_facts(
                session.as_deref(),
                client.agent_get(target).await?,
                &Default::default(),
            )),
            Server::Core(core) => core_agent(core, target).await,
        }
    }

    pub async fn read(&self, target: &str, what: Read) -> Result<Text> {
        match self {
            Server::Herdr { client, .. } => {
                check_agent_target(target)?;
                let r = match what {
                    Read::Visible => client.agent_read(target, ReadSource::Visible, None).await,
                    Read::Recent(lines) => match client
                        .agent_read(target, ReadSource::RecentUnwrapped, Some(lines))
                        .await
                    {
                        // Herdr refuses history reads while an agent sits at a dialog.
                        Err(e) if e.server_code() == Some("agent_not_idle") => {
                            client.agent_read(target, ReadSource::Visible, None).await
                        }
                        other => other,
                    },
                }?;
                Ok(Text {
                    text: r.text,
                    revision: r.revision,
                    truncated: r.truncated,
                })
            }
            Server::Core(core) => {
                let address = core_address(core, target).await?;
                let lines = match what {
                    Read::Visible => 0,
                    Read::Recent(lines) => lines,
                };
                let r = core.client().read(&address, lines).await?;
                Ok(Text {
                    text: r.text,
                    revision: 0,
                    truncated: !r.complete,
                })
            }
        }
    }

    /// The visible screen with its colours; plain text on core, none when the server can't give it.
    pub async fn read_ansi(&self, target: &str) -> Option<String> {
        match self {
            Server::Herdr { client, .. } => {
                check_agent_target(target).ok()?;
                client
                    .agent_read_screen_ansi(target)
                    .await
                    .ok()
                    .map(|r| r.text)
            }
            Server::Core(_) => self.read(target, Read::Visible).await.ok().map(|r| r.text),
        }
    }

    /// Types `text` as a prompt; refused while the agent sits at a dialog. Returns its status.
    pub async fn prompt(&self, target: &str, text: &str) -> Result<AgentStatus> {
        match self {
            Server::Herdr { client, .. } => {
                check_agent_target(target)?;
                let a = client
                    .agent_prompt(AgentPromptParams {
                        target: target.to_owned(),
                        text: text.to_owned(),
                        wait: None,
                    })
                    .await?;
                Ok(a.agent_status)
            }
            // Never forced: the core refuses a blocked program.
            Server::Core(core) => {
                let listed = core_agent(core, target).await?;
                let address = listed.target;
                // The mirror may lag: the same process must still be an agent there now.
                let now = core.client().get(&address).await?;
                if core
                    .fresh(&now)
                    .is_none_or(|f| f.identity != listed.identity)
                {
                    return Err(not_found(target));
                }
                let sent = core.client().send(&address, text).await?;
                Ok(core_watch::word(sent.status.as_deref()))
            }
        }
    }

    /// The process in the agent's pane, for the dialog path, read fresh; with what the server
    /// itself says a blocked agent waits for, when it says.
    pub async fn process(&self, target: &str) -> Result<(super::dialogs::Process, Option<String>)> {
        match self {
            Server::Herdr { client, .. } => {
                let a = client.agent_get(target).await?;
                let p = super::dialogs::Process {
                    kind: a.agent,
                    terminal_id: a.terminal_id,
                    agent_session: a.agent_session.map(|s| s.value),
                    state_change_seq: a.state_change_seq,
                    blocked: a.agent_status == AgentStatus::Blocked,
                };
                Ok((p, None))
            }
            Server::Core(core) => {
                let address = core_address(core, target).await?;
                let t = core.client().get(&address).await?;
                let f = core.fresh(&t).ok_or_else(|| not_found(target))?;
                // The identity and the process in front together: either changing is another process.
                let fg = t.foreground.as_ref().map_or(0, |f| f.pid);
                let p = super::dialogs::Process {
                    kind: f.kind,
                    terminal_id: f.identity.map(|id| format!("{id}:{fg}")),
                    agent_session: None,
                    state_change_seq: f.seq,
                    blocked: f.status == AgentStatus::Blocked,
                };
                Ok((p, f.blocked_kind))
            }
        }
    }

    /// A core agent's address, from its name or address; a Herdr target as given.
    pub async fn address(&self, target: &str) -> Result<String> {
        match self {
            Server::Herdr { .. } => Ok(target.to_owned()),
            Server::Core(core) => core_address(core, target).await,
        }
    }

    pub fn as_core(&self) -> Option<&Arc<Core>> {
        match self {
            Server::Core(core) => Some(core),
            Server::Herdr { .. } => None,
        }
    }

    /// The pids of the processes in front in the agent's pane.
    pub async fn pids(&self, facts: &Facts) -> Result<Vec<u32>> {
        match self {
            Server::Herdr { client, .. } => Ok(client.pane_foreground_pids(&facts.target).await?),
            Server::Core(_) => Ok(facts.pids.clone()),
        }
    }
}

/// A core agent by address, then by a name only it has. Only a listed agent is ever targeted,
/// never a shell.
async fn core_agent(core: &Core, target: &str) -> Result<Facts> {
    let all = core.agents().await?;
    if let Some(f) = all.iter().find(|f| f.target == target) {
        return Ok(f.clone());
    }
    let mut named = all
        .into_iter()
        .filter(|f| f.name.as_deref() == Some(target));
    match (named.next(), named.next()) {
        (Some(f), None) => Ok(f),
        _ => Err(not_found(target)),
    }
}

async fn core_address(core: &Core, target: &str) -> Result<String> {
    core_agent(core, target).await.map(|f| f.target)
}

fn not_found(target: &str) -> Error {
    Error::Hc(HcError::Server {
        method: "terminal.get",
        kind: "not_found".into(),
        message: format!("no agent {target:?} on herdr-core"),
    })
}
