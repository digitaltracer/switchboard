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
use super::codex::{CodexPane, clear_startup, excerpt};
use super::herdrs::{Herdrs, qualified, split_qualified};
use super::hooks::{DecideError, HookHub};
use super::transcript;
use crate::agents::{claude, codex, hook_command};
use crate::cli::Agent;
use crate::herdr::types::{
    AgentPromptParams, AgentStartParams, AgentStatus, ReadSource, WorktreeCreateParams,
};
use crate::herdr::{Client, HerdrError};
use crate::hook::wire::SecretFile;
use crate::proto::{ErrorCode, Event, Op, OwnedSession, ProtoError};

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
}

fn err(code: ErrorCode, message: impl Into<String>) -> ProtoError {
    ProtoError::new(code, message)
}

fn herdr_err(e: HerdrError) -> ProtoError {
    let code = if e.outcome_unknown() {
        ErrorCode::OutcomeUnknown
    } else if e.server_code() == Some("agent_blocked") {
        ErrorCode::AgentBlocked
    } else if e.server_code() == Some("agent_not_found") {
        ErrorCode::NotFound
    } else if matches!(e, HerdrError::InvalidArgument(_)) {
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
            cfg,
            sessions: Mutex::default(),
        })
    }

    /// The Herdr server a task runs in.
    pub(crate) fn herdr_for(&self, s: &RunnerSession) -> Client {
        self.herdrs.client(s.herdr_session.as_deref())
    }

    /// Where a named agent lives: `<session>/<agent>`, a task's own agent, or the legacy server.
    fn resolve_agent<'a>(&self, name: &'a str) -> (Client, &'a str, Option<RunnerSession>) {
        let (session, agent) = split_qualified(name);
        let owned = self.all().into_iter().find(|s| {
            s.agent_name == agent && (session.is_none() || s.herdr_session.as_deref() == session)
        });
        match (session, &owned) {
            (Some(s), _) => (self.herdrs.client(Some(s)), agent, owned),
            (None, Some(o)) => (self.herdr_for(o), agent, owned.clone()),
            (None, None) => (self.herdrs.client(None), agent, None),
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
        for (server, client) in self.herdrs.running().await {
            let Ok(list) = agents::list(&client, &names).await else {
                continue;
            };
            for mut a in list {
                if a.session_id.is_none() {
                    a.name = qualified(server.as_deref(), &a.name);
                }
                a.herdr_session = server.clone();
                out.push(a);
            }
        }
        Ok(out)
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
            } => {
                self.start(StartRequest {
                    session_id,
                    project_id,
                    repo_path: repo_path.into(),
                    branch,
                    agent,
                    prompt,
                    deny_patterns,
                })
                .await
            }
            Op::Prompt { session_id, text } => self.prompt(&session_id, &text).await,
            Op::Read { session_id, lines } => self.read(&session_id, lines).await,
            Op::Decide {
                session_id,
                approval_id,
                allow,
            } => self.decide(&session_id, &approval_id, allow),
            Op::Answer { .. } => Err(err(
                ErrorCode::BadRequest,
                "screen dialog answers are not supported yet",
            )),
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
                let (herdr, agent, _) = self.resolve_agent(&agent);
                let text = agents::read(&herdr, agent, lines, visible)
                    .await
                    .map_err(herdr_err)?;
                let mut out = json!({ "text": text, "untrusted": true });
                if visible
                    && ansi
                    && let Some(screen) = agents::screen_ansi(&herdr, agent).await
                {
                    out["ansi"] = Value::String(screen);
                }
                Ok(out)
            }
            Op::PromptAgent { agent, text } => {
                let (herdr, name, owned) = self.resolve_agent(&agent);
                // A session's prompts go through its own op, which the hub records and audits.
                if owned.is_some() {
                    return Err(err(
                        ErrorCode::BadRequest,
                        format!("{agent} is a Switchboard session; prompt the session"),
                    ));
                }
                let outcome = agents::prompt(&herdr, name, &text)
                    .await
                    .map_err(herdr_err)?;
                Ok(serde_json::to_value(outcome).expect("serializes"))
            }
            Op::Transcript {
                agent,
                after,
                before,
            } => self.transcript(&agent, after, before).await,
            Op::Changes { session_id } => {
                let s = self.session(&session_id)?;
                match (&s.worktree_path, &s.start_commit) {
                    (Some(wt), Some(base)) => changes(Path::new(wt), base)
                        .await
                        .map_err(|e| err(ErrorCode::Internal, e)),
                    _ => Ok(json!({ "files": [], "truncated": false })),
                }
            }
        }
    }

    /// A page of an agent's conversation from its own log: a Claude agent's found through the
    /// Claude running in its pane, else the session id Herdr reports; a Codex agent's through its
    /// folder.
    async fn transcript(
        &self,
        agent: &str,
        after: Option<u64>,
        before: Option<u64>,
    ) -> Result<Value, ProtoError> {
        let (herdr, name, owned) = self.resolve_agent(agent);
        let snap = herdr.session_snapshot().await.map_err(herdr_err)?;
        let info = snap
            .agents
            .into_iter()
            .find(|a| a.name.as_deref() == Some(name) || a.pane_id == name)
            .ok_or_else(|| err(ErrorCode::NotFound, format!("no agent {agent:?}")))?;
        let none = || {
            err(
                ErrorCode::NotFound,
                "no conversation log for this agent yet",
            )
        };
        let find: Box<dyn FnOnce() -> Option<PathBuf> + Send> = match info.agent.as_deref() {
            Some("claude") => {
                let pids = herdr
                    .pane_foreground_pids(&info.pane_id)
                    .await
                    .unwrap_or_default();
                let reported = info.agent_session.map(|s| s.value);
                let projects = self.cfg.claude_projects.clone();
                Box::new(move || {
                    let live = projects
                        .parent()
                        .and_then(|dir| transcript::live_claude_session(dir, &pids));
                    transcript::find_log(&projects, &live.or(reported)?)
                })
            }
            Some("codex") => {
                // A session's own worktree, else wherever Herdr says the agent runs.
                let cwd = owned
                    .and_then(|s| s.worktree_path)
                    .or(info.cwd)
                    .ok_or_else(none)?;
                let sessions = self.cfg.codex_sessions.clone();
                Box::new(move || transcript::find_codex_log(&sessions, Path::new(&cwd)))
            }
            _ => {
                return Err(err(
                    ErrorCode::BadRequest,
                    "only Claude and Codex agents have a conversation to read",
                ));
            }
        };
        let page = tokio::task::spawn_blocking(move || {
            let path = find().ok_or_else(none)?;
            transcript::read_page(&path, after, before).map_err(|e| {
                err(
                    ErrorCode::Internal,
                    format!("reading the conversation: {e}"),
                )
            })
        })
        .await
        .map_err(|e| err(ErrorCode::Internal, e.to_string()))??;
        Ok(serde_json::to_value(page).expect("serializes"))
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
        codex::check_version(&version)
            .map_err(|e| err(ErrorCode::UnsupportedAgentVersion, e.to_string()))?;
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
        };
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
                let version = s.codex_version.as_deref().unwrap_or_default();
                clear_startup(&pane, version, self.cfg.startup_timeout)
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
            let snap = self
                .herdr_for(s)
                .session_snapshot()
                .await
                .map_err(herdr_err)?;
            let ready = snap
                .agents
                .iter()
                .find(|a| Some(&a.pane_id) == s.pane_id.as_ref())
                .is_some_and(|a| a.interactive_ready && !a.launch_pending);
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
        if let Some(ws) = &s.workspace_id
            && let Err(close) = self.herdr_for(s).workspace_close(ws).await
        {
            tracing::warn!(error = %close, "closing workspace of failed start");
        }
        if let Some(name) = &s.herdr_session
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
        let agent = self
            .herdr_for(&s)
            .agent_prompt(AgentPromptParams {
                target: s.agent_name.clone(),
                text: text.to_owned(),
                wait: None,
            })
            .await
            .map_err(herdr_err)?;
        self.audit(
            session_id,
            "prompt",
            json!({ "chars": text.chars().count() }),
        );
        Ok(json!({ "agent_status": agent.agent_status }))
    }

    async fn read(&self, session_id: &str, lines: u32) -> Result<Value, ProtoError> {
        let s = self.session(session_id)?;
        let lines = Some(if lines == 0 {
            DEFAULT_READ_LINES
        } else {
            lines
        });
        let herdr = self.herdr_for(&s);
        // Herdr refuses history reads while an agent sits at a dialog.
        let read = match herdr
            .agent_read(&s.agent_name, ReadSource::RecentUnwrapped, lines)
            .await
        {
            Err(e) if e.server_code() == Some("agent_not_idle") => {
                herdr
                    .agent_read(&s.agent_name, ReadSource::Visible, None)
                    .await
            }
            other => other,
        }
        .map_err(herdr_err)?;
        Ok(
            json!({ "text": read.text, "revision": read.revision, "truncated": read.truncated, "untrusted": true }),
        )
    }

    fn decide(
        &self,
        session_id: &str,
        approval_id: &str,
        allow: bool,
    ) -> Result<Value, ProtoError> {
        self.hooks
            .decide(session_id, approval_id, allow)
            .map_err(|e| match e {
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
        if let (Some(ws), Some(path)) = (&s.workspace_id, &s.worktree_path) {
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
                let snap = self.herdr_for(&s).session_snapshot().await.ok();
                snaps.insert(s.herdr_session.clone(), snap);
            }
        }
        self.all()
            .into_iter()
            .filter_map(|s| {
                let status = snaps
                    .get(&s.herdr_session)
                    .and_then(Option::as_ref)
                    .and_then(|snap| {
                        snap.agents
                            .iter()
                            .find(|a| Some(&a.pane_id) == s.pane_id.as_ref())
                    })
                    .map_or(AgentStatus::Unknown, |a| a.agent_status);
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

#[derive(Debug, Clone)]
pub struct StartRequest {
    pub session_id: String,
    pub project_id: String,
    pub repo_path: PathBuf,
    pub branch: String,
    pub agent: Agent,
    pub prompt: String,
    pub deny_patterns: Vec<String>,
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
            Some(json!({ "path": path, "added": added.parse::<u64>().ok(), "deleted": deleted.parse::<u64>().ok() }))
        })
        .collect();
    let new = git(worktree, &["ls-files", "--others", "--exclude-standard"]).await?;
    files.extend(
        String::from_utf8_lossy(&new.stdout)
            .lines()
            .map(|path| json!({ "path": path, "added": null, "deleted": null })),
    );
    let truncated = files.len() > MAX_CHANGED_FILES;
    files.truncate(MAX_CHANGED_FILES);
    Ok(json!({ "files": files, "truncated": truncated }))
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

#[cfg(test)]
#[path = "sessions_tests.rs"]
mod tests;
