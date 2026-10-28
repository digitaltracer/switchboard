//! Herdr socket client. Newline-delimited JSON over a Unix socket; Herdr answers one
//! request per connection, and a subscription keeps its connection open.
//! This module imports nothing else from the crate.

mod mirror;
pub mod types;

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use tokio::io::{AsyncBufReadExt, AsyncRead, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::net::UnixStream;
use tokio::net::unix::OwnedReadHalf;

pub use mirror::{Change, Mirror, PaneView};
use types::*;

/// The Herdr socket protocol this client was written against.
pub const PROTOCOL: u32 = 22;

const MAX_LINE: u64 = 16 << 20;

#[derive(Debug, thiserror::Error)]
pub enum HerdrError {
    /// The request was never sent, so it had no effect.
    #[error("connecting to {path}: {source}")]
    Connect {
        path: PathBuf,
        source: std::io::Error,
    },
    /// The request may or may not have been applied.
    #[error("{method}: connection failed after sending: {source}")]
    Io {
        method: &'static str,
        source: std::io::Error,
    },
    /// The request may or may not have been applied.
    #[error("{method}: timed out")]
    Timeout { method: &'static str },
    #[error("{method}: {code}: {message}")]
    Server {
        method: &'static str,
        code: String,
        message: String,
    },
    #[error("{method}: malformed response: {detail}")]
    Protocol {
        method: &'static str,
        detail: String,
    },
    #[error("herdr {version} speaks protocol {protocol}; switchboard supports {PROTOCOL}")]
    Incompatible { version: String, protocol: u32 },
    #[error("invalid argument: {0}")]
    InvalidArgument(String),
}

impl HerdrError {
    /// A mutation that failed this way must be reconciled, not retried blindly.
    pub fn outcome_unknown(&self) -> bool {
        matches!(self, HerdrError::Io { .. } | HerdrError::Timeout { .. })
    }

    /// Nothing listens on the socket: the server isn't running, as opposed to slow.
    pub fn server_gone(&self) -> bool {
        matches!(self, HerdrError::Connect { .. })
    }

    pub fn server_code(&self) -> Option<&str> {
        match self {
            HerdrError::Server { code, .. } => Some(code),
            _ => None,
        }
    }
}

pub type Result<T> = std::result::Result<T, HerdrError>;

/// Socket path of a named Herdr session under `config_dir` (normally `~/.config/herdr`).
pub fn session_socket(config_dir: &Path, session: &str) -> PathBuf {
    if session == "default" {
        config_dir.join("herdr.sock")
    } else {
        config_dir.join("sessions").join(session).join("herdr.sock")
    }
}

/// The socket the `herdr` CLI would use: `HERDR_SOCKET_PATH`, then `HERDR_SESSION`, then the
/// default session. A configured `session` overrides the environment.
pub fn find_socket(
    config_dir: &Path,
    session: Option<&str>,
    env: impl Fn(&str) -> Option<String>,
) -> PathBuf {
    let set = |k: &str| env(k).filter(|v| !v.is_empty());
    match (session, set("HERDR_SOCKET_PATH"), set("HERDR_SESSION")) {
        (Some(s), _, _) => session_socket(config_dir, s),
        (None, Some(path), _) => PathBuf::from(path),
        (None, None, Some(s)) => session_socket(config_dir, &s),
        (None, None, None) => session_socket(config_dir, "default"),
    }
}

pub struct Client {
    socket: PathBuf,
    timeout: Duration,
    next_id: AtomicU64,
}

impl Client {
    pub fn new(socket: impl Into<PathBuf>) -> Self {
        Self {
            socket: socket.into(),
            timeout: Duration::from_secs(30),
            next_id: AtomicU64::new(1),
        }
    }

    /// Per-request timeout, on top of any server-side wait the request asks for.
    pub fn with_timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }

    pub fn socket(&self) -> &Path {
        &self.socket
    }

    async fn connect(&self) -> Result<UnixStream> {
        UnixStream::connect(&self.socket)
            .await
            .map_err(|source| HerdrError::Connect {
                path: self.socket.clone(),
                source,
            })
    }

    async fn send(&self, stream: &mut UnixStream, method: &Method) -> Result<String> {
        let id = format!("sb-{}", self.next_id.fetch_add(1, Ordering::Relaxed));
        let mut line = serde_json::to_vec(&Request {
            id: id.clone(),
            method,
        })
        .expect("requests always serialize");
        line.push(b'\n');
        stream
            .write_all(&line)
            .await
            .map_err(|source| HerdrError::Io {
                method: method.name(),
                source,
            })?;
        Ok(id)
    }

    /// Sends one request on a fresh connection and returns its result.
    pub async fn call(
        &self,
        method: Method,
        extra_wait: Option<Duration>,
    ) -> Result<ResponseResult> {
        let name = method.name();
        let mut stream = self.connect().await?;
        let deadline = self.timeout + extra_wait.unwrap_or_default();
        let fut = async {
            let id = self.send(&mut stream, &method).await?;
            let (read, _write) = stream.into_split();
            let mut reader = BufReader::new(read);
            let line = read_line(&mut reader, name).await?;
            parse_response(name, &id, &line)
        };
        tokio::time::timeout(deadline, fut)
            .await
            .map_err(|_| HerdrError::Timeout { method: name })?
    }

    pub async fn ping(&self) -> Result<(String, u32)> {
        match self.call(Method::Ping(Empty {}), None).await? {
            ResponseResult::Pong { version, protocol } => Ok((version, protocol)),
            other => unexpected("ping", other),
        }
    }

    /// Refuses to proceed against a protocol version this client doesn't know.
    pub async fn check_protocol(&self) -> Result<String> {
        let (version, protocol) = self.ping().await?;
        if protocol != PROTOCOL {
            return Err(HerdrError::Incompatible { version, protocol });
        }
        Ok(version)
    }

    pub async fn session_snapshot(&self) -> Result<SessionSnapshot> {
        match self.call(Method::SessionSnapshot(Empty {}), None).await? {
            ResponseResult::SessionSnapshot { snapshot } => Ok(snapshot),
            other => unexpected("session.snapshot", other),
        }
    }

    pub async fn worktree_create(&self, params: WorktreeCreateParams) -> Result<WorktreeCreated> {
        match self.call(Method::WorktreeCreate(params), None).await? {
            ResponseResult::WorktreeCreated {
                workspace,
                tab,
                root_pane,
                worktree,
            } => Ok(WorktreeCreated {
                workspace,
                tab,
                root_pane,
                worktree,
            }),
            other => unexpected("worktree.create", other),
        }
    }

    pub async fn worktree_list(
        &self,
        params: WorktreeListParams,
    ) -> Result<(WorktreeSourceInfo, Vec<WorktreeInfo>)> {
        match self.call(Method::WorktreeList(params), None).await? {
            ResponseResult::WorktreeList { source, worktrees } => Ok((source, worktrees)),
            other => unexpected("worktree.list", other),
        }
    }

    /// Never forces: Herdr refuses to remove a dirty worktree.
    pub async fn worktree_remove(&self, workspace_id: &str) -> Result<String> {
        let params = WorktreeRemoveParams {
            workspace_id: workspace_id.to_owned(),
        };
        match self.call(Method::WorktreeRemove(params), None).await? {
            ResponseResult::WorktreeRemoved { path, .. } => Ok(path),
            other => unexpected("worktree.remove", other),
        }
    }

    pub async fn agent_start(&self, params: AgentStartParams) -> Result<AgentInfo> {
        check_agent_name(&params.name)?;
        if let Some(t) = params.timeout_ms
            && !(3001..=300_000).contains(&t)
        {
            return Err(HerdrError::InvalidArgument(format!(
                "agent.start timeout_ms must be in 3001..=300000, got {t}"
            )));
        }
        let wait = Duration::from_millis(params.timeout_ms.unwrap_or(30_000));
        match self.call(Method::AgentStart(params), Some(wait)).await? {
            ResponseResult::AgentStarted { agent, .. } | ResponseResult::AgentInfo { agent } => {
                Ok(agent)
            }
            other => unexpected("agent.start", other),
        }
    }

    /// Herdr rejects this with `agent_blocked` while the agent sits at a dialog.
    pub async fn agent_prompt(&self, params: AgentPromptParams) -> Result<AgentInfo> {
        let wait = params
            .wait
            .as_ref()
            .and_then(|w| w.timeout_ms)
            .map(Duration::from_millis);
        match self.call(Method::AgentPrompt(params), wait).await? {
            ResponseResult::AgentPrompted { agent } | ResponseResult::AgentInfo { agent } => {
                Ok(agent)
            }
            other => unexpected("agent.prompt", other),
        }
    }

    pub async fn agent_wait(&self, params: AgentWaitParams) -> Result<AgentInfo> {
        let wait = params.timeout_ms.map(Duration::from_millis);
        match self.call(Method::AgentWait(params), wait).await? {
            ResponseResult::AgentInfo { agent } => Ok(agent),
            other => unexpected("agent.wait", other),
        }
    }

    pub async fn agent_read(
        &self,
        target: &str,
        source: ReadSource,
        lines: Option<u32>,
    ) -> Result<PaneReadResult> {
        let params = AgentReadParams {
            target: target.to_owned(),
            source,
            lines,
            format: ReadFormat::Text,
            strip_ansi: true,
        };
        match self.call(Method::AgentRead(params), None).await? {
            ResponseResult::PaneRead { read } => Ok(read),
            other => unexpected("agent.read", other),
        }
    }

    /// The visible screen with its colours, as ANSI escape codes. Untrusted text.
    pub async fn agent_read_screen_ansi(&self, target: &str) -> Result<PaneReadResult> {
        let params = AgentReadParams {
            target: target.to_owned(),
            source: ReadSource::Visible,
            lines: None,
            format: ReadFormat::Ansi,
            strip_ansi: false,
        };
        match self.call(Method::AgentRead(params), None).await? {
            ResponseResult::PaneRead { read } => Ok(read),
            other => unexpected("agent.read", other),
        }
    }

    /// The pids of a pane's foreground processes.
    pub async fn pane_foreground_pids(&self, pane_id: &str) -> Result<Vec<u32>> {
        let params = PaneProcessInfoParams {
            pane_id: pane_id.to_owned(),
        };
        match self.call(Method::PaneProcessInfo(params), None).await? {
            ResponseResult::PaneProcessInfo { process_info } => Ok(process_info
                .foreground_processes
                .into_iter()
                .map(|p| p.pid)
                .collect()),
            other => unexpected("pane.process_info", other),
        }
    }

    pub async fn agent_send_keys(&self, target: &str, keys: Vec<String>) -> Result<()> {
        let params = AgentSendKeysParams {
            target: target.to_owned(),
            keys,
        };
        self.call(Method::AgentSendKeys(params), None)
            .await
            .map(drop)
    }

    pub async fn workspace_close(&self, workspace_id: &str) -> Result<()> {
        let params = WorkspaceCloseParams {
            workspace_id: workspace_id.to_owned(),
        };
        self.call(Method::WorkspaceClose(params), None)
            .await
            .map(drop)
    }

    /// Opens a subscription and waits for its acknowledgement.
    pub async fn subscribe(&self, subscriptions: Vec<Subscription>) -> Result<EventStream> {
        const NAME: &str = "events.subscribe";
        let method = Method::EventsSubscribe(EventsSubscribeParams { subscriptions });
        let mut stream = self.connect().await?;
        let ack = async {
            let id = self.send(&mut stream, &method).await?;
            let (read, write) = stream.into_split();
            let mut reader = BufReader::new(read);
            let line = read_line(&mut reader, NAME).await?;
            match parse_response(NAME, &id, &line)? {
                ResponseResult::SubscriptionStarted => Ok(EventStream {
                    reader,
                    _write: write,
                }),
                other => unexpected(NAME, other),
            }
        };
        tokio::time::timeout(self.timeout, ack)
            .await
            .map_err(|_| HerdrError::Timeout { method: NAME })?
    }
}

#[derive(Debug, Clone)]
pub struct WorktreeCreated {
    pub workspace: WorkspaceInfo,
    pub tab: TabInfo,
    pub root_pane: PaneInfo,
    pub worktree: WorktreeInfo,
}

/// Pushed events from one subscription. `None` from `next` means events may have been
/// lost: resubscribe and reconcile from `session.snapshot`.
pub struct EventStream {
    reader: BufReader<OwnedReadHalf>,
    _write: tokio::net::unix::OwnedWriteHalf,
}

impl EventStream {
    pub async fn next(&mut self) -> Option<Result<Event>> {
        const NAME: &str = "events.subscribe";
        let line = match read_line(&mut self.reader, NAME).await {
            Ok(line) => line,
            Err(_) => return None,
        };
        Some(
            serde_json::from_str::<EventFrame>(&line)
                .and_then(EventFrame::into_event)
                .map_err(|e| HerdrError::Protocol {
                    method: NAME,
                    detail: e.to_string(),
                }),
        )
    }
}

async fn read_line<R: AsyncRead + Unpin>(
    reader: &mut BufReader<R>,
    method: &'static str,
) -> Result<String> {
    let mut buf = Vec::new();
    let n = (&mut *reader)
        .take(MAX_LINE)
        .read_until(b'\n', &mut buf)
        .await
        .map_err(|source| HerdrError::Io { method, source })?;
    if n == 0 {
        return Err(HerdrError::Io {
            method,
            source: std::io::ErrorKind::UnexpectedEof.into(),
        });
    }
    if buf.last() != Some(&b'\n') {
        return Err(HerdrError::Protocol {
            method,
            detail: "line too long or truncated".into(),
        });
    }
    String::from_utf8(buf).map_err(|e| HerdrError::Protocol {
        method,
        detail: e.to_string(),
    })
}

pub(crate) fn parse_response(method: &'static str, id: &str, line: &str) -> Result<ResponseResult> {
    let resp: Response = serde_json::from_str(line).map_err(|e| HerdrError::Protocol {
        method,
        detail: e.to_string(),
    })?;
    if let Some(err) = resp.error {
        return Err(HerdrError::Server {
            method,
            code: err.code,
            message: err.message,
        });
    }
    if resp.id != id {
        return Err(HerdrError::Protocol {
            method,
            detail: format!("response id {:?} does not match {id:?}", resp.id),
        });
    }
    resp.result.ok_or_else(|| HerdrError::Protocol {
        method,
        detail: "no result or error".into(),
    })
}

fn unexpected<T>(method: &'static str, got: ResponseResult) -> Result<T> {
    Err(HerdrError::Protocol {
        method,
        detail: format!("unexpected result {got:?}"),
    })
}

/// Herdr agent names: `[a-z][a-z0-9_-]{0,31}`.
/// An agent's name, or the pane id of one that has none (`w1:p2`).
pub fn check_agent_target(target: &str) -> Result<()> {
    let digits = |s: &str| !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit());
    let pane = target.split_once(':').is_some_and(|(w, p)| {
        w.strip_prefix('w').is_some_and(digits) && p.strip_prefix('p').is_some_and(digits)
    });
    if pane {
        Ok(())
    } else {
        check_agent_name(target)
    }
}

pub fn check_agent_name(name: &str) -> Result<()> {
    let mut chars = name.chars();
    let ok = name.len() <= 32
        && chars.next().is_some_and(|c| c.is_ascii_lowercase())
        && chars.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_' || c == '-');
    if ok {
        Ok(())
    } else {
        Err(HerdrError::InvalidArgument(format!("agent name {name:?}")))
    }
}

#[cfg(test)]
mod tests;
