//! The session service: lifecycle, routing, idempotency, reconciliation, approvals and the
//! notification mute state. Step 7's MCP tools and the WhatsApp bridge call this typed API.

use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use tokio::sync::{mpsc, watch};
use tokio::task::JoinHandle;
use tokio::time::Instant;

use super::runners::{Readiness, RunnerInfo, Runners};
use crate::cli::Agent;
use crate::config::{HubConfig, ProjectConfig};
use crate::proto::{ErrorCode, Event, Frame, Hello, Op, OwnedSession, ProtoError};
use crate::redact::redact;
use crate::runner::random_id;
use crate::runner::sessions::agent_name;
use crate::store::{
    ApprovalRecord, Begin, DecideOutcome, Decision, Delivery, Lifecycle, NewOperation, OpState,
    Operation, SessionPatch, SessionRecord, Store, StoreError,
};
use crate::whatsapp::watchlist::WatchState;

pub const DEFAULT_MACHINE: &str = "vps";
/// Longest a runner start can take: worktree, agent launch, liveness, readiness and the prompt.
pub const START_TIMEOUT: Duration = Duration::from_secs(240);
pub const CALL_TIMEOUT: Duration = Duration::from_secs(30);
pub const STOP_TIMEOUT: Duration = Duration::from_secs(90);
pub const DEDUPE_WINDOW: Duration = Duration::from_secs(60);
/// The hub expires an approval this long before the runner would, so a decision never races it.
pub const DEADLINE_MARGIN: Duration = Duration::from_secs(5);
pub const MAX_WAIT: Duration = Duration::from_secs(25);
pub const MAX_READ_LINES: u32 = 500;
pub const SWEEP_INTERVAL: Duration = Duration::from_secs(1);
const RECHECK_RETRY: Duration = Duration::from_secs(10);

const KIND_START: &str = "session_start";
const KIND_PROMPT: &str = "session_prompt";
const KIND_DECIDE: &str = "session_decide";
const KIND_STOP: &str = "session_stop";
const KIND_ANSWER: &str = "session_answer";

/// Unix milliseconds; manual in tests so deadlines are exact.
#[derive(Clone)]
pub enum Clock {
    System,
    Manual(Arc<AtomicI64>),
}

impl Clock {
    pub fn manual(start_ms: i64) -> Self {
        Clock::Manual(Arc::new(AtomicI64::new(start_ms)))
    }

    pub fn now_ms(&self) -> i64 {
        match self {
            Clock::System => SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map_or(0, |d| d.as_millis() as i64),
            Clock::Manual(ms) => ms.load(Ordering::SeqCst),
        }
    }

    pub fn advance(&self, by: Duration) {
        if let Clock::Manual(ms) = self {
            ms.fetch_add(by.as_millis() as i64, Ordering::SeqCst);
        }
    }
}

/// Who is calling, for the audit log: `mcp:<client>`, `whatsapp`, `admin`.
#[derive(Debug, Clone)]
pub struct Caller {
    pub actor: String,
}

impl Caller {
    pub fn new(actor: impl Into<String>) -> Self {
        Self {
            actor: actor.into(),
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum HubError {
    #[error("unknown project {0:?}")]
    UnknownProject(String),
    #[error("unknown machine {0:?}")]
    UnknownMachine(String),
    #[error("project {project:?} has no repo on machine {machine:?}")]
    ProjectNotOnMachine { project: String, machine: String },
    #[error("machine {0:?} is offline")]
    MachineOffline(String),
    #[error("machine {0:?} is reconciling after a reconnect; retry shortly")]
    Reconciling(String),
    #[error("unknown session {0:?}")]
    UnknownSession(String),
    #[error("unknown approval {0:?} for this session")]
    UnknownApproval(String),
    #[error("session {id:?} is {lifecycle:?}")]
    NotRunning { id: String, lifecycle: Lifecycle },
    #[error("bad request: {0}")]
    BadRequest(String),
    #[error("request id {0:?} was already used for a different call")]
    RequestIdReused(String),
    /// The change may or may not have happened; the session reconciles when its runner is back.
    #[error("outcome unknown for operation {operation_id:?}")]
    OutcomeUnknown {
        operation_id: String,
        session_id: Option<String>,
    },
    #[error("runner: {0}")]
    Runner(ProtoError),
    /// A retried request id whose recorded outcome was a failure.
    #[error("{code}: {message}")]
    Recorded { code: String, message: String },
    #[error("store: {0}")]
    Store(StoreError),
}

impl HubError {
    pub fn code(&self) -> String {
        match self {
            HubError::UnknownProject(_) => "unknown_project".into(),
            HubError::UnknownMachine(_) => "unknown_machine".into(),
            HubError::ProjectNotOnMachine { .. } => "project_not_on_machine".into(),
            HubError::MachineOffline(_) => "machine_offline".into(),
            HubError::Reconciling(_) => "reconciling".into(),
            HubError::UnknownSession(_) => "unknown_session".into(),
            HubError::UnknownApproval(_) => "unknown_approval".into(),
            HubError::NotRunning { .. } => "session_not_running".into(),
            HubError::BadRequest(_) => "bad_request".into(),
            HubError::RequestIdReused(_) => "request_id_reused".into(),
            HubError::OutcomeUnknown { .. } => "outcome_unknown".into(),
            HubError::Runner(e) => code_str(e.code),
            HubError::Recorded { code, .. } => code.clone(),
            HubError::Store(_) => "internal".into(),
        }
    }
}

impl From<StoreError> for HubError {
    fn from(e: StoreError) -> Self {
        match e {
            StoreError::RequestIdReused(id) => HubError::RequestIdReused(id),
            other => HubError::Store(other),
        }
    }
}

fn code_str(code: ErrorCode) -> String {
    serde_json::to_value(code)
        .ok()
        .and_then(|v| v.as_str().map(str::to_owned))
        .unwrap_or_else(|| "unknown".into())
}

fn error_json(code: &str, message: &str) -> Value {
    json!({ "error": { "code": code, "message": message } })
}

fn sha256_hex(s: &str) -> String {
    hex::encode(Sha256::digest(s.as_bytes()))
}

fn fingerprint(args: &Value) -> String {
    sha256_hex(&args.to_string())
}

#[derive(Debug, Clone)]
pub struct StartRequest {
    /// Exact project id or alias from the allowlist.
    pub project_id: String,
    pub prompt: String,
    /// Defaults to `vps`. Never rerouted.
    pub machine_id: Option<String>,
    /// Defaults to the project's agent.
    pub agent: Option<Agent>,
    pub notify: bool,
    pub request_id: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StartOutcome {
    Started,
    /// Same project, machine, agent and prompt within the dedupe window.
    Deduplicated,
    /// A retried request id.
    Replayed,
    /// The runner's reply was lost; its own list of sessions showed the start happened.
    Reconciled,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct StartResult {
    pub session_id: String,
    pub project_id: String,
    pub machine_id: String,
    pub agent: String,
    pub agent_name: Option<String>,
    pub branch: Option<String>,
    pub muted: bool,
    pub outcome: StartOutcome,
    pub operation_id: String,
}

impl StartResult {
    fn of(s: &SessionRecord, outcome: StartOutcome, operation_id: &str) -> Self {
        Self {
            session_id: s.id.clone(),
            project_id: s.project_id.clone(),
            machine_id: s.machine_id.clone(),
            agent: s.agent.clone(),
            agent_name: s.agent_name.clone(),
            branch: s.branch.clone(),
            muted: s.muted,
            outcome,
            operation_id: operation_id.to_owned(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ProjectView {
    pub id: String,
    pub aliases: Vec<String>,
    pub description: String,
    pub default_agent: Agent,
    pub machines: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct MachineView {
    pub id: String,
    pub connected: bool,
    pub readiness: Readiness,
    pub runner: Option<RunnerInfo>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ApprovalView {
    pub id: String,
    pub tool_name: String,
    pub tool_input: Value,
    pub created_at: i64,
    pub deadline: i64,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct SessionView {
    pub id: String,
    pub project_id: String,
    pub machine_id: String,
    pub machine: Readiness,
    pub agent: String,
    pub agent_name: Option<String>,
    pub branch: Option<String>,
    pub worktree_path: Option<String>,
    pub lifecycle: Lifecycle,
    pub agent_status: String,
    pub hook_failed: bool,
    pub error: Option<String>,
    pub revision: i64,
    pub created_at: i64,
    pub updated_at: i64,
    pub muted: bool,
    /// False when muted, or when mute-all is on.
    pub notifies: bool,
    pub pending_approvals: Vec<ApprovalView>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PromptResult {
    pub session_id: String,
    pub agent_status: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DecideResult {
    pub session_id: String,
    pub approval_id: String,
    /// The recorded outcome: an allow that never reached the runner is a deny.
    pub outcome: Decision,
    pub reason: Option<String>,
    /// `unknown` with an allow means the tool may have run.
    pub delivery: Option<Delivery>,
    /// The approval was already decided; nothing was sent again.
    pub replayed: bool,
}

impl DecideResult {
    fn of(a: &ApprovalRecord, replayed: bool) -> Self {
        Self {
            session_id: a.session_id.clone(),
            approval_id: a.id.clone(),
            outcome: a.outcome.unwrap_or(Decision::Deny),
            reason: a.reason.clone(),
            delivery: a.delivery,
            replayed,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct StopResult {
    pub session_id: String,
    pub worktree_removed: bool,
    pub kept_because: Option<String>,
    pub branch: Option<String>,
    pub already_stopped: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ReadResult {
    pub text: String,
    pub revision: Option<u64>,
    pub truncated: bool,
    /// Terminal output is the agent's, not Switchboard's.
    pub untrusted: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct NotifyResult {
    pub session_id: String,
    pub agent_name: Option<String>,
    pub muted: bool,
    pub mute_all: bool,
    pub notifies: bool,
}

pub struct SessionService {
    cfg: Arc<HubConfig>,
    store: Store,
    pub(crate) runners: Runners,
    clock: Clock,
    changes: watch::Sender<u64>,
    shutdown: watch::Sender<bool>,
    start_lock: tokio::sync::Mutex<()>,
    /// Per machine, starts whose outcome is unknown; that machine takes no new start until they resolve.
    unresolved: Mutex<HashMap<String, HashSet<String>>>,
}

impl SessionService {
    pub fn new(cfg: HubConfig, store: Store, clock: Clock) -> Arc<Self> {
        Arc::new(Self {
            cfg: Arc::new(cfg),
            store,
            runners: Runners::default(),
            clock,
            changes: watch::channel(0).0,
            shutdown: watch::channel(false).0,
            start_lock: tokio::sync::Mutex::new(()),
            unresolved: Mutex::default(),
        })
    }

    pub fn config(&self) -> &HubConfig {
        &self.cfg
    }

    pub fn store(&self) -> &Store {
        &self.store
    }

    pub fn now(&self) -> i64 {
        self.clock.now_ms()
    }

    pub fn readiness(&self, machine: &str) -> Readiness {
        self.runners.readiness(machine)
    }

    pub fn shutdown_signal(&self) -> watch::Receiver<bool> {
        self.shutdown.subscribe()
    }

    pub fn begin_shutdown(&self) {
        self.shutdown.send_replace(true);
    }

    fn changed(&self) {
        self.changes.send_modify(|v| *v += 1);
    }

    async fn audit(
        &self,
        actor: &str,
        session_id: Option<&str>,
        kind: &str,
        detail: Value,
    ) -> Result<(), HubError> {
        self.store
            .append_audit(self.now(), actor, session_id, kind, &redact(&detail))
            .await?;
        Ok(())
    }

    async fn patch(
        &self,
        id: &str,
        patch: SessionPatch,
    ) -> Result<Option<SessionRecord>, HubError> {
        let s = self.store.update_session(id, patch, self.now()).await?;
        self.changed();
        Ok(s)
    }

    async fn session(&self, id: &str) -> Result<SessionRecord, HubError> {
        self.store
            .get_session(id)
            .await?
            .ok_or_else(|| HubError::UnknownSession(id.to_owned()))
    }

    /// A session that belongs to `machine`; events never touch another machine's sessions.
    async fn owned_by(&self, machine: &str, id: &str) -> Result<Option<SessionRecord>, HubError> {
        Ok(self
            .store
            .get_session(id)
            .await?
            .filter(|s| s.machine_id == machine))
    }

    fn resolve_project(&self, id: &str) -> Result<&ProjectConfig, HubError> {
        self.cfg
            .projects
            .iter()
            .find(|p| p.id == id || p.aliases.iter().any(|a| a == id))
            .ok_or_else(|| HubError::UnknownProject(id.to_owned()))
    }

    fn is_unresolved(&self, machine: &str) -> bool {
        self.unresolved
            .lock()
            .unwrap()
            .get(machine)
            .is_some_and(|s| !s.is_empty())
    }

    fn set_unresolved(&self, machine: &str, session_id: &str, unresolved: bool) {
        let mut map = self.unresolved.lock().unwrap();
        let set = map.entry(machine.to_owned()).or_default();
        if unresolved {
            set.insert(session_id.to_owned());
        } else {
            set.remove(session_id);
        }
    }

    fn unresolved_on(&self, machine: &str) -> Vec<String> {
        self.unresolved
            .lock()
            .unwrap()
            .get(machine)
            .map(|s| s.iter().cloned().collect())
            .unwrap_or_default()
    }

    // Idempotency.

    /// The recorded outcome of a retried request id, if there is one.
    async fn replayed(
        &self,
        request_id: Option<&str>,
        kind: &str,
        fingerprint: &str,
    ) -> Result<Option<Value>, HubError> {
        let Some(rid) = request_id else {
            return Ok(None);
        };
        match self.store.get_operation(rid).await? {
            None => Ok(None),
            Some(op) if op.kind != kind || op.fingerprint != fingerprint => {
                Err(HubError::RequestIdReused(rid.to_owned()))
            }
            Some(op) => replay(op).map(Some),
        }
    }

    /// Records the operation before any runner call. `Err(value)` is a concurrent retry's outcome.
    async fn begin(
        &self,
        request_id: Option<&str>,
        kind: &str,
        fingerprint: &str,
        session_id: Option<&str>,
    ) -> Result<Result<String, Value>, HubError> {
        let op = NewOperation {
            request_id: request_id
                .map(str::to_owned)
                .unwrap_or_else(|| format!("op-{}", random_id())),
            kind: kind.to_owned(),
            fingerprint: fingerprint.to_owned(),
            session_id: session_id.map(str::to_owned),
        };
        let id = op.request_id.clone();
        match self.store.begin_session_operation(op, self.now()).await? {
            Begin::Started => Ok(Ok(id)),
            Begin::Existing(op) => replay(op).map(Err),
        }
    }

    async fn finish(
        &self,
        op_id: &str,
        state: OpState,
        session_id: Option<&str>,
        result: &Value,
    ) -> Result<(), HubError> {
        self.store
            .finish_operation(op_id, state, session_id, Some(result), self.now())
            .await?;
        Ok(())
    }

    /// Records a failed runner call. Never sent: the record is dropped and the machine is offline.
    async fn settle_err(
        &self,
        op_id: &str,
        session_id: Option<&str>,
        machine: &str,
        e: ProtoError,
    ) -> HubError {
        let recorded = match e.code {
            ErrorCode::OutcomeUnknown => {
                let result = json!({ "operation_id": op_id, "session_id": session_id });
                self.finish(op_id, OpState::OutcomeUnknown, session_id, &result)
                    .await
                    .map(|()| HubError::OutcomeUnknown {
                        operation_id: op_id.to_owned(),
                        session_id: session_id.map(str::to_owned),
                    })
            }
            ErrorCode::RunnerDisconnected => self
                .store
                .delete_operation(op_id)
                .await
                .map(|()| HubError::MachineOffline(machine.to_owned()))
                .map_err(HubError::from),
            code => {
                let result = error_json(&code_str(code), &e.message);
                self.finish(op_id, OpState::Failed, session_id, &result)
                    .await
                    .map(|()| HubError::Runner(e))
            }
        };
        recorded.unwrap_or_else(|err| err)
    }

    // Startup and listing.

    /// After a hub start nothing is in flight: open operations become unknown, approvals are denied.
    pub async fn recover(&self) -> Result<(), HubError> {
        let now = self.now();
        let unknown = self.store.mark_pending_unknown(now).await?;
        let denied = self.store.deny_all_pending("hub_restart", now).await?;
        for a in &denied {
            self.audit_denied("hub", a).await?;
        }
        for a in self.store.mark_unconfirmed_allows_unknown().await? {
            self.audit(
                "hub",
                Some(&a.session_id),
                "approval_delivery_unknown",
                json!({ "approval_id": a.id, "tool_name": a.tool_name }),
            )
            .await?;
        }
        for s in self.store.list_sessions().await? {
            if s.lifecycle == Lifecycle::Starting {
                self.set_unresolved(&s.machine_id, &s.id, true);
            }
        }
        tracing::info!(unknown, denied = denied.len(), "hub state recovered");
        Ok(())
    }

    pub fn projects(&self) -> Vec<ProjectView> {
        self.cfg
            .projects
            .iter()
            .map(|p| ProjectView {
                id: p.id.clone(),
                aliases: p.aliases.clone(),
                description: p.description.clone(),
                default_agent: p.default_agent,
                machines: p.repos.keys().cloned().collect(),
            })
            .collect()
    }

    pub fn machines(&self) -> Vec<MachineView> {
        self.cfg
            .machines
            .iter()
            .map(|m| {
                let readiness = self.runners.readiness(&m.id);
                MachineView {
                    id: m.id.clone(),
                    connected: readiness != Readiness::Offline,
                    readiness,
                    runner: self.runners.info(&m.id),
                }
            })
            .collect()
    }

    // Start.

    pub async fn start(
        self: &Arc<Self>,
        caller: &Caller,
        req: StartRequest,
    ) -> Result<StartResult, HubError> {
        let project = self.resolve_project(&req.project_id)?.clone();
        let machine = req
            .machine_id
            .clone()
            .unwrap_or_else(|| DEFAULT_MACHINE.to_owned());
        if self.cfg.machine(&machine).is_none() {
            return Err(HubError::UnknownMachine(machine));
        }
        let repo = project
            .repos
            .get(&machine)
            .ok_or_else(|| HubError::ProjectNotOnMachine {
                project: project.id.clone(),
                machine: machine.clone(),
            })?
            .clone();
        if req.prompt.trim().is_empty() {
            return Err(HubError::BadRequest("prompt is empty".into()));
        }
        let agent = req.agent.unwrap_or(project.default_agent);
        let prompt_sha256 = sha256_hex(&req.prompt);
        let fp = fingerprint(&json!({
            "project": project.id, "machine": machine, "agent": agent.as_str(),
            "prompt_sha256": prompt_sha256, "notify": req.notify,
        }));
        let rid = req.request_id.as_deref();
        if let Some(v) = self.replayed(rid, KIND_START, &fp).await? {
            return self.replay_start(v).await;
        }
        match self.runners.readiness(&machine) {
            Readiness::Offline => return Err(HubError::MachineOffline(machine)),
            Readiness::Reconciling => return Err(HubError::Reconciling(machine)),
            Readiness::Ready if self.is_unresolved(&machine) => {
                return Err(HubError::Reconciling(machine));
            }
            Readiness::Ready => {}
        }

        let guard = self.start_lock.lock().await;
        let now = self.now();
        let since = now - DEDUPE_WINDOW.as_millis() as i64;
        if let Some(existing) = self
            .store
            .find_recent_start(&project.id, &machine, agent.as_str(), &prompt_sha256, since)
            .await?
        {
            let op_id = match self.begin(rid, KIND_START, &fp, Some(&existing.id)).await? {
                Ok(id) => id,
                Err(v) => return self.replay_start(v).await,
            };
            let result = StartResult::of(&existing, StartOutcome::Deduplicated, &op_id);
            let v = serde_json::to_value(&result).expect("serializes");
            self.finish(&op_id, OpState::Done, Some(&existing.id), &v)
                .await?;
            return Ok(result);
        }

        let session_id = random_id()[..12].to_owned();
        let record = SessionRecord {
            id: session_id.clone(),
            project_id: project.id.clone(),
            machine_id: machine.clone(),
            agent: agent.as_str().to_owned(),
            prompt_sha256,
            branch: Some(format!("sb/{session_id}")),
            worktree_path: None,
            workspace_id: None,
            pane_id: None,
            lifecycle: Lifecycle::Starting,
            agent_status: "unknown".into(),
            revision: 0,
            created_at: now,
            updated_at: now,
            agent_name: Some(agent_name(&session_id)),
            muted: !req.notify,
            hook_failed: false,
            error: None,
        };
        let op_id = rid
            .map(str::to_owned)
            .unwrap_or_else(|| format!("op-{}", random_id()));
        let op = NewOperation {
            request_id: op_id.clone(),
            kind: KIND_START.into(),
            fingerprint: fp,
            session_id: Some(session_id.clone()),
        };
        if let Begin::Existing(op) = self.store.begin_start(op, record.clone(), now).await? {
            return self.replay_start(replay(op)?).await;
        }
        drop(guard);
        self.changed();
        self.audit(
            &caller.actor,
            Some(&session_id),
            "session_start",
            json!({
                "project_id": project.id, "machine_id": machine, "agent": agent.as_str(),
                "branch": record.branch, "notify": req.notify, "prompt": req.prompt,
                "operation_id": op_id,
            }),
        )
        .await?;

        let op = Op::StartSession {
            session_id: session_id.clone(),
            project_id: project.id.clone(),
            repo_path: repo.to_string_lossy().into_owned(),
            branch: record.branch.clone().expect("set above"),
            agent,
            prompt: req.prompt.clone(),
            deny_patterns: project.deny_patterns.clone(),
        };
        match self.runners.call(&machine, op, START_TIMEOUT).await {
            Ok(v) => {
                let str_field = |k: &str| v.get(k).and_then(Value::as_str).map(str::to_owned);
                let s = self
                    .patch(
                        &session_id,
                        SessionPatch {
                            lifecycle: Some(Lifecycle::Running),
                            branch: str_field("branch"),
                            worktree_path: str_field("worktree_path"),
                            workspace_id: str_field("workspace_id"),
                            pane_id: str_field("pane_id"),
                            ..Default::default()
                        },
                    )
                    .await?
                    .unwrap_or(record);
                let result = StartResult::of(&s, StartOutcome::Started, &op_id);
                let v = serde_json::to_value(&result).expect("serializes");
                self.finish(&op_id, OpState::Done, Some(&session_id), &v)
                    .await?;
                self.set_unresolved(&machine, &session_id, false);
                Ok(result)
            }
            Err(e) if e.code == ErrorCode::OutcomeUnknown => {
                // A reconnect may already have reconciled it.
                if let Some(op) = self.store.get_operation(&op_id).await?
                    && matches!(op.state, OpState::Done | OpState::Failed)
                {
                    return self.replay_start(replay(op)?).await;
                }
                self.set_unresolved(&machine, &session_id, true);
                let still_starting = self
                    .store
                    .get_session(&session_id)
                    .await?
                    .is_some_and(|s| s.lifecycle == Lifecycle::Starting);
                if !still_starting {
                    self.set_unresolved(&machine, &session_id, false);
                }
                self.audit(
                    "hub",
                    Some(&session_id),
                    "start_outcome_unknown",
                    json!({ "operation_id": op_id, "message": e.message }),
                )
                .await?;
                Err(self
                    .settle_err(&op_id, Some(&session_id), &machine, e)
                    .await)
            }
            Err(e) => {
                let message = e.message.clone();
                self.patch(
                    &session_id,
                    SessionPatch {
                        lifecycle: Some(Lifecycle::Failed),
                        error: Some(format!("{}: {message}", code_str(e.code))),
                        ..Default::default()
                    },
                )
                .await?;
                // Never sent: the operation record is dropped so a retry can start afresh.
                Err(self
                    .settle_err(&op_id, Some(&session_id), &machine, e)
                    .await)
            }
        }
    }

    async fn replay_start(&self, v: Value) -> Result<StartResult, HubError> {
        let mut result: StartResult = serde_json::from_value(v)
            .map_err(|e| HubError::BadRequest(format!("recorded start is unreadable: {e}")))?;
        result.outcome = StartOutcome::Replayed;
        Ok(result)
    }

    // Prompts, reads and dialog answers.

    pub async fn prompt(
        self: &Arc<Self>,
        caller: &Caller,
        session_id: &str,
        text: &str,
        request_id: Option<&str>,
    ) -> Result<PromptResult, HubError> {
        let fp = fingerprint(&json!({ "session_id": session_id, "text": text }));
        if let Some(v) = self.replayed(request_id, KIND_PROMPT, &fp).await? {
            return from_recorded(v);
        }
        let s = self.session(session_id).await?;
        if s.lifecycle != Lifecycle::Running {
            return Err(HubError::NotRunning {
                id: s.id,
                lifecycle: s.lifecycle,
            });
        }
        if text.trim().is_empty() {
            return Err(HubError::BadRequest("prompt is empty".into()));
        }
        let op_id = match self
            .begin(request_id, KIND_PROMPT, &fp, Some(session_id))
            .await?
        {
            Ok(id) => id,
            Err(v) => return from_recorded(v),
        };
        self.audit(
            &caller.actor,
            Some(session_id),
            "prompt",
            json!({ "text": text, "operation_id": op_id }),
        )
        .await?;
        let op = Op::Prompt {
            session_id: session_id.to_owned(),
            text: text.to_owned(),
        };
        let v = match self.runners.call(&s.machine_id, op, CALL_TIMEOUT).await {
            Ok(v) => v,
            Err(e) => {
                return Err(self
                    .settle_err(&op_id, Some(session_id), &s.machine_id, e)
                    .await);
            }
        };
        let agent_status = v
            .get("agent_status")
            .and_then(Value::as_str)
            .map(str::to_owned);
        if let Some(status) = &agent_status {
            self.patch(
                session_id,
                SessionPatch {
                    agent_status: Some(status.clone()),
                    ..Default::default()
                },
            )
            .await?;
        }
        let result = PromptResult {
            session_id: session_id.to_owned(),
            agent_status,
        };
        let v = serde_json::to_value(&result).expect("serializes");
        self.finish(&op_id, OpState::Done, Some(session_id), &v)
            .await?;
        Ok(result)
    }

    /// The last `lines` of the agent's terminal, ANSI stripped by Herdr. Untrusted text.
    pub async fn read(&self, session_id: &str, lines: u32) -> Result<ReadResult, HubError> {
        let s = self.session(session_id).await?;
        let op = Op::Read {
            session_id: session_id.to_owned(),
            lines: lines.min(MAX_READ_LINES),
        };
        let v = self
            .runners
            .call(&s.machine_id, op, CALL_TIMEOUT)
            .await
            .map_err(|e| runner_error(&s.machine_id, e))?;
        Ok(ReadResult {
            text: v
                .get("text")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_owned(),
            revision: v.get("revision").and_then(Value::as_u64),
            truncated: v.get("truncated").and_then(Value::as_bool).unwrap_or(false),
            untrusted: true,
        })
    }

    /// Answers a question dialog the runner recognised. Permission dialogs go through `decide`.
    pub async fn answer(
        self: &Arc<Self>,
        caller: &Caller,
        session_id: &str,
        dialog_id: &str,
        option: &str,
    ) -> Result<Value, HubError> {
        let s = self.session(session_id).await?;
        if s.lifecycle != Lifecycle::Running {
            return Err(HubError::NotRunning {
                id: s.id,
                lifecycle: s.lifecycle,
            });
        }
        let fp = fingerprint(&json!({ "dialog_id": dialog_id, "option": option }));
        let op_id = match self.begin(None, KIND_ANSWER, &fp, Some(session_id)).await? {
            Ok(id) => id,
            Err(v) => return Ok(v),
        };
        self.audit(
            &caller.actor,
            Some(session_id),
            "dialog_answer",
            json!({ "dialog_id": dialog_id, "option": option, "operation_id": op_id }),
        )
        .await?;
        let op = Op::Answer {
            session_id: session_id.to_owned(),
            dialog_id: dialog_id.to_owned(),
            option: option.to_owned(),
        };
        let v = match self.runners.call(&s.machine_id, op, CALL_TIMEOUT).await {
            Ok(v) => v,
            Err(e) => {
                return Err(self
                    .settle_err(&op_id, Some(session_id), &s.machine_id, e)
                    .await);
            }
        };
        self.finish(&op_id, OpState::Done, Some(session_id), &v)
            .await?;
        Ok(v)
    }

    // Approvals.

    pub async fn decide(
        self: &Arc<Self>,
        caller: &Caller,
        session_id: &str,
        approval_id: &str,
        allow: bool,
        request_id: Option<&str>,
    ) -> Result<DecideResult, HubError> {
        let fp = fingerprint(&json!({
            "session_id": session_id, "approval_id": approval_id, "allow": allow,
        }));
        if let Some(v) = self.replayed(request_id, KIND_DECIDE, &fp).await? {
            let mut r: DecideResult = from_recorded(v)?;
            r.replayed = true;
            return Ok(r);
        }
        let s = self.session(session_id).await?;
        let approval = self
            .store
            .get_approval(approval_id)
            .await?
            .filter(|a| a.session_id == session_id)
            .ok_or_else(|| HubError::UnknownApproval(approval_id.to_owned()))?;
        let op_id = match self
            .begin(request_id, KIND_DECIDE, &fp, Some(session_id))
            .await?
        {
            Ok(id) => id,
            Err(v) => {
                let mut r: DecideResult = from_recorded(v)?;
                r.replayed = true;
                return Ok(r);
            }
        };
        let decision = if allow {
            Decision::Allow
        } else {
            Decision::Deny
        };
        let reason = format!(
            "{} by {}",
            if allow { "approved" } else { "denied" },
            caller.actor
        );
        let outcome = self
            .store
            .decide_approval(approval_id, decision, Some(&reason), self.now())
            .await?;
        let result = match outcome {
            DecideOutcome::NotFound => {
                return Err(HubError::UnknownApproval(approval_id.to_owned()));
            }
            DecideOutcome::AlreadyDecided { .. } => {
                DecideResult::of(&approval_now(self, approval_id).await?, true)
            }
            DecideOutcome::Expired => {
                self.audit(
                    "hub",
                    Some(session_id),
                    "approval_expired",
                    json!({ "approval_id": approval_id, "tool_name": approval.tool_name }),
                )
                .await?;
                let a = self
                    .deliver(&s.machine_id, session_id, approval_id, false)
                    .await?;
                DecideResult::of(&a, false)
            }
            DecideOutcome::Applied(d) => {
                self.audit(
                    &caller.actor,
                    Some(session_id),
                    "decision",
                    json!({
                        "approval_id": approval_id, "tool_name": approval.tool_name,
                        "tool_input": approval.tool_input, "decision": d.as_str(),
                    }),
                )
                .await?;
                let a = self
                    .deliver(&s.machine_id, session_id, approval_id, d == Decision::Allow)
                    .await?;
                DecideResult::of(&a, false)
            }
        };
        self.patch(session_id, SessionPatch::default()).await?;
        let v = serde_json::to_value(&result).expect("serializes");
        self.finish(&op_id, OpState::Done, Some(session_id), &v)
            .await?;
        Ok(result)
    }

    /// Hands a recorded decision to the runner and records what became of it.
    async fn deliver(
        &self,
        machine: &str,
        session_id: &str,
        approval_id: &str,
        allow: bool,
    ) -> Result<ApprovalRecord, HubError> {
        let op = Op::Decide {
            session_id: session_id.to_owned(),
            approval_id: approval_id.to_owned(),
            allow,
        };
        let delivery = match self.runners.call(machine, op, CALL_TIMEOUT).await {
            Ok(_) => Delivery::Delivered,
            Err(e) if e.code == ErrorCode::OutcomeUnknown => Delivery::Unknown,
            Err(e) => {
                if allow {
                    self.audit(
                        "hub",
                        Some(session_id),
                        "approval_not_delivered",
                        json!({ "approval_id": approval_id, "error": e.message }),
                    )
                    .await?;
                }
                Delivery::NotDelivered
            }
        };
        self.store
            .settle_delivery(approval_id, delivery)
            .await?
            .ok_or_else(|| HubError::UnknownApproval(approval_id.to_owned()))
    }

    /// Sends a recorded deny in the background; the runner denies on its own deadline anyway.
    fn deliver_deny_later(self: &Arc<Self>, machine: &str, a: &ApprovalRecord) {
        let (svc, machine, session_id, id) = (
            self.clone(),
            machine.to_owned(),
            a.session_id.clone(),
            a.id.clone(),
        );
        tokio::spawn(async move {
            if let Err(e) = svc.deliver(&machine, &session_id, &id, false).await {
                tracing::warn!(error = %e, approval_id = %id, "delivering a deny");
            }
        });
    }

    async fn audit_denied(&self, actor: &str, a: &ApprovalRecord) -> Result<(), HubError> {
        self.audit(
            actor,
            Some(&a.session_id),
            "approval_denied",
            json!({ "approval_id": a.id, "tool_name": a.tool_name, "reason": a.reason }),
        )
        .await?;
        self.patch(&a.session_id, SessionPatch::default()).await?;
        Ok(())
    }

    /// Denies approvals past their deadline. The background sweeper calls this every second.
    pub async fn sweep(self: &Arc<Self>) -> Result<usize, HubError> {
        let expired = self.store.expire_approvals(self.now()).await?;
        for a in &expired {
            self.audit_denied("hub", a).await?;
            if let Some(s) = self.store.get_session(&a.session_id).await? {
                self.deliver_deny_later(&s.machine_id, a);
            }
        }
        Ok(expired.len())
    }

    pub fn spawn_sweeper(self: &Arc<Self>) -> JoinHandle<()> {
        let svc = self.clone();
        tokio::spawn(async move {
            let mut tick = tokio::time::interval(SWEEP_INTERVAL);
            loop {
                tick.tick().await;
                if let Err(e) = svc.sweep().await {
                    tracing::warn!(error = %e, "approval sweep failed");
                }
            }
        })
    }

    // Stop.

    pub async fn stop(
        self: &Arc<Self>,
        caller: &Caller,
        session_id: &str,
        remove_worktree_if_clean: bool,
    ) -> Result<StopResult, HubError> {
        let s = self.session(session_id).await?;
        match s.lifecycle {
            Lifecycle::Stopped => {
                return Ok(StopResult {
                    session_id: s.id,
                    worktree_removed: false,
                    kept_because: None,
                    branch: s.branch,
                    already_stopped: true,
                });
            }
            Lifecycle::Starting => {
                return Err(HubError::NotRunning {
                    id: s.id,
                    lifecycle: s.lifecycle,
                });
            }
            _ => {}
        }
        if self.runners.readiness(&s.machine_id) == Readiness::Offline {
            return Err(HubError::MachineOffline(s.machine_id));
        }
        let fp = fingerprint(&json!({ "remove_worktree_if_clean": remove_worktree_if_clean }));
        let op_id = match self.begin(None, KIND_STOP, &fp, Some(session_id)).await? {
            Ok(id) => id,
            Err(v) => return from_recorded(v),
        };
        for a in self
            .store
            .deny_pending(session_id, "stopped", self.now())
            .await?
        {
            self.audit_denied(&caller.actor, &a).await?;
        }
        self.patch(
            session_id,
            SessionPatch {
                lifecycle: Some(Lifecycle::Stopping),
                ..Default::default()
            },
        )
        .await?;
        self.audit(
            &caller.actor,
            Some(session_id),
            "stop",
            json!({ "remove_worktree_if_clean": remove_worktree_if_clean, "operation_id": op_id }),
        )
        .await?;
        let op = Op::Stop {
            session_id: session_id.to_owned(),
            remove_worktree_if_clean,
        };
        let v = match self.runners.call(&s.machine_id, op, STOP_TIMEOUT).await {
            Ok(v) => v,
            // The runner no longer has it, so it is already stopped there.
            Err(e) if e.code == ErrorCode::NotFound => json!({ "worktree_removed": false }),
            Err(e) if e.code == ErrorCode::RunnerDisconnected => {
                self.patch(
                    session_id,
                    SessionPatch {
                        lifecycle: Some(s.lifecycle),
                        ..Default::default()
                    },
                )
                .await?;
                return Err(self
                    .settle_err(&op_id, Some(session_id), &s.machine_id, e)
                    .await);
            }
            Err(e) => {
                if e.code != ErrorCode::OutcomeUnknown {
                    self.patch(
                        session_id,
                        SessionPatch {
                            error: Some(format!("stop failed: {}", e.message)),
                            ..Default::default()
                        },
                    )
                    .await?;
                }
                return Err(self
                    .settle_err(&op_id, Some(session_id), &s.machine_id, e)
                    .await);
            }
        };
        self.patch(
            session_id,
            SessionPatch {
                lifecycle: Some(Lifecycle::Stopped),
                ..Default::default()
            },
        )
        .await?;
        let result = StopResult {
            session_id: session_id.to_owned(),
            worktree_removed: v
                .get("worktree_removed")
                .and_then(Value::as_bool)
                .unwrap_or(false),
            kept_because: v
                .get("kept_because")
                .and_then(Value::as_str)
                .map(str::to_owned),
            branch: s.branch,
            already_stopped: false,
        };
        let v = serde_json::to_value(&result).expect("serializes");
        self.finish(&op_id, OpState::Done, Some(session_id), &v)
            .await?;
        Ok(result)
    }

    // Reading sessions.

    async fn view(&self, s: SessionRecord) -> Result<SessionView, HubError> {
        let mute_all = self.store.notify_settings().await?.mute_all;
        let pending = self.store.pending_approvals(&s.id).await?;
        Ok(SessionView {
            machine: self.runners.readiness(&s.machine_id),
            notifies: !s.muted && !mute_all,
            pending_approvals: pending
                .into_iter()
                .map(|a| ApprovalView {
                    id: a.id,
                    tool_name: a.tool_name,
                    tool_input: a.tool_input,
                    created_at: a.created_at,
                    deadline: a.deadline,
                })
                .collect(),
            id: s.id,
            project_id: s.project_id,
            machine_id: s.machine_id,
            agent: s.agent,
            agent_name: s.agent_name,
            branch: s.branch,
            worktree_path: s.worktree_path,
            lifecycle: s.lifecycle,
            agent_status: s.agent_status,
            hook_failed: s.hook_failed,
            error: s.error,
            revision: s.revision,
            created_at: s.created_at,
            updated_at: s.updated_at,
            muted: s.muted,
        })
    }

    /// The session's state. With `after_revision`, waits up to `wait` (capped at 25 s) for a newer one.
    pub async fn get(
        &self,
        session_id: &str,
        after_revision: Option<i64>,
        wait: Duration,
    ) -> Result<SessionView, HubError> {
        let mut changes = self.changes.subscribe();
        let deadline = Instant::now() + wait.min(MAX_WAIT);
        loop {
            let s = self.session(session_id).await?;
            if after_revision.is_none_or(|r| s.revision > r) || Instant::now() >= deadline {
                return self.view(s).await;
            }
            if !matches!(
                tokio::time::timeout_at(deadline, changes.changed()).await,
                Ok(Ok(()))
            ) {
                return self.view(self.session(session_id).await?).await;
            }
        }
    }

    pub async fn list(&self) -> Result<Vec<SessionView>, HubError> {
        let mut out = Vec::new();
        for s in self.store.list_sessions().await? {
            out.push(self.view(s).await?);
        }
        Ok(out)
    }

    // Notification mutes, shared with the WhatsApp bridge.

    pub async fn notify(
        &self,
        caller: &Caller,
        session_id: &str,
        muted: bool,
    ) -> Result<NotifyResult, HubError> {
        self.session(session_id).await?;
        let s = self
            .patch(
                session_id,
                SessionPatch {
                    muted: Some(muted),
                    ..Default::default()
                },
            )
            .await?
            .ok_or_else(|| HubError::UnknownSession(session_id.to_owned()))?;
        self.audit(
            &caller.actor,
            Some(session_id),
            "notify",
            json!({ "muted": muted }),
        )
        .await?;
        let mute_all = self.store.notify_settings().await?.mute_all;
        Ok(NotifyResult {
            session_id: s.id,
            agent_name: s.agent_name,
            muted: s.muted,
            mute_all,
            notifies: !s.muted && !mute_all,
        })
    }

    /// The bridge's view: opted-in agents, muted agent names and mute-all.
    pub async fn notify_state(&self) -> Result<WatchState, HubError> {
        let settings = self.store.notify_settings().await?;
        let mut state = WatchState {
            mute_all: settings.mute_all,
            ..Default::default()
        };
        for a in settings.agents {
            if a.watched {
                state.watched.insert(a.name.clone());
            }
            if a.muted {
                state.muted.insert(a.name);
            }
        }
        for s in self.store.list_sessions().await? {
            if let (true, Some(name)) = (s.muted, s.agent_name) {
                state.muted.insert(name);
            }
        }
        Ok(state)
    }

    pub async fn set_mute_all(
        &self,
        caller: &Caller,
        mute_all: bool,
    ) -> Result<WatchState, HubError> {
        self.store.set_mute_all(mute_all).await?;
        self.audit(
            &caller.actor,
            None,
            "notify_all",
            json!({ "mute_all": mute_all }),
        )
        .await?;
        self.changed();
        self.notify_state().await
    }

    /// Mutes or unmutes by Herdr agent name: a session's `sb-` agent, or any other named agent.
    pub async fn set_agent_muted(
        &self,
        caller: &Caller,
        agent: &str,
        muted: bool,
    ) -> Result<WatchState, HubError> {
        match self.store.get_session_by_agent_name(agent).await? {
            Some(s) => {
                self.notify(caller, &s.id, muted).await?;
            }
            None => {
                self.store
                    .set_notify_agent(agent, None, Some(muted), self.now())
                    .await?;
                self.audit(
                    &caller.actor,
                    None,
                    "notify",
                    json!({ "agent": agent, "muted": muted }),
                )
                .await?;
            }
        }
        self.notify_state().await
    }

    /// Opts an agent that Switchboard did not create in or out of notifications.
    pub async fn set_agent_watched(
        &self,
        caller: &Caller,
        agent: &str,
        watched: bool,
    ) -> Result<WatchState, HubError> {
        self.store
            .set_notify_agent(agent, Some(watched), None, self.now())
            .await?;
        self.audit(
            &caller.actor,
            None,
            "watch",
            json!({ "agent": agent, "watched": watched }),
        )
        .await?;
        self.notify_state().await
    }

    /// Imports the bridge's own watch file once. Returns false if it was imported before.
    pub async fn import_watch_state(
        &self,
        caller: &Caller,
        state: &WatchState,
    ) -> Result<bool, HubError> {
        let imported = self
            .store
            .import_notify(
                state.watched.iter().cloned().collect(),
                state.muted.iter().cloned().collect(),
                state.mute_all,
                self.now(),
            )
            .await?;
        if imported {
            self.audit(
                &caller.actor,
                None,
                "notify_import",
                json!({
                    "watched": state.watched, "muted": state.muted, "mute_all": state.mute_all,
                }),
            )
            .await?;
            self.changed();
        }
        Ok(imported)
    }

    pub async fn audit_log(
        &self,
        session_id: Option<&str>,
        limit: u32,
    ) -> Result<Vec<crate::store::AuditEntry>, HubError> {
        Ok(self.store.list_audit(session_id, limit).await?)
    }

    // Runner connections.

    /// Attaches a runner's connection and reconciles its hello; the machine takes starts after that.
    pub async fn runner_connected(
        self: &Arc<Self>,
        machine: &str,
        hello: Hello,
    ) -> (u64, mpsc::Receiver<Frame>) {
        let info = RunnerInfo {
            runner_version: hello.runner_version.clone(),
            herdr_version: hello.herdr_version.clone(),
            connected_at: self.now(),
        };
        let (generation, rx) = self.runners.attach(machine, info.clone());
        let reconciled = async {
            self.reconcile(machine, &hello.sessions).await?;
            self.audit(
                &format!("runner:{machine}"),
                None,
                "runner_connected",
                json!({ "runner_version": info.runner_version, "herdr_version": info.herdr_version,
                        "sessions": hello.sessions.len() }),
            )
            .await
        };
        match reconciled.await {
            Ok(()) => self.runners.set_ready(machine, generation),
            // Starts stay refused until the next connection reconciles.
            Err(e) => tracing::error!(error = %e, machine, "reconciling runner hello failed"),
        }
        self.changed();
        (generation, rx)
    }

    pub async fn runner_disconnected(self: &Arc<Self>, machine: &str, generation: u64) {
        if !self.runners.detach(machine, generation) {
            return;
        }
        let result = async {
            let denied = self
                .store
                .deny_pending_on_machine(machine, "runner_lost", self.now())
                .await?;
            for a in &denied {
                self.audit_denied("hub", a).await?;
            }
            self.audit(
                "hub",
                None,
                "runner_disconnected",
                json!({ "machine_id": machine }),
            )
            .await
        };
        if let Err(e) = result.await {
            tracing::error!(error = %e, machine, "handling runner loss");
        }
        self.changed();
    }

    pub async fn runner_frame(self: &Arc<Self>, machine: &str, generation: u64, frame: Frame) {
        match frame {
            Frame::Resp(resp) => {
                if !self.runners.complete(machine, generation, resp) {
                    tracing::debug!(machine, "dropping a stale or unknown reply");
                }
            }
            Frame::Event(event) if self.runners.is_current(machine, generation) => {
                if let Err(e) = self.event(machine, event).await {
                    tracing::warn!(error = %e, machine, "handling runner event");
                }
            }
            Frame::Event(_) => {}
            other => tracing::warn!(machine, ?other, "unexpected frame from runner"),
        }
    }

    /// Applies the runner's own list of sessions to the hub's records for that machine.
    async fn reconcile(
        self: &Arc<Self>,
        machine: &str,
        owned: &[OwnedSession],
    ) -> Result<(), HubError> {
        let now = self.now();
        let by_id: HashMap<&str, &OwnedSession> =
            owned.iter().map(|o| (o.session_id.as_str(), o)).collect();
        let mut known = HashSet::new();
        for s in self.store.list_sessions().await? {
            if s.machine_id != machine {
                continue;
            }
            known.insert(s.id.clone());
            match by_id.get(s.id.as_str()) {
                Some(o) => self.reconcile_owned(&s, o).await?,
                None => self.reconcile_missing(&s, now).await?,
            }
        }
        for o in owned.iter().filter(|o| !known.contains(&o.session_id)) {
            self.audit(
                &format!("runner:{machine}"),
                None,
                "unknown_runner_session",
                json!({ "session_id": o.session_id, "workspace_id": o.workspace_id }),
            )
            .await?;
        }
        // Denials recorded while the runner was away; it may still be holding those approvals.
        let since = now - DEADLINE_MARGIN.as_millis() as i64;
        for a in self.store.undelivered_denials(machine, since).await? {
            self.deliver_deny_later(machine, &a);
        }
        Ok(())
    }

    async fn reconcile_owned(&self, s: &SessionRecord, o: &OwnedSession) -> Result<(), HubError> {
        let mut patch = SessionPatch {
            agent_status: Some(o.agent_status.clone()),
            hook_failed: Some(o.hook_failed),
            workspace_id: Some(o.workspace_id.clone()),
            pane_id: o.pane_id.clone(),
            ..Default::default()
        };
        let start_reconciled = s.lifecycle == Lifecycle::Starting;
        if start_reconciled {
            patch.lifecycle = Some(Lifecycle::Running);
        }
        let unchanged = !start_reconciled
            && s.agent_status == o.agent_status
            && s.hook_failed == o.hook_failed
            && s.workspace_id.as_deref() == Some(&o.workspace_id)
            && (o.pane_id.is_none() || s.pane_id == o.pane_id);
        if unchanged {
            return Ok(());
        }
        let updated = self.patch(&s.id, patch).await?;
        if start_reconciled && let Some(updated) = updated {
            self.resolve_start(&updated).await?;
        }
        Ok(())
    }

    async fn reconcile_missing(
        self: &Arc<Self>,
        s: &SessionRecord,
        now: i64,
    ) -> Result<(), HubError> {
        match s.lifecycle {
            Lifecycle::Running => {
                for a in self
                    .store
                    .deny_pending(&s.id, "session_exited", now)
                    .await?
                {
                    self.audit_denied("hub", &a).await?;
                }
                self.patch(
                    &s.id,
                    SessionPatch {
                        lifecycle: Some(Lifecycle::Exited),
                        error: Some("the runner no longer has this session".into()),
                        ..Default::default()
                    },
                )
                .await?;
            }
            Lifecycle::Stopping => {
                self.patch(
                    &s.id,
                    SessionPatch {
                        lifecycle: Some(Lifecycle::Stopped),
                        ..Default::default()
                    },
                )
                .await?;
                self.store
                    .resolve_session_operations(
                        &s.id,
                        KIND_STOP,
                        OpState::Done,
                        &json!({ "session_id": s.id, "reconciled": true }),
                        now,
                    )
                    .await?;
            }
            Lifecycle::Starting => {
                let deadline = s.created_at + START_TIMEOUT.as_millis() as i64;
                if now >= deadline {
                    self.fail_start(s, "the start did not complete on the runner")
                        .await?;
                } else {
                    // It may still be creating the worktree; look again once a start must be over.
                    self.set_unresolved(&s.machine_id, &s.id, true);
                    self.schedule_recheck(
                        &s.machine_id,
                        Duration::from_millis((deadline - now) as u64),
                    );
                }
            }
            _ => {}
        }
        Ok(())
    }

    fn schedule_recheck(self: &Arc<Self>, machine: &str, after: Duration) {
        let (svc, machine) = (self.clone(), machine.to_owned());
        tokio::spawn(async move {
            tokio::time::sleep(after).await;
            if let Err(e) = svc.recheck(&machine).await {
                tracing::warn!(error = %e, machine, "rechecking unresolved starts");
            }
        });
    }

    /// Settles this machine's unresolved starts from the runner's current list of sessions.
    pub async fn recheck(self: &Arc<Self>, machine: &str) -> Result<(), HubError> {
        let ids = self.unresolved_on(machine);
        if ids.is_empty() || self.runners.readiness(machine) != Readiness::Ready {
            return Ok(());
        }
        let owned: Vec<OwnedSession> = match self
            .runners
            .call(machine, Op::ListSessions, CALL_TIMEOUT)
            .await
        {
            Ok(v) => serde_json::from_value(v)
                .map_err(|e| HubError::BadRequest(format!("runner session list: {e}")))?,
            Err(e) => {
                self.schedule_recheck(machine, RECHECK_RETRY);
                return Err(runner_error(machine, e));
            }
        };
        let now = self.now();
        let mut pending = false;
        for id in ids {
            let Some(s) = self.store.get_session(&id).await? else {
                self.set_unresolved(machine, &id, false);
                continue;
            };
            if s.lifecycle != Lifecycle::Starting {
                self.set_unresolved(machine, &id, false);
            } else if let Some(o) = owned.iter().find(|o| o.session_id == id) {
                self.reconcile_owned(&s, o).await?;
            } else if now >= s.created_at + START_TIMEOUT.as_millis() as i64 {
                self.fail_start(&s, "the start did not complete on the runner")
                    .await?;
            } else {
                pending = true;
            }
        }
        if pending {
            self.schedule_recheck(machine, RECHECK_RETRY);
        }
        Ok(())
    }

    /// Records a start that turned out to have happened.
    async fn resolve_start(&self, s: &SessionRecord) -> Result<(), HubError> {
        for op in self.store.open_operations(&s.id, KIND_START).await? {
            let result = StartResult::of(s, StartOutcome::Reconciled, &op.request_id);
            let v = serde_json::to_value(&result).expect("serializes");
            self.store
                .finish_operation(&op.request_id, OpState::Done, None, Some(&v), self.now())
                .await?;
        }
        self.set_unresolved(&s.machine_id, &s.id, false);
        self.audit("hub", Some(&s.id), "start_reconciled", json!({}))
            .await
    }

    async fn fail_start(&self, s: &SessionRecord, message: &str) -> Result<(), HubError> {
        self.patch(
            &s.id,
            SessionPatch {
                lifecycle: Some(Lifecycle::Failed),
                error: Some(message.to_owned()),
                ..Default::default()
            },
        )
        .await?;
        self.store
            .resolve_session_operations(
                &s.id,
                KIND_START,
                OpState::Failed,
                &error_json("start_failed", message),
                self.now(),
            )
            .await?;
        self.set_unresolved(&s.machine_id, &s.id, false);
        self.audit(
            "hub",
            Some(&s.id),
            "start_failed",
            json!({ "message": message }),
        )
        .await
    }

    async fn event(self: &Arc<Self>, machine: &str, event: Event) -> Result<(), HubError> {
        let actor = format!("runner:{machine}");
        match event {
            Event::AgentStatus {
                session_id,
                agent_status,
                ..
            } => {
                if let Some(s) = self.owned_by(machine, &session_id).await?
                    && s.agent_status != agent_status
                {
                    self.patch(
                        &s.id,
                        SessionPatch {
                            agent_status: Some(agent_status),
                            ..Default::default()
                        },
                    )
                    .await?;
                }
            }
            Event::ApprovalRequested {
                session_id,
                approval_id,
                tool_name,
                tool_input,
                deadline_ms,
            } => {
                let live = self
                    .owned_by(machine, &session_id)
                    .await?
                    .filter(|s| matches!(s.lifecycle, Lifecycle::Starting | Lifecycle::Running));
                if live.is_none() {
                    self.audit(
                        &actor,
                        None,
                        "approval_rejected",
                        json!({ "session_id": session_id, "approval_id": approval_id,
                                "tool_name": tool_name, "reason": "no live session" }),
                    )
                    .await?;
                    let svc = self.clone();
                    let machine = machine.to_owned();
                    tokio::spawn(async move {
                        let op = Op::Decide {
                            session_id,
                            approval_id,
                            allow: false,
                        };
                        let _ = svc.runners.call(&machine, op, CALL_TIMEOUT).await;
                    });
                    return Ok(());
                }
                let now = self.now();
                let deadline =
                    (now + deadline_ms as i64 - DEADLINE_MARGIN.as_millis() as i64).max(now);
                let created = self
                    .store
                    .create_approval(ApprovalRecord {
                        id: approval_id.clone(),
                        session_id: session_id.clone(),
                        tool_name: tool_name.clone(),
                        tool_input: tool_input.clone(),
                        created_at: now,
                        deadline,
                        outcome: None,
                        reason: None,
                        decided_at: None,
                        delivery: None,
                    })
                    .await;
                if let Err(e) = created {
                    tracing::warn!(error = %e, approval_id, "ignoring a repeated approval request");
                    return Ok(());
                }
                self.audit(
                    &actor,
                    Some(&session_id),
                    "approval_requested",
                    json!({ "approval_id": approval_id, "tool_name": tool_name,
                            "tool_input": tool_input, "deadline": deadline }),
                )
                .await?;
                self.patch(&session_id, SessionPatch::default()).await?;
            }
            Event::ApprovalClosed {
                session_id,
                approval_id,
                reason,
            } => {
                let Some(s) = self.owned_by(machine, &session_id).await? else {
                    return Ok(());
                };
                if self
                    .store
                    .get_approval(&approval_id)
                    .await?
                    .is_none_or(|a| a.session_id != s.id)
                {
                    return Ok(());
                }
                let outcome = self
                    .store
                    .decide_approval(&approval_id, Decision::Deny, Some(&reason), self.now())
                    .await?;
                if matches!(outcome, DecideOutcome::Applied(_) | DecideOutcome::Expired) {
                    // The runner closed it itself, so its deny is already in force there.
                    self.store
                        .settle_delivery(&approval_id, Delivery::Delivered)
                        .await?;
                    self.audit(
                        &actor,
                        Some(&s.id),
                        "approval_closed",
                        json!({ "approval_id": approval_id, "reason": reason }),
                    )
                    .await?;
                    self.patch(&s.id, SessionPatch::default()).await?;
                }
            }
            Event::HookFailed {
                session_id,
                reason,
                screen_excerpt,
            } => {
                if let Some(s) = self.owned_by(machine, &session_id).await? {
                    self.patch(
                        &s.id,
                        SessionPatch {
                            hook_failed: Some(true),
                            ..Default::default()
                        },
                    )
                    .await?;
                    self.audit(
                        &actor,
                        Some(&s.id),
                        "hook_failed",
                        json!({ "reason": reason, "screen_excerpt": screen_excerpt }),
                    )
                    .await?;
                }
            }
            Event::Audit {
                session_id,
                kind,
                detail,
            } => {
                let s = self.owned_by(machine, &session_id).await?;
                let detail = match &s {
                    Some(_) => detail,
                    None => json!({ "claimed_session_id": session_id, "detail": detail }),
                };
                self.audit(
                    &actor,
                    s.as_ref().map(|s| s.id.as_str()),
                    &kind,
                    detail.clone(),
                )
                .await?;
                if let Some(s) = s {
                    self.lifecycle_hint(&s, &kind, &detail).await?;
                }
            }
            Event::SessionExited { session_id } => {
                let Some(s) = self.owned_by(machine, &session_id).await? else {
                    return Ok(());
                };
                let lifecycle = match s.lifecycle {
                    Lifecycle::Stopping => Lifecycle::Stopped,
                    Lifecycle::Starting | Lifecycle::Running => Lifecycle::Exited,
                    _ => return Ok(()),
                };
                for a in self
                    .store
                    .deny_pending(&s.id, "session_exited", self.now())
                    .await?
                {
                    self.audit_denied(&actor, &a).await?;
                }
                self.patch(
                    &s.id,
                    SessionPatch {
                        lifecycle: Some(lifecycle),
                        ..Default::default()
                    },
                )
                .await?;
            }
            Event::Resynced { sessions } => {
                for o in sessions {
                    if let Some(s) = self.owned_by(machine, &o.session_id).await?
                        && (s.agent_status != o.agent_status || s.hook_failed != o.hook_failed)
                    {
                        self.patch(
                            &s.id,
                            SessionPatch {
                                agent_status: Some(o.agent_status),
                                hook_failed: Some(o.hook_failed),
                                ..Default::default()
                            },
                        )
                        .await?;
                    }
                }
            }
        }
        Ok(())
    }

    /// The runner's own audit entries settle a start whose reply was lost.
    async fn lifecycle_hint(
        &self,
        s: &SessionRecord,
        kind: &str,
        detail: &Value,
    ) -> Result<(), HubError> {
        let unresolved = self
            .unresolved
            .lock()
            .unwrap()
            .get(&s.machine_id)
            .is_some_and(|set| set.contains(&s.id));
        if !unresolved || s.lifecycle != Lifecycle::Starting {
            return Ok(());
        }
        match kind {
            "prompt" if detail.get("initial") == Some(&json!(true)) => {
                if let Some(s) = self
                    .patch(
                        &s.id,
                        SessionPatch {
                            lifecycle: Some(Lifecycle::Running),
                            ..Default::default()
                        },
                    )
                    .await?
                {
                    self.resolve_start(&s).await?;
                }
            }
            "start_failed" => {
                let message = detail
                    .get("message")
                    .and_then(Value::as_str)
                    .unwrap_or("the runner reported a failed start");
                self.fail_start(s, message).await?;
            }
            _ => {}
        }
        Ok(())
    }
}

fn runner_error(machine: &str, e: ProtoError) -> HubError {
    match e.code {
        ErrorCode::RunnerDisconnected => HubError::MachineOffline(machine.to_owned()),
        _ => HubError::Runner(e),
    }
}

fn replay(op: Operation) -> Result<Value, HubError> {
    match op.state {
        OpState::Done => Ok(op.result.unwrap_or(Value::Null)),
        OpState::Failed => {
            let err = op.result.as_ref().and_then(|r| r.get("error"));
            let field = |k: &str| {
                err.and_then(|e| e.get(k))
                    .and_then(Value::as_str)
                    .unwrap_or("failed")
                    .to_owned()
            };
            Err(HubError::Recorded {
                code: field("code"),
                message: field("message"),
            })
        }
        OpState::Pending | OpState::OutcomeUnknown => Err(HubError::OutcomeUnknown {
            operation_id: op.request_id,
            session_id: op.session_id,
        }),
    }
}

fn from_recorded<T: serde::de::DeserializeOwned>(v: Value) -> Result<T, HubError> {
    serde_json::from_value(v)
        .map_err(|e| HubError::BadRequest(format!("recorded result is unreadable: {e}")))
}

async fn approval_now(svc: &SessionService, id: &str) -> Result<ApprovalRecord, HubError> {
    svc.store
        .get_approval(id)
        .await?
        .ok_or_else(|| HubError::UnknownApproval(id.to_owned()))
}

#[cfg(test)]
#[path = "service_tests.rs"]
mod tests;
