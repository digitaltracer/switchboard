//! Runner-side sessions: start, prompt, read, decide, stop, and their on-disk state.

use std::collections::HashMap;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tokio::sync::mpsc;
use tokio::time::Instant;

use super::agents;
use super::backend::{self, Facts, Read, Server};
use super::codex::{CodexPane, clear_startup, excerpt};
use super::core_watch::{self, Core};
use super::dialogs::{self, Answered, PaneLocks, ServerPane};
use super::herdrs::{Herdrs, qualified, split_qualified};
use super::hooks::{DecideError, HookHub};
use super::logcache::{self, LogCache};
use super::logwatch::{self, Found, LogWatch};
use super::{skills, transcript};
use crate::agents::{claude, codex, hook_command};
use crate::cli::Agent;
use crate::herdr::types::{
    AgentPromptParams, AgentStartParams, AgentStatus, ReadSource, WorktreeCreateParams,
};
use crate::herdr::{Client, HerdrError};
use crate::hook::wire::SecretFile;
use crate::proto::{ErrorCode, Event, Op, OwnedSession, ProtoError, TranscriptPage};
pub use core_start::{CoreStarts, LAUNCHER, SESSION_LABEL};

const AGENT_START_TIMEOUT_MS: u64 = 60_000;
const PANE_BUSY_RETRY: Duration = Duration::from_millis(500);
const PANE_BUSY_ATTEMPTS: u32 = 20;
const DEFAULT_READ_LINES: u32 = 80;
const MAX_CHANGED_FILES: usize = 200;
/// Queue slots status updates leave free for approvals and lifecycle events.
const STATUS_RESERVE: usize = 64;

/// What the runner keeps about a session, persisted as `meta.json` in its directory.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RunnerSession {
    pub session_id: String,
    pub agent: Agent,
    pub project_id: String,
    pub repo_path: PathBuf,
    pub branch: String,
    pub deny_patterns: Vec<String>,
    pub agent_name: String,
    pub workspace_id: Option<String>,
    pub pane_id: Option<String>,
    pub worktree_path: Option<String>,
    pub codex_version: Option<String>,
    /// The commit the worktree was created from.
    #[serde(default)]
    pub start_commit: Option<String>,
    /// The Herdr session this task runs in; none for tasks from before each got its own.
    #[serde(default)]
    pub herdr_session: Option<String>,
    /// False until the agent has started; a runner that finds a false one undoes the start.
    #[serde(default = "launched_before_this_field")]
    pub launched: bool,
    /// The herdr-core a session in the Herdr app started on.
    #[serde(default)]
    pub core_id: Option<String>,
    /// Its terminal's identity there, `core:address:pid:created_ms`.
    #[serde(default)]
    pub terminal_identity: Option<String>,
}

/// Tasks saved before `launched` existed were saved only after a complete start.
fn launched_before_this_field() -> bool {
    true
}

pub struct SessionsConfig {
    pub state_dir: PathBuf,
    pub claude_config: PathBuf,
    pub claude_mode: claude::PermissionMode,
    /// Where Claude Code keeps its conversation logs, normally `~/.claude/projects`.
    pub claude_projects: PathBuf,
    /// Where Codex keeps its, normally `~/.codex/sessions`.
    pub codex_sessions: PathBuf,
    pub exe: PathBuf,
    pub hook_addr: SocketAddr,
    /// Command printing the Codex version, normally `codex --version`.
    pub codex_version_cmd: Vec<String>,
    /// How long a Claude session may take to prove its hook is live.
    pub liveness_timeout: Duration,
    /// How long a Codex startup screen may take to be recognised and cleared.
    pub startup_timeout: Duration,
}

pub struct Sessions {
    pub(crate) herdrs: Herdrs,
    pub(crate) hooks: Arc<HookHub>,
    events: mpsc::Sender<Event>,
    cfg: SessionsConfig,
    sessions: Mutex<HashMap<String, RunnerSession>>,
    pub(crate) logwatch: Arc<LogWatch>,
    /// Which log each pane's agent writes.
    pub(crate) logs: Arc<LogCache>,
    /// Transcript reads waiting for their log to grow.
    transcript_waits: tokio::sync::Semaphore,
    /// Agents whose Herdr status really changed, by the name the hub uses.
    status_changes: tokio::sync::broadcast::Sender<String>,
    /// One action at a time per pane: dialog answers, prompts and the Codex key paths.
    pub(crate) pane_locks: PaneLocks,
    answered: Arc<Answered>,
    /// Occurrences already reported as waiting.
    waited: Mutex<std::collections::VecDeque<String>>,
    /// This machine's herdr-core, when the runner follows one.
    core: std::sync::OnceLock<Arc<Core>>,
    core_starts: std::sync::OnceLock<CoreStarts>,
}

/// How long an agent stays blocked before its owner's phones are told it's waiting.
pub const WAITING_HOLD: Duration = Duration::from_secs(3);
const KEEP_WAITED: usize = 500;

fn err(code: ErrorCode, message: impl Into<String>) -> ProtoError {
    ProtoError::new(code, message)
}

fn herdr_err(e: HerdrError) -> ProtoError {
    backend_err(e.into())
}

fn backend_err(e: backend::Error) -> ProtoError {
    let code = if e.outcome_unknown() {
        ErrorCode::OutcomeUnknown
    } else if e.is_blocked() {
        ErrorCode::AgentBlocked
    } else if e.is_not_found() {
        ErrorCode::NotFound
    } else if e.is_invalid_argument() {
        ErrorCode::BadRequest
    } else {
        ErrorCode::Herdr
    };
    err(code, e.to_string())
}

/// Session ids name directories, so they are restricted to `[a-z0-9-]{1,40}`.
pub fn check_session_id(id: &str) -> Result<(), ProtoError> {
    let ok = !id.is_empty()
        && id.len() <= 40
        && id
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-');
    if ok {
        Ok(())
    } else {
        Err(err(ErrorCode::BadRequest, format!("bad session id {id:?}")))
    }
}

/// Herdr agent name for a session: `sb-` plus up to 29 characters of the id.
pub fn agent_name(session_id: &str) -> String {
    let tail: String = session_id.chars().filter(|c| *c != '-').take(29).collect();
    format!("sb-{tail}")
}

impl Sessions {
    /// Every task in one Herdr server, as before.
    pub fn new(
        herdr: Client,
        hooks: Arc<HookHub>,
        events: mpsc::Sender<Event>,
        cfg: SessionsConfig,
    ) -> Arc<Self> {
        Self::with_herdrs(Herdrs::single(herdr.socket()), hooks, events, cfg)
    }

    /// Each new task in a Herdr session of its own.
    pub fn with_herdrs(
        herdrs: Herdrs,
        hooks: Arc<HookHub>,
        events: mpsc::Sender<Event>,
        cfg: SessionsConfig,
    ) -> Arc<Self> {
        Arc::new(Self {
            herdrs,
            hooks,
            events,
            sessions: Mutex::default(),
            logwatch: LogWatch::new(),
            logs: Arc::new(LogCache::new(
                cfg.claude_projects.clone(),
                cfg.codex_sessions.clone(),
            )),
            transcript_waits: tokio::sync::Semaphore::new(logwatch::MAX_WAITS),
            status_changes: tokio::sync::broadcast::channel(256).0,
            pane_locks: PaneLocks::default(),
            answered: Answered::load(cfg.state_dir.join("dialog-answers.json")),
            waited: Mutex::default(),
            core: std::sync::OnceLock::new(),
            core_starts: std::sync::OnceLock::new(),
            cfg,
        })
    }

    /// Follows `core` too; set once, at start.
    pub fn set_core(&self, core: Arc<Core>) {
        let _ = self.core.set(core);
    }

    pub(crate) fn core(&self) -> Option<Arc<Core>> {
        self.core.get().cloned()
    }

    /// The Herdr server a task runs in.
    pub(crate) fn herdr_for(&self, s: &RunnerSession) -> Client {
        self.herdrs.client(s.herdr_session.as_deref())
    }

    /// The server a task runs in.
    pub(crate) fn server_for(&self, s: &RunnerSession) -> Server {
        match (s.herdr_session.as_deref(), self.core()) {
            (Some(core_watch::SERVER), Some(core)) => Server::Core(core),
            _ => Server::herdr(s.herdr_session.clone(), self.herdr_for(s)),
        }
    }

    /// What a task's own prompts and reads go to: its terminal's address on core, else its name.
    pub(crate) fn target_for(s: &RunnerSession) -> &str {
        match (core_start::is_core(s), &s.pane_id) {
            (true, Some(address)) => address,
            _ => &s.agent_name,
        }
    }

    /// Every server that answers: the running Herdr sessions, then the core when it's up.
    async fn servers(&self) -> Vec<Server> {
        let mut out: Vec<Server> = self
            .herdrs
            .running()
            .await
            .into_iter()
            .map(|(name, client)| Server::herdr(name, client))
            .collect();
        if let Some(core) = self.core().filter(|c| c.core_id().is_some()) {
            out.push(Server::Core(core));
        }
        out
    }

    /// A TUI pane showing a core terminal (`hc attach`) is listed once, from the core.
    async fn attached_to_core(&self, server: &Server, facts: &Facts) -> bool {
        let Some(client) = server.as_herdr() else {
            return false;
        };
        if self.core().is_none_or(|c| c.core_id().is_none()) {
            return false;
        }
        client
            .pane_foreground_names(&facts.target)
            .await
            .is_ok_and(|names| {
                names
                    .iter()
                    .any(|n| matches!(n.rsplit('/').next(), Some("hc" | "herdr-core")))
            })
    }

    /// The server's agents without TUI panes that only show a core terminal.
    async fn listed(&self, server: &Server) -> backend::Result<Vec<Facts>> {
        let mut out = Vec::new();
        for f in server.agents().await? {
            if !self.attached_to_core(server, &f).await {
                out.push(f);
            }
        }
        Ok(out)
    }

    /// Where a named agent lives: `<session>/<agent>`, a task's own agent, or the legacy server.
    fn resolve_agent<'a>(&self, name: &'a str) -> (Server, &'a str, Option<RunnerSession>) {
        let (session, agent) = split_qualified(name);
        let owned = self.all().into_iter().find(|s| {
            s.agent_name == agent && (session.is_none() || s.herdr_session.as_deref() == session)
        });
        match (session, &owned) {
            (Some(core_watch::SERVER), _) if let Some(core) = self.core() => {
                (Server::Core(core), agent, owned)
            }
            (Some(s), _) => (
                Server::herdr(Some(s.to_owned()), self.herdrs.client(Some(s))),
                agent,
                owned,
            ),
            (None, Some(o)) => (self.server_for(o), agent, owned.clone()),
            (None, None) => (Server::herdr(None, self.herdrs.client(None)), agent, None),
        }
    }

    /// Agents in every running Herdr session; agents outside tasks are `<session>/<agent>`.
    async fn all_agents(&self) -> Result<Vec<crate::proto::AgentSummary>, ProtoError> {
        let names: HashMap<String, String> = self
            .all()
            .into_iter()
            .map(|s| (s.agent_name, s.session_id))
            .collect();
        let mut out = Vec::new();
        for server in self.servers().await {
            let Ok(facts) = self.listed(&server).await else {
                continue;
            };
            let name = server.name().map(str::to_owned);
            for mut a in agents::summaries(facts, &names) {
                if a.session_id.is_none() {
                    a.name = qualified(name.as_deref(), &a.name);
                }
                a.herdr_session = name.clone();
                out.push(a);
            }
        }
        Ok(out)
    }

    /// Every agent's last line, named as `all_agents` names them, from one snapshot per Herdr
    /// server and the log cache. An agent without a log, or that isn't Claude or Codex, has none.
    async fn activity(&self) -> Vec<crate::proto::AgentActivity> {
        let tasks = self.all();
        let mut out = Vec::new();
        let mut logs = std::collections::HashSet::new();
        for srv in self.servers().await {
            let Ok(facts) = self.listed(&srv).await else {
                continue;
            };
            let server = srv.name().map(str::to_owned);
            let panes = facts.iter().map(|a| a.target.clone()).collect();
            self.logs.retain(srv.socket(), &panes);
            for info in facts {
                let local = info.local_name();
                let task = tasks.iter().find(|s| s.agent_name == local);
                let name = match task {
                    Some(_) => local.clone(),
                    None => qualified(server.as_deref(), &local),
                };
                let owned = task
                    .filter(|s| server.is_none() || s.herdr_session == server)
                    .cloned();
                let line = match info.kind.as_deref() {
                    Some("claude" | "codex") => match self.log_for(&srv, owned, info).await {
                        Ok(found) => {
                            logs.insert(found.log.clone());
                            let cache = self.logs.clone();
                            tokio::task::spawn_blocking(move || cache.last_line(&found.log))
                                .await
                                .ok()
                                .flatten()
                        }
                        Err(_) => None,
                    },
                    _ => None,
                };
                let (line, at) = line.unzip();
                out.push(crate::proto::AgentActivity {
                    name,
                    session_id: task.map(|s| s.session_id.clone()),
                    line,
                    at: at.flatten(),
                });
            }
        }
        self.logs.keep_lines(&logs);
        tracing::debug!(
            agents = out.len(),
            searches = self.logs.searches(),
            bytes_read = self.logs.bytes_read(),
            "activity"
        );
        out
    }

    fn dir(&self, session_id: &str) -> PathBuf {
        self.cfg.state_dir.join("sessions").join(session_id)
    }

    pub fn get(&self, session_id: &str) -> Option<RunnerSession> {
        self.sessions.lock().unwrap().get(session_id).cloned()
    }

    pub fn all(&self) -> Vec<RunnerSession> {
        self.sessions.lock().unwrap().values().cloned().collect()
    }

    pub(crate) fn emit(&self, event: Event) {
        if let Err(e) = self.events.try_send(event) {
            tracing::warn!(error = %e, "dropping runner event");
        }
    }

    /// For status updates that come again anyway: they never take the room approvals and
    /// lifecycle events need while the hub is away.
    /// Wakes transcript reads waiting on `agent`, whose log may have changed with its status.
    pub(crate) fn status_changed(&self, agent: &str) {
        let _ = self.status_changes.send(agent.to_owned());
    }

    pub(crate) fn emit_status(&self, event: Event) {
        if self.events.capacity() > STATUS_RESERVE {
            self.emit(event);
        } else {
            tracing::debug!("event queue nearly full; skipping a status update");
        }
    }

    fn audit(&self, session_id: &str, kind: &str, detail: Value) {
        tracing::info!(session_id, kind, %detail, "audit");
        self.emit(Event::Audit {
            session_id: session_id.to_owned(),
            kind: kind.to_owned(),
            detail,
        });
    }

    /// Latches `hook_failed`: the session stays blocked and no more keys are sent to it.
    pub(crate) fn hook_failed(
        &self,
        s: &RunnerSession,
        reason: &str,
        screen_excerpt: Option<String>,
    ) {
        self.hooks.mark_hook_failed(&s.session_id);
        self.audit(
            &s.session_id,
            "hook_failed",
            json!({ "reason": reason, "screen_excerpt": screen_excerpt }),
        );
        self.emit(Event::HookFailed {
            session_id: s.session_id.clone(),
            reason: reason.to_owned(),
            screen_excerpt,
        });
    }

    pub(crate) fn fallback_denied(&self, s: &RunnerSession, screen_excerpt: String) {
        self.audit(
            &s.session_id,
            "fallback_deny",
            json!({ "screen_excerpt": screen_excerpt }),
        );
    }

    fn save(&self, s: &RunnerSession) -> std::io::Result<()> {
        let path = self.dir(&s.session_id).join("meta.json");
        std::fs::write(
            path,
            serde_json::to_vec_pretty(s).map_err(std::io::Error::other)?,
        )?;
        self.sessions
            .lock()
            .unwrap()
            .insert(s.session_id.clone(), s.clone());
        Ok(())
    }

    /// Reloads sessions left by a previous runner process and re-registers their hooks.
    pub fn load(&self) -> std::io::Result<usize> {
        let root = self.cfg.state_dir.join("sessions");
        let Ok(entries) = std::fs::read_dir(&root) else {
            return Ok(0);
        };
        let mut n = 0;
        for entry in entries.flatten() {
            let dir = entry.path();
            let meta = std::fs::read(dir.join("meta.json"))
                .ok()
                .and_then(|b| serde_json::from_slice::<RunnerSession>(&b).ok());
            let secret = SecretFile::read(&dir.join("secret.json")).ok();
            let (Some(meta), Some(secret)) = (meta, secret) else {
                tracing::warn!(dir = %dir.display(), "skipping unreadable session directory");
                continue;
            };
            // The hook endpoint port changes per process; point the helper at the new one.
            let secret = SecretFile {
                addr: self.cfg.hook_addr,
                ..secret
            };
            secret.write(&dir.join("secret.json"))?;
            self.hooks.register(
                &meta.session_id,
                &secret.token,
                meta.agent,
                meta.deny_patterns.clone(),
            );
            self.sessions
                .lock()
                .unwrap()
                .insert(meta.session_id.clone(), meta);
            n += 1;
        }
        Ok(n)
    }

    /// Undoes starts a previous runner process didn't finish; the hub fails them once they're gone.
    pub async fn undo_interrupted_starts(&self) -> usize {
        let interrupted: Vec<RunnerSession> =
            self.all().into_iter().filter(|s| !s.launched).collect();
        let e = err(ErrorCode::Internal, "the runner restarted during the start");
        for s in &interrupted {
            self.abort(s, &e).await;
        }
        interrupted.len()
    }

    pub async fn handle(&self, op: Op) -> Result<Value, ProtoError> {
        match op {
            Op::StartSession {
                session_id,
                project_id,
                repo_path,
                branch,
                agent,
                prompt,
                deny_patterns,
                backend,
            } => {
                self.start(StartRequest {
                    session_id,
                    project_id,
                    repo_path: repo_path.into(),
                    branch,
                    agent,
                    prompt,
                    deny_patterns,
                    backend,
                })
                .await
            }
            Op::Prompt { session_id, text } => self.prompt(&session_id, &text).await,
            Op::Read { session_id, lines } => self.read(&session_id, lines).await,
            Op::Decide {
                session_id,
                approval_id,
                allow,
                choices,
            } => self.decide(&session_id, &approval_id, allow, &choices),
            Op::Dialog { agent } => self.dialog(&agent).await,
            Op::Answer {
                agent,
                occurrence,
                option,
                label_sha256,
                deny_only,
            } => {
                let req = dialogs::AnswerRequest {
                    occurrence,
                    option,
                    label_sha256,
                    deny_only,
                };
                self.answer(&agent, &req).await
            }
            Op::Back { agent, occurrence } => self.back(&agent, &occurrence).await,
            Op::Stop {
                session_id,
                remove_worktree_if_clean,
            } => self.stop(&session_id, remove_worktree_if_clean).await,
            Op::Delete {
                session_id,
                herdr_session,
                repo_path,
                worktree_path,
                branch,
            } => {
                self.delete(&session_id, herdr_session, repo_path, worktree_path, branch)
                    .await
            }
            Op::ListSessions => Ok(serde_json::to_value(self.owned().await).expect("serializes")),
            Op::Agents => Ok(serde_json::to_value(self.all_agents().await?).expect("serializes")),
            Op::Activity => Ok(serde_json::to_value(self.activity().await).expect("serializes")),
            Op::StartOptions => {
                Ok(serde_json::to_value(self.start_options().await).expect("serializes"))
            }
            Op::ReadAgent {
                agent,
                lines,
                visible,
                ansi,
            } => {
                let lines = if lines == 0 {
                    DEFAULT_READ_LINES
                } else {
                    lines
                };
                let (server, agent, _) = self.resolve_agent(&agent);
                let text = agents::read(&server, agent, lines, visible)
                    .await
                    .map_err(backend_err)?;
                let mut out = json!({ "text": text, "untrusted": true });
                if visible
                    && ansi
                    && let Some(screen) = agents::screen_ansi(&server, agent).await
                {
                    out["ansi"] = Value::String(screen);
                }
                Ok(out)
            }
            Op::PromptAgent { agent, text } => {
                let (server, name, owned) = self.resolve_agent(&agent);
                // A session's prompts go through its own op, which the hub records and audits.
                if owned.is_some() {
                    return Err(err(
                        ErrorCode::BadRequest,
                        format!("{agent} is a Switchboard session; prompt the session"),
                    ));
                }
                let pane = server.agent(name).await.ok().map(|a| a.target);
                let _lock = match &pane {
                    Some(p) => Some(self.pane_locks.lock(server.name(), p).await),
                    None => None,
                };
                let outcome = agents::prompt(&server, name, &text)
                    .await
                    .map_err(backend_err)?;
                Ok(serde_json::to_value(outcome).expect("serializes"))
            }
            Op::Transcript {
                agent,
                after,
                before,
                wait_ms,
            } => self.transcript(&agent, after, before, wait_ms).await,
            Op::Skills { agent } => self.skills(&agent).await,
            Op::Changes { session_id } => {
                let s = self.session(&session_id)?;
                match (&s.worktree_path, &s.start_commit) {
                    (Some(wt), Some(base)) => changes(Path::new(wt), base)
                        .await
                        .map_err(|e| err(ErrorCode::Internal, e)),
                    _ => Ok(json!({ "files": [], "truncated": false })),
                }
            }
            Op::Summarize { prompt, model } => super::summary::summarize(&prompt, &model)
                .await
                .map(|s| json!({ "summary": s }))
                .map_err(|e| err(ErrorCode::Internal, e)),
            Op::Diff {
                session_id,
                path,
                from,
            } => {
                let s = self.session(&session_id)?;
                match (&s.worktree_path, &s.start_commit) {
                    (Some(wt), Some(base)) => super::diff::diff(Path::new(wt), base, &path, from)
                        .await
                        .map_err(|e| err(ErrorCode::BadRequest, e)),
                    _ => Err(err(ErrorCode::BadRequest, "this session has no worktree")),
                }
            }
        }
    }

    /// A page of an agent's conversation from its own log. With `after` and `wait_ms`, it first
    /// waits for the log to grow past `after`, woken by the OS, for at most 20 s in all.
    async fn transcript(
        &self,
        agent: &str,
        after: Option<u64>,
        before: Option<u64>,
        wait_ms: Option<u64>,
    ) -> Result<Value, ProtoError> {
        let start = Instant::now();
        let wait = match (after, before, wait_ms) {
            (Some(after), None, Some(ms)) if ms > 0 => {
                self.transcript_waits.try_acquire().ok().map(|slot| {
                    (
                        slot,
                        after,
                        start + Duration::from_millis(ms).min(logwatch::MAX_WAIT),
                    )
                })
            }
            _ => None,
        };
        let path = match wait {
            None => self.find_log(agent).await?.log,
            Some((_slot, after, deadline)) => {
                // Subscribed before the lookup, so a change during it isn't missed.
                let status = self.status_changes.subscribe();
                let found = tokio::time::timeout_at(deadline, self.find_log(agent))
                    .await
                    .map_err(|_| {
                        err(
                            ErrorCode::DeadlineExceeded,
                            "finding the conversation took too long",
                        )
                    })??;
                logwatch::wait(
                    &self.logwatch,
                    Some(status),
                    agent,
                    after,
                    deadline,
                    found,
                    || async { self.find_log(agent).await.ok() },
                )
                .await
            }
        };
        let logs = self.logs.clone();
        let mut page = tokio::task::spawn_blocking(move || {
            let mut page = transcript::read_page(&path, after, before).map_err(|e| {
                err(
                    ErrorCode::Internal,
                    format!("reading the conversation: {e}"),
                )
            })?;
            if before.is_none() {
                page.queued = logs.queued(&path, page.end);
            }
            Ok::<_, ProtoError>(page)
        })
        .await
        .map_err(|e| err(ErrorCode::Internal, e.to_string()))??;
        self.name_senders(&mut page);
        Ok(serde_json::to_value(page).expect("serializes"))
    }

    /// Names each message another terminal sent by that terminal, where this machine's core knows it.
    fn name_senders(&self, page: &mut TranscriptPage) {
        let Some(core) = self.core() else {
            return;
        };
        for from in page
            .items
            .iter_mut()
            .chain(&mut page.queued)
            .filter_map(|i| i.from.as_mut())
        {
            if let Some(n) = core.sender(from) {
                *from = n;
            }
        }
    }

    /// An agent's log: a Claude agent's found through the Claude running in its pane, else the
    /// session id Herdr reports; a Codex agent's through its folder.
    async fn find_log(&self, agent: &str) -> Result<Found, ProtoError> {
        let (server, name, owned) = self.resolve_agent(agent);
        let facts = server.agents().await.map_err(backend_err)?;
        let panes = facts.iter().map(|a| a.target.clone()).collect();
        self.logs.retain(server.socket(), &panes);
        let info = facts
            .into_iter()
            .find(|a| a.is(name))
            .ok_or_else(|| err(ErrorCode::NotFound, format!("no agent {agent:?}")))?;
        self.log_for(&server, owned, info).await
    }

    /// The log of the agent its server describes as `info`: the cached one while it still holds,
    /// else a full search. Pids are read only for a search.
    async fn log_for(
        &self,
        server: &Server,
        owned: Option<RunnerSession>,
        info: Facts,
    ) -> Result<Found, ProtoError> {
        let none = || {
            err(
                ErrorCode::NotFound,
                "no conversation log for this agent yet",
            )
        };
        let (kind, cwd) = match info.kind.as_deref() {
            Some("claude") => ("claude", info.cwd.clone()),
            // A session's own worktree, else wherever Herdr says the agent runs.
            Some("codex") => (
                "codex",
                Some(
                    owned
                        .and_then(|s| s.worktree_path)
                        .or(info.cwd.clone())
                        .ok_or_else(none)?,
                ),
            ),
            _ => {
                return Err(err(
                    ErrorCode::BadRequest,
                    "only Claude and Codex agents have a conversation to read",
                ));
            }
        };
        let pane = logcache::Pane {
            server: server.socket().to_owned(),
            pane_id: info.target.clone(),
            terminal_id: info.identity.clone(),
            kind: kind.to_owned(),
            reported: info.reported_session.clone(),
            cwd,
        };
        let blocking = |f: Box<dyn FnOnce() -> Option<Found> + Send>| async move {
            tokio::task::spawn_blocking(f)
                .await
                .map_err(|e| err(ErrorCode::Internal, e.to_string()))
        };
        let (logs, probe) = (self.logs.clone(), pane.clone());
        if let Some(found) = blocking(Box::new(move || logs.hit(&probe))).await? {
            return Ok(found);
        }
        let pids = match kind {
            "claude" => server.pids(&info).await.unwrap_or_default(),
            _ => Vec::new(),
        };
        let logs = self.logs.clone();
        blocking(Box::new(move || logs.search(pane, pids)))
            .await?
            .ok_or_else(none)
    }

    /// The skills and commands an agent can run, from its machine's and its repo's folders.
    async fn skills(&self, agent: &str) -> Result<Value, ProtoError> {
        let (server, name, owned) = self.resolve_agent(agent);
        let info = server
            .agents()
            .await
            .map_err(backend_err)?
            .into_iter()
            .find(|a| a.is(name))
            .ok_or_else(|| err(ErrorCode::NotFound, format!("no agent {agent:?}")))?;
        let cwd = owned.and_then(|s| s.worktree_path).or(info.cwd);
        let kind = info.kind.unwrap_or_default();
        let claude_dir = self.cfg.claude_projects.parent().map(Path::to_owned);
        let codex_home = self.cfg.codex_sessions.parent().map(Path::to_owned);
        let list = tokio::task::spawn_blocking(move || match kind.as_str() {
            "claude" => claude_dir.map_or_else(Vec::new, |dir| {
                let repo = cwd.as_deref().map(|c| repo_root(Path::new(c)));
                skills::claude(&dir, repo.as_deref())
            }),
            "codex" => codex_home.map_or_else(Vec::new, |home| skills::codex(&home)),
            _ => Vec::new(),
        })
        .await
        .map_err(|e| err(ErrorCode::Internal, e.to_string()))?;
        Ok(json!({ "skills": list }))
    }

    /// An agent's pane for the dialog path: its server and pane, and the session it belongs to,
    /// if any.
    async fn dialog_pane(&self, agent: &str) -> Result<DialogPane, ProtoError> {
        let (srv, name, owned) = self.resolve_agent(agent);
        let info = srv.agent(name).await.map_err(backend_err)?;
        let server = srv.name().map(str::to_owned);
        let pane_id = info.target.clone();
        // The pane decides whose it is, however it was asked for; on core, only the same process.
        let owned = self
            .all()
            .into_iter()
            .find(|s| {
                s.herdr_session == server
                    && s.pane_id.as_ref() == Some(&pane_id)
                    && (!core_start::is_core(s) || s.terminal_identity == info.identity)
            })
            .or(owned);
        // The agent just fetched is what the log is checked against; no second server call.
        let log = self
            .log_for(&srv, owned.clone(), info)
            .await
            .ok()
            .map(|f| f.log);
        Ok(DialogPane {
            io: ServerPane {
                key: (srv.socket().to_owned(), pane_id.clone()),
                logs: self.logs.clone(),
                server: srv,
                target: name.to_owned(),
                log,
                said: Default::default(),
            },
            pane_id,
            server,
            owned,
        })
    }

    /// Why keys may not go to a pane, asked again just before Enter.
    fn key_refusal(&self, owned: Option<&RunnerSession>) -> Option<&'static str> {
        let s = owned?;
        if s.agent == Agent::Claude {
            return Some("hooked");
        }
        let h = self.hooks.state(&s.session_id).unwrap_or_default();
        if h.hook_failed {
            Some("hook_failed")
        } else if h.pending > 0 || h.open_calls > 0 {
            Some("hook_pending")
        } else {
            None
        }
    }

    async fn dialog(&self, agent: &str) -> Result<Value, ProtoError> {
        let p = self.dialog_pane(agent).await?;
        let seen = dialogs::look(&p.io).await?;
        let refuse = || self.key_refusal(p.owned.as_ref());
        let view = dialogs::view(&seen, &refuse, &self.answered);
        Ok(
            json!({ "dialog": view, "occurrence": seen.occurrence, "blocked": seen.process.blocked,
                    "terminal_id": seen.process.terminal_id }),
        )
    }

    async fn answer(&self, agent: &str, req: &dialogs::AnswerRequest) -> Result<Value, ProtoError> {
        let p = self.dialog_pane(agent).await?;
        let _lock = self.pane_locks.lock(p.server.as_deref(), &p.pane_id).await;
        let refuse = || self.key_refusal(p.owned.as_ref());
        let result = dialogs::answer(&p.io, &refuse, &self.answered, req).await;
        let detail = json!({
            "agent": agent, "occurrence": req.occurrence, "option": req.option,
            "outcome": match &result { Ok(_) => "answered".to_owned(), Err(e) => code_name(e.code) },
        });
        tracing::info!(%detail, "dialog answer");
        if let Some(s) = &p.owned {
            self.audit(&s.session_id, "dialog_answer", detail);
        }
        // The dialog now showing goes back with the answer, so a group's next question needs no
        // second read.
        result.map(|after| {
            json!({ "answered": true, "dialog": dialogs::view(&after, &refuse, &self.answered) })
        })
    }

    async fn back(&self, agent: &str, occurrence: &str) -> Result<Value, ProtoError> {
        let p = self.dialog_pane(agent).await?;
        let _lock = self.pane_locks.lock(p.server.as_deref(), &p.pane_id).await;
        let refuse = || self.key_refusal(p.owned.as_ref());
        let result = dialogs::back(&p.io, &refuse, &self.answered, occurrence).await;
        let detail = json!({
            "agent": agent, "occurrence": occurrence,
            "outcome": match &result { Ok(_) => "moved".to_owned(), Err(e) => code_name(e.code) },
        });
        tracing::info!(%detail, "dialog back");
        if let Some(s) = &p.owned {
            self.audit(&s.session_id, "dialog_back", detail);
        }
        result.map(|after| {
            json!({ "moved": true, "dialog": dialogs::view(&after, &refuse, &self.answered) })
        })
    }

    /// Once `agent` has just become blocked: if it is still blocked by the same state after the
    /// hold, and no hook call is pending (its approval notifies instead), reports it waiting.
    pub(crate) fn hold_waiting(self: &Arc<Self>, agent: String) {
        let sessions = self.clone();
        tokio::spawn(async move {
            if let Err(e) = sessions.check_waiting(&agent).await {
                tracing::debug!(agent, error = %e, "no waiting check");
            }
        });
    }

    async fn check_waiting(&self, agent: &str) -> Result<(), ProtoError> {
        let p = self.dialog_pane(agent).await?;
        let before = dialogs::DialogIo::process(&p.io)
            .await
            .map_err(|e| err(ErrorCode::Herdr, e))?;
        tokio::time::sleep(WAITING_HOLD).await;
        let seen = dialogs::look(&p.io).await?;
        let held = seen.process.blocked
            && seen.process.state_change_seq == before.state_change_seq
            && seen.process.terminal_id == before.terminal_id;
        let hooked = p.owned.as_ref().is_some_and(|s| {
            let h = self.hooks.state(&s.session_id).unwrap_or_default();
            h.pending > 0 || h.open_calls > 0
        });
        if !held || hooked {
            return Ok(());
        }
        {
            let mut waited = self.waited.lock().unwrap();
            if waited.contains(&seen.occurrence) {
                return Ok(());
            }
            waited.push_back(seen.occurrence.clone());
            if waited.len() > KEEP_WAITED {
                waited.pop_front();
            }
        }
        self.emit(Event::Waiting {
            agent_name: agent.to_owned(),
            occurrence: seen.occurrence,
            terminal_id: seen.process.terminal_id.unwrap_or_default(),
            herdr_session: p.server,
            session_id: p.owned.map(|s| s.session_id),
        });
        Ok(())
    }

    fn session(&self, session_id: &str) -> Result<RunnerSession, ProtoError> {
        self.get(session_id)
            .ok_or_else(|| err(ErrorCode::NotFound, format!("no session {session_id}")))
    }

    async fn codex_version(&self) -> Result<String, ProtoError> {
        let (prog, args) = self
            .cfg
            .codex_version_cmd
            .split_first()
            .ok_or_else(|| err(ErrorCode::Internal, "no codex version command"))?;
        let out = tokio::time::timeout(
            Duration::from_secs(15),
            tokio::process::Command::new(prog).args(args).output(),
        )
        .await
        .map_err(|_| {
            err(
                ErrorCode::UnsupportedAgentVersion,
                "codex --version timed out",
            )
        })?
        .map_err(|e| {
            err(
                ErrorCode::UnsupportedAgentVersion,
                format!("running codex --version: {e}"),
            )
        })?;
        let text = String::from_utf8_lossy(&out.stdout);
        let version = codex::parse_version(&text).ok_or_else(|| {
            err(
                ErrorCode::UnsupportedAgentVersion,
                format!("unrecognised codex version output {:?}", text.trim()),
            )
        })?;
        Ok(version)
    }

    pub async fn start(&self, req: StartRequest) -> Result<Value, ProtoError> {
        check_session_id(&req.session_id)?;
        if let Some(existing) = self.get(&req.session_id) {
            return Ok(started_json(&existing));
        }
        if !req.repo_path.is_absolute() {
            return Err(err(ErrorCode::BadRequest, "repo_path must be absolute"));
        }
        // Refuse an unknown Codex before creating anything.
        let codex_version = match req.agent {
            Agent::Codex => Some(self.codex_version().await?),
            Agent::Claude => None,
        };
        let on_core = self.choose_core(req.backend).await?;

        let dir = self.dir(&req.session_id);
        create_private_dir(&dir)
            .map_err(|e| err(ErrorCode::Internal, format!("session dir: {e}")))?;
        let token = super::random_id();
        let secret_path = dir.join("secret.json");
        SecretFile {
            addr: self.cfg.hook_addr,
            session_id: req.session_id.clone(),
            token: token.clone(),
        }
        .write(&secret_path)
        .map_err(|e| err(ErrorCode::Internal, format!("secret file: {e}")))?;
        let hook_cmd = hook_command(&self.cfg.exe, req.agent, &secret_path);
        let args = match req.agent {
            Agent::Claude => {
                let settings = dir.join("claude-settings.json");
                let guard = !req.deny_patterns.is_empty();
                let mode = self.cfg.claude_mode;
                std::fs::write(
                    &settings,
                    claude::settings(&hook_cmd, mode, guard).to_string(),
                )
                .map_err(|e| err(ErrorCode::Internal, format!("claude settings: {e}")))?;
                if claude::pretrust(&self.cfg.claude_config, &req.repo_path).map_err(|e| {
                    err(
                        ErrorCode::Internal,
                        format!("pre-trusting repo for Claude: {e}"),
                    )
                })? {
                    self.audit(
                        &req.session_id,
                        "claude_pretrust",
                        json!({ "repo_root": req.repo_path }),
                    );
                }
                claude::launch_args(&settings, mode)
            }
            Agent::Codex => codex::launch_args(&req.repo_path, &hook_cmd),
        };
        self.hooks.register(
            &req.session_id,
            &token,
            req.agent,
            req.deny_patterns.clone(),
        );

        let short = &req.session_id[..req.session_id.len().min(4)];
        let herdr_base = format!("{}-{short}", req.project_id);
        let mut session = RunnerSession {
            session_id: req.session_id.clone(),
            agent: req.agent,
            project_id: req.project_id,
            repo_path: req.repo_path.clone(),
            branch: req.branch.clone(),
            deny_patterns: req.deny_patterns,
            agent_name: agent_name(&req.session_id),
            workspace_id: None,
            pane_id: None,
            worktree_path: None,
            codex_version,
            start_commit: None,
            herdr_session: None,
            launched: false,
            core_id: None,
            terminal_identity: None,
        };
        if let Some(core) = on_core {
            return self.start_on_core(session, core, args, req.prompt).await;
        }
        // Each task gets a Herdr session of its own, named after the project.
        let name = self.herdrs.free_name(&herdr_base, &req.session_id);
        match self.herdrs.create(&name).await {
            Ok(created) => session.herdr_session = created,
            Err(e) => {
                let e = err(ErrorCode::Herdr, e);
                self.abort(&session, &e).await;
                return Err(e);
            }
        }
        self.save(&session)
            .map_err(|e| err(ErrorCode::Internal, format!("saving session: {e}")))?;
        let herdr = self.herdr_for(&session);

        let created = match herdr
            .worktree_create(WorktreeCreateParams {
                cwd: Some(req.repo_path.to_string_lossy().into_owned()),
                branch: Some(req.branch.clone()),
                ..Default::default()
            })
            .await
        {
            Ok(created) => created,
            Err(e) => {
                let e = herdr_err(e);
                self.abort(&session, &e).await;
                return Err(e);
            }
        };
        session.workspace_id = Some(created.workspace.workspace_id.clone());
        session.pane_id = Some(created.root_pane.pane_id.clone());
        session.worktree_path = Some(created.worktree.path.clone());
        session.start_commit = head_commit(Path::new(&created.worktree.path)).await;
        self.save(&session)
            .map_err(|e| err(ErrorCode::Internal, format!("saving session: {e}")))?;

        if let Err(e) = self.launch(&session, args).await {
            self.abort(&session, &e).await;
            return Err(e);
        }
        session.launched = true;
        self.save(&session)
            .map_err(|e| err(ErrorCode::Internal, format!("saving session: {e}")))?;
        if let Err(e) = herdr
            .agent_prompt(AgentPromptParams {
                target: session.agent_name.clone(),
                text: req.prompt,
                wait: None,
            })
            .await
        {
            let e = herdr_err(e);
            // The hub marks the task failed, so its agent mustn't keep running out of sight. If the
            // prompt may have arrived, the task is left for the hub to reconcile.
            if e.code != ErrorCode::OutcomeUnknown {
                self.abort(&session, &e).await;
            }
            return Err(e);
        }
        self.audit(&session.session_id, "prompt", json!({ "initial": true }));
        Ok(started_json(&session))
    }

    /// Starts the agent and returns only once it is safe to send the task prompt.
    async fn launch(&self, s: &RunnerSession, args: Vec<String>) -> Result<(), ProtoError> {
        let herdr = self.herdr_for(s);
        let pane_id = s.pane_id.clone().expect("set after worktree creation");
        let params = AgentStartParams {
            name: s.agent_name.clone(),
            kind: s.agent.as_str().to_owned(),
            pane_id,
            args,
            timeout_ms: Some(AGENT_START_TIMEOUT_MS),
        };
        let mut attempt = 0;
        let started = loop {
            match herdr.agent_start(params.clone()).await {
                Err(e)
                    if e.server_code() == Some("agent_pane_busy")
                        && attempt < PANE_BUSY_ATTEMPTS =>
                {
                    attempt += 1;
                    tokio::time::sleep(PANE_BUSY_RETRY).await;
                }
                other => break other,
            }
        };
        match s.agent {
            Agent::Claude => {
                if let Err(e) = started {
                    let screen = visible(&herdr, &s.agent_name).await;
                    return Err(err(
                        ErrorCode::StartupFailed,
                        format!("{e}; screen:\n{}", excerpt(&screen)),
                    ));
                }
                let deadline = Instant::now() + self.cfg.liveness_timeout;
                while !self.hooks.state(&s.session_id).is_some_and(|h| h.live) {
                    if Instant::now() >= deadline {
                        return Err(err(
                            ErrorCode::HooksUntrusted,
                            "the SessionStart hook never reached the runner",
                        ));
                    }
                    tokio::time::sleep(Duration::from_millis(200)).await;
                }
                self.wait_interactive(s).await
            }
            Agent::Codex => {
                // Herdr reports the "Folder access" screen as idle, so the screen decides, not the status.
                if let Err(e) = started
                    && e.server_code() != Some("agent_not_ready")
                {
                    return Err(herdr_err(e));
                }
                let pane = CodexPane::new(&herdr, s.agent, &s.agent_name).expect("codex session");
                let pane_id = s.pane_id.as_deref().unwrap_or_default();
                let _lock = self
                    .pane_locks
                    .lock(s.herdr_session.as_deref(), pane_id)
                    .await;
                clear_startup(&pane, self.cfg.startup_timeout)
                    .await
                    .map_err(|f| {
                        self.audit(
                            &s.session_id,
                            "startup_failed",
                            json!({ "reason": f.reason, "screen_excerpt": f.screen_excerpt }),
                        );
                        err(
                            ErrorCode::StartupFailed,
                            format!("{}; screen:\n{}", f.reason, f.screen_excerpt),
                        )
                    })?;
                self.audit(
                    &s.session_id,
                    "startup_cleared",
                    json!({ "screen": "folder_access" }),
                );
                self.wait_interactive(s).await
            }
        }
    }

    /// Herdr refuses prompts until it has finished the launch, which can lag the agent's own UI.
    async fn wait_interactive(&self, s: &RunnerSession) -> Result<(), ProtoError> {
        let deadline = Instant::now() + self.cfg.startup_timeout;
        loop {
            let ready = self
                .server_for(s)
                .agents()
                .await
                .map_err(backend_err)?
                .iter()
                .find(|a| Some(&a.target) == s.pane_id.as_ref())
                .is_some_and(|a| a.ready);
            if ready {
                return Ok(());
            }
            if Instant::now() >= deadline {
                return Err(err(
                    ErrorCode::StartupFailed,
                    "Herdr never marked the agent ready for prompts",
                ));
            }
            tokio::time::sleep(Duration::from_millis(500)).await;
        }
    }

    /// Closes the workspace of a failed start; the worktree and branch are kept.
    async fn abort(&self, s: &RunnerSession, e: &ProtoError) {
        self.hooks.unregister(&s.session_id);
        if core_start::is_core(s) {
            self.end_core_terminal(s).await;
        } else if let Some(ws) = &s.workspace_id
            && let Err(close) = self.herdr_for(s).workspace_close(ws).await
        {
            tracing::warn!(error = %close, "closing workspace of failed start");
        }
        if let Some(name) = &s.herdr_session
            && !core_start::is_core(s)
            && let Err(stop) = self.herdrs.stop(name).await
        {
            tracing::warn!(error = %stop, "stopping the Herdr session of a failed start");
        }
        self.audit(
            &s.session_id,
            "start_failed",
            json!({ "code": e.code, "message": e.message }),
        );
        self.forget(&s.session_id);
    }

    fn forget(&self, session_id: &str) {
        self.sessions.lock().unwrap().remove(session_id);
        let _ = std::fs::remove_dir_all(self.dir(session_id));
    }

    async fn prompt(&self, session_id: &str, text: &str) -> Result<Value, ProtoError> {
        let s = self.session(session_id)?;
        let _lock = match &s.pane_id {
            Some(pane) => Some(self.pane_locks.lock(s.herdr_session.as_deref(), pane).await),
            None => None,
        };
        let status = self
            .server_for(&s)
            .prompt(Self::target_for(&s), text)
            .await
            .map_err(backend_err)?;
        self.audit(
            session_id,
            "prompt",
            json!({ "chars": text.chars().count() }),
        );
        Ok(json!({ "agent_status": status }))
    }

    async fn read(&self, session_id: &str, lines: u32) -> Result<Value, ProtoError> {
        let s = self.session(session_id)?;
        let lines = if lines == 0 {
            DEFAULT_READ_LINES
        } else {
            lines
        };
        let read = self
            .server_for(&s)
            .read(Self::target_for(&s), Read::Recent(lines))
            .await
            .map_err(backend_err)?;
        Ok(
            json!({ "text": read.text, "revision": read.revision, "truncated": read.truncated, "untrusted": true }),
        )
    }

    fn decide(
        &self,
        session_id: &str,
        approval_id: &str,
        allow: bool,
        choices: &[String],
    ) -> Result<Value, ProtoError> {
        self.hooks
            .decide(session_id, approval_id, allow, choices)
            .map_err(|e| match e {
                DecideError::BadChoices(why) => err(ErrorCode::BadRequest, why),
                DecideError::UnknownSession => {
                    err(ErrorCode::NotFound, format!("no session {session_id}"))
                }
                DecideError::NotPending => err(
                    ErrorCode::NotFound,
                    format!("approval {approval_id} is not pending"),
                ),
            })?;
        self.audit(
            session_id,
            "decision",
            json!({ "approval_id": approval_id, "allow": allow }),
        );
        Ok(json!({ "applied": true }))
    }

    async fn stop(&self, session_id: &str, remove_if_clean: bool) -> Result<Value, ProtoError> {
        let s = self.session(session_id)?;
        let herdr = self.herdr_for(&s);
        self.hooks.unregister(session_id);
        let mut removed = false;
        let mut kept_because = None;
        if core_start::is_core(&s) {
            (removed, kept_because) = self.stop_core(&s, remove_if_clean).await;
        } else if let (Some(ws), Some(path)) = (&s.workspace_id, &s.worktree_path) {
            if remove_if_clean {
                match worktree_clean_and_merged(Path::new(path), &s.repo_path, &s.branch).await {
                    // A failed removal keeps the worktree; the stop itself still completes.
                    Ok(()) => match herdr.worktree_remove(ws).await {
                        Ok(_) => removed = true,
                        Err(e) => kept_because = Some(format!("removing it failed: {e}")),
                    },
                    Err(why) => kept_because = Some(why),
                }
            }
            if !removed && let Err(e) = herdr.workspace_close(ws).await {
                tracing::warn!(error = %e, "closing workspace on stop");
            }
        }
        // The task's own Herdr session stops too; it stays listed, with its worktree kept.
        if let Some(name) = &s.herdr_session
            && !core_start::is_core(&s)
            && let Err(e) = self.herdrs.stop(name).await
        {
            tracing::warn!(error = %e, herdr_session = name, "stopping the task's Herdr session");
        }
        self.audit(
            session_id,
            "stop",
            json!({ "worktree_removed": removed, "kept_because": kept_because }),
        );
        self.forget(session_id);
        self.emit(Event::SessionExited {
            session_id: session_id.to_owned(),
        });
        Ok(json!({ "worktree_removed": removed, "kept_because": kept_because, "branch": s.branch }))
    }

    /// Stops the session if it's still here, removes its worktree only if clean and merged, then
    /// deletes its Herdr session. Each step accepts having been done already, so a retry is safe.
    async fn delete(
        &self,
        session_id: &str,
        herdr_session: Option<String>,
        repo_path: Option<String>,
        worktree_path: Option<String>,
        branch: Option<String>,
    ) -> Result<Value, ProtoError> {
        let (removed, kept_because, herdr_session) = match self.get(session_id) {
            Some(s) => {
                let v = self.stop(session_id, true).await?;
                (
                    v["worktree_removed"].as_bool().unwrap_or(false),
                    v["kept_because"].as_str().map(str::to_owned),
                    s.herdr_session.or(herdr_session),
                )
            }
            None => {
                let (removed, kept) = match (&repo_path, &worktree_path, &branch) {
                    (Some(repo), Some(path), Some(branch)) => {
                        remove_stopped_worktree(Path::new(repo), Path::new(path), branch).await
                    }
                    _ => (false, None),
                };
                (removed, kept, herdr_session)
            }
        };
        let herdr_session_deleted = match &herdr_session {
            // A session in the Herdr app has no server of its own.
            Some(name) if name == core_watch::SERVER => false,
            Some(name) => {
                self.herdrs.delete(name).await.map_err(|e| {
                    err(
                        ErrorCode::Herdr,
                        format!("deleting Herdr session {name}: {e}"),
                    )
                })?;
                true
            }
            None => false,
        };
        self.audit(
            session_id,
            "delete",
            json!({ "worktree_removed": removed, "kept_because": kept_because, "herdr_session": herdr_session }),
        );
        Ok(json!({
            "worktree_removed": removed,
            "kept_because": kept_because,
            "herdr_session_deleted": herdr_session_deleted,
        }))
    }

    pub async fn owned(&self) -> Vec<OwnedSession> {
        let mut snaps = HashMap::new();
        for s in self.all() {
            if !snaps.contains_key(&s.herdr_session) {
                let facts = self.server_for(&s).agents().await.ok();
                snaps.insert(s.herdr_session.clone(), facts);
            }
        }
        self.all()
            .into_iter()
            .filter_map(|s| {
                let status = snaps
                    .get(&s.herdr_session)
                    .and_then(Option::as_ref)
                    .and_then(|facts| facts.iter().find(|a| Some(&a.target) == s.pane_id.as_ref()))
                    .map_or(AgentStatus::Unknown, |a| a.status);
                let hook_failed = self
                    .hooks
                    .state(&s.session_id)
                    .is_some_and(|h| h.hook_failed);
                Some(OwnedSession {
                    hook_failed,
                    session_id: s.session_id,
                    workspace_id: s.workspace_id?,
                    pane_id: s.pane_id,
                    agent_status: serde_json::to_value(status).ok()?.as_str()?.to_owned(),
                })
            })
            .collect()
    }
}

struct DialogPane {
    io: ServerPane,
    pane_id: String,
    server: Option<String>,
    owned: Option<RunnerSession>,
}

fn code_name(code: ErrorCode) -> String {
    serde_json::to_value(code)
        .ok()
        .and_then(|v| v.as_str().map(str::to_owned))
        .unwrap_or_else(|| "unknown".into())
}

#[derive(Debug, Clone)]
pub struct StartRequest {
    pub session_id: String,
    pub project_id: String,
    pub repo_path: PathBuf,
    pub branch: String,
    pub agent: Agent,
    pub prompt: String,
    pub deny_patterns: Vec<String>,
    /// Overrides `new_sessions` for this start.
    pub backend: Option<crate::proto::StartBackend>,
}

fn started_json(s: &RunnerSession) -> Value {
    json!({
        "session_id": s.session_id,
        "branch": s.branch,
        "worktree_path": s.worktree_path,
        "workspace_id": s.workspace_id,
        "pane_id": s.pane_id,
        "agent_name": s.agent_name,
        "codex_version": s.codex_version,
        "start_commit": s.start_commit,
        "herdr_session": s.herdr_session,
    })
}

async fn visible(herdr: &Client, target: &str) -> String {
    herdr
        .agent_read(target, ReadSource::Visible, None)
        .await
        .map(|r| r.text)
        .unwrap_or_default()
}

fn create_private_dir(dir: &Path) -> std::io::Result<()> {
    use std::os::unix::fs::{DirBuilderExt, PermissionsExt};
    std::fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(dir)?;
    std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700))
}

/// `Ok` only for a worktree with no changes whose branch is already in the main checkout's HEAD.
pub async fn worktree_clean_and_merged(
    worktree: &Path,
    repo: &Path,
    branch: &str,
) -> Result<(), String> {
    let status = git(worktree, &["status", "--porcelain"]).await?;
    if !status.stdout.is_empty() {
        return Err("dirty".into());
    }
    let merged = git(repo, &["merge-base", "--is-ancestor", branch, "HEAD"]).await?;
    if !merged.status.success() {
        return Err("unmerged".into());
    }
    Ok(())
}

/// A stopped session's worktree, removed as Herdr removes one: only a linked worktree of `repo`,
/// only if clean and merged, never forced. The branch stays. Already gone counts as not removed.
async fn remove_stopped_worktree(repo: &Path, path: &Path, branch: &str) -> (bool, Option<String>) {
    if !path.exists() {
        return (false, None);
    }
    let canon = |p: &Path| std::fs::canonicalize(p).unwrap_or_else(|_| p.to_owned());
    let target = canon(path);
    if target == canon(repo) {
        return (false, Some("it is the repository's main checkout".into()));
    }
    let listed = match git(repo, &["worktree", "list", "--porcelain"]).await {
        Ok(o) if o.status.success() => String::from_utf8_lossy(&o.stdout).into_owned(),
        Ok(o) => {
            let why = String::from_utf8_lossy(&o.stderr).trim().to_owned();
            return (false, Some(format!("git worktree list: {why}")));
        }
        Err(e) => return (false, Some(e)),
    };
    let registered = listed
        .lines()
        .filter_map(|l| l.strip_prefix("worktree "))
        .any(|w| canon(Path::new(w)) == target);
    if !registered {
        return (
            false,
            Some("it is not a worktree of the project's repository".into()),
        );
    }
    if let Err(why) = worktree_clean_and_merged(path, repo, branch).await {
        return (false, Some(why));
    }
    match git(repo, &["worktree", "remove", &target.to_string_lossy()]).await {
        Ok(o) if o.status.success() => (true, None),
        Ok(o) => {
            let why = String::from_utf8_lossy(&o.stderr).trim().to_owned();
            (false, Some(format!("removing it failed: {why}")))
        }
        Err(e) => (false, Some(format!("removing it failed: {e}"))),
    }
}

/// Files changed since `base`, committed or not, with line counts; new untracked files have none.
async fn changes(worktree: &Path, base: &str) -> Result<Value, String> {
    let diff = git(worktree, &["diff", "--numstat", "--no-renames", base, "--"]).await?;
    if !diff.status.success() {
        return Err(format!(
            "git diff: {}",
            String::from_utf8_lossy(&diff.stderr).trim()
        ));
    }
    let mut files: Vec<Value> = String::from_utf8_lossy(&diff.stdout)
        .lines()
        .filter_map(|l| {
            let mut f = l.splitn(3, '\t');
            let (added, deleted, path) = (f.next()?, f.next()?, f.next()?);
            let mut f = json!({ "path": path, "added": added.parse::<u64>().ok(), "deleted": deleted.parse::<u64>().ok() });
            if added == "-" {
                f["binary"] = json!(true);
                f["size"] = json!(file_size(worktree, path));
            }
            Some(f)
        })
        .collect();
    let new = git(worktree, &["ls-files", "--others", "--exclude-standard"]).await?;
    files.extend(String::from_utf8_lossy(&new.stdout).lines().map(|path| {
        let mut f = json!({ "path": path, "added": null, "deleted": null });
        if super::diff::looks_binary(&worktree.join(path)) {
            f["binary"] = json!(true);
            f["size"] = json!(file_size(worktree, path));
        }
        f
    }));
    let truncated = files.len() > MAX_CHANGED_FILES;
    files.truncate(MAX_CHANGED_FILES);
    Ok(json!({ "files": files, "truncated": truncated }))
}

/// A changed file's size in the worktree; none once it's deleted.
fn file_size(worktree: &Path, path: &str) -> Option<u64> {
    std::fs::symlink_metadata(worktree.join(path))
        .ok()
        .map(|m| m.len())
}

/// The commit a fresh worktree is on; `None` when git can't say.
async fn head_commit(worktree: &Path) -> Option<String> {
    let out = git(worktree, &["rev-parse", "--verify", "HEAD"])
        .await
        .ok()?;
    let sha = String::from_utf8(out.stdout).ok()?.trim().to_owned();
    let hex = sha.len() >= 40 && sha.bytes().all(|b| b.is_ascii_hexdigit());
    (out.status.success() && hex).then_some(sha)
}

async fn git(dir: &Path, args: &[&str]) -> Result<std::process::Output, String> {
    tokio::process::Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .output()
        .await
        .map_err(|e| format!("git: {e}"))
}

/// The git checkout `dir` is in, else `dir` itself.
fn repo_root(dir: &Path) -> PathBuf {
    dir.ancestors()
        .find(|d| d.join(".git").exists())
        .unwrap_or(dir)
        .to_owned()
}

#[path = "core_sessions.rs"]
mod core_start;

#[cfg(test)]
#[path = "sessions_tests.rs"]
mod tests;
