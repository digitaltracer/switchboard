//! The app API's public types. They are separate from the hub's internal ones so the contract
//! changes only on purpose. Status-like fields are strings with documented values, so an old
//! client never fails to decode a value added later.

use serde::{Deserialize, Serialize};
use serde_json::Value;
use utoipa::{IntoParams, ToSchema};

use crate::hub::route::Target;
use crate::hub::service::{
    ApprovalView, ChangesResult, DecideResult, DeleteResult, MachineView, ProjectView,
    PromptResult, SessionList, SessionView, StartResult, StopResult,
};
use crate::proto::TranscriptPage;
use crate::store::{DeviceRecord, Operation, SeenAgentRecord};

#[derive(Debug, Serialize, ToSchema)]
pub struct ApiError {
    pub error: ErrorBody,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct ErrorBody {
    /// Stable code, e.g. `outcome_unknown`, `approve_not_allowed`, `invalid_token`.
    pub code: String,
    pub message: String,
    /// Set with `outcome_unknown`: look the request up later by its request id.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub operation_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct Meta {
    pub api_version: u32,
    pub hub_id: String,
    /// The oldest app build this hub still serves; older builds should ask to be updated.
    pub min_client_build: u32,
    /// The signed message version the hub accepts.
    pub message_version: u32,
    /// The longest long-poll wait the hub honours.
    pub max_wait_ms: u64,
}

// Pairing and tokens.

#[derive(Debug, Deserialize, ToSchema)]
pub struct PairRequest {
    /// The single-use code from `switchboard admin pair-device`.
    pub code: String,
    /// Shown in the devices list, e.g. "Adarsh's iPhone".
    pub name: String,
    /// P-256 public key, SEC1 uncompressed, base64url without padding. Signs token challenges.
    pub auth_key: String,
    /// Same encoding. Signs approvals; the phone keeps it behind Face ID or the passcode.
    pub approval_key: String,
    #[serde(default)]
    pub client_build: Option<i64>,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct PairResponse {
    pub device_id: String,
    /// Show this; the admin confirms the terminal shows the same.
    pub fingerprint: String,
    pub hub_id: String,
    /// `pending` until the admin confirms.
    pub state: String,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct PairStatus {
    /// `pending`, `active`, `rejected` or `revoked`.
    pub state: String,
}

#[derive(Debug, Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
pub struct WaitQuery {
    /// Return as soon as something is newer than this revision.
    pub after_revision: Option<i64>,
    /// How long to wait for a change, capped at `max_wait_ms`.
    pub wait_ms: Option<u64>,
}

#[derive(Debug, Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
pub struct PairWaitQuery {
    /// How long to wait while the device is still pending, capped at `max_wait_ms`.
    pub wait_ms: Option<u64>,
}

#[derive(Debug, Deserialize, ToSchema)]
pub struct TokenChallengeRequest {
    pub device_id: String,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct Challenge {
    pub nonce: String,
    /// Unix milliseconds; part of the signed message.
    pub expires_at: i64,
    pub message_version: u32,
    pub hub_id: String,
    pub device_id: String,
}

#[derive(Debug, Deserialize, ToSchema)]
pub struct TokenRequest {
    pub device_id: String,
    pub nonce: String,
    /// DER ECDSA P-256 over SHA-256 of the token message, base64url. The message is
    /// `switchboard-token-v1`, then `hub=`, `device=`, `nonce=`, `expires=` lines, joined by `\n`.
    pub signature: String,
    pub message_version: u32,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct TokenResponse {
    pub access_token: String,
    pub expires_at: i64,
}

// Projects, machines and sessions.

#[derive(Debug, Serialize, ToSchema)]
pub struct Project {
    pub id: String,
    pub aliases: Vec<String>,
    pub description: String,
    /// `claude` or `codex`.
    pub default_agent: String,
    pub machines: Vec<String>,
}

impl From<ProjectView> for Project {
    fn from(p: ProjectView) -> Self {
        Self {
            id: p.id,
            aliases: p.aliases,
            description: p.description,
            default_agent: p.default_agent.as_str().to_owned(),
            machines: p.machines,
        }
    }
}

#[derive(Debug, Serialize, ToSchema)]
pub struct Machine {
    pub id: String,
    /// `offline`, `reconciling` or `ready`.
    pub status: String,
    pub default: bool,
}

impl Machine {
    pub fn of(m: MachineView, default_machine: &str) -> Self {
        Self {
            default: m.id == default_machine,
            id: m.id,
            status: enum_str(&m.readiness),
        }
    }
}

#[derive(Debug, Serialize, ToSchema)]
pub struct Approval {
    pub id: String,
    /// `permission`, `question` (Claude's AskUserQuestion) or `plan` (plan approval). Questions
    /// and plans can't be answered from the app yet: approving one leaves the agent at a dialog.
    pub kind: String,
    pub tool_name: String,
    /// The masked tool input as text: a shell command, or the input as JSON.
    pub input_text: String,
    /// For a question, each question's text.
    pub questions: Vec<String>,
    /// For a plan, the plan.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub plan: Option<String>,
    pub created_at: i64,
    /// Unix milliseconds; unanswered by then means denied.
    pub deadline: i64,
}

/// Classifies an approval the way the app shows it.
pub fn approval_kind(tool_name: &str) -> &'static str {
    match tool_name {
        "AskUserQuestion" => "question",
        "ExitPlanMode" => "plan",
        _ => "permission",
    }
}

impl From<ApprovalView> for Approval {
    fn from(a: ApprovalView) -> Self {
        let input = &a.tool_input;
        let input_text = match input.get("command").and_then(Value::as_str) {
            Some(cmd) => cmd.to_owned(),
            None => serde_json::to_string_pretty(input).unwrap_or_default(),
        };
        let questions = input
            .get("questions")
            .and_then(Value::as_array)
            .map(|qs| {
                qs.iter()
                    .filter_map(|q| q.get("question").and_then(Value::as_str))
                    .map(str::to_owned)
                    .collect()
            })
            .unwrap_or_default();
        Self {
            kind: approval_kind(&a.tool_name).to_owned(),
            plan: input.get("plan").and_then(Value::as_str).map(str::to_owned),
            id: a.id,
            tool_name: a.tool_name,
            input_text,
            questions,
            created_at: a.created_at,
            deadline: a.deadline,
        }
    }
}

#[derive(Debug, Serialize, ToSchema)]
pub struct Session {
    pub id: String,
    /// Project plus the first four characters of the id, as WhatsApp shows it.
    pub label: String,
    pub project_id: String,
    pub machine_id: String,
    /// `offline`, `reconciling` or `ready`.
    pub machine_status: String,
    /// `claude` or `codex`.
    pub agent: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub agent_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub branch: Option<String>,
    /// `starting`, `running`, `stopping`, `stopped`, `exited`, `failed` or `deleted`.
    pub lifecycle: String,
    /// Herdr's view: `idle`, `working`, `blocked`, `done` or `unknown`.
    pub agent_status: String,
    /// Stuck at a dialog the runner won't answer; answer it in Herdr.
    pub hook_failed: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    pub revision: i64,
    pub created_at: i64,
    pub updated_at: i64,
    pub muted: bool,
    /// False when muted or when mute-all is on.
    pub notifies: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub start_commit: Option<String>,
    /// The Herdr session it runs in; `herdr session attach <name>` opens it on the machine.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub herdr_session: Option<String>,
    pub pending_approvals: Vec<Approval>,
}

impl From<SessionView> for Session {
    fn from(s: SessionView) -> Self {
        Self {
            label: crate::hub::route::label(&s.project_id, &s.id),
            machine_status: enum_str(&s.machine),
            lifecycle: s.lifecycle.as_str().to_owned(),
            pending_approvals: s.pending_approvals.into_iter().map(Into::into).collect(),
            id: s.id,
            project_id: s.project_id,
            machine_id: s.machine_id,
            agent: s.agent,
            agent_name: s.agent_name,
            branch: s.branch,
            agent_status: s.agent_status,
            hook_failed: s.hook_failed,
            error: s.error,
            revision: s.revision,
            created_at: s.created_at,
            updated_at: s.updated_at,
            muted: s.muted,
            notifies: s.notifies,
            start_commit: s.start_commit,
            herdr_session: s.herdr_session,
        }
    }
}

#[derive(Debug, Serialize, ToSchema)]
pub struct Sessions {
    /// Pass back as `after_revision` to wait for the next change.
    pub revision: i64,
    pub sessions: Vec<Session>,
}

impl From<SessionList> for Sessions {
    fn from(l: SessionList) -> Self {
        Self {
            revision: l.revision,
            sessions: l.sessions.into_iter().map(Into::into).collect(),
        }
    }
}

#[derive(Debug, Deserialize, ToSchema)]
pub struct StartRequest {
    pub project_id: String,
    pub prompt: String,
    /// Defaults to the hub's default machine.
    #[serde(default)]
    pub machine_id: Option<String>,
    /// `claude` or `codex`; defaults to the project's agent.
    #[serde(default)]
    pub agent: Option<String>,
    #[serde(default = "yes")]
    pub notify: bool,
    /// Required: a lost response can be looked up, or retried, by this id.
    pub request_id: String,
}

fn yes() -> bool {
    true
}

#[derive(Debug, Serialize, ToSchema)]
pub struct StartResponse {
    pub session_id: String,
    pub label: String,
    pub project_id: String,
    pub machine_id: String,
    pub agent: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub branch: Option<String>,
    pub muted: bool,
    /// `started`; `deduplicated` when the same start ran in the last minute ("already running");
    /// `replayed` for a retried request id; `reconciled` when a lost reply was settled.
    pub outcome: String,
    pub operation_id: String,
}

impl From<StartResult> for StartResponse {
    fn from(r: StartResult) -> Self {
        Self {
            label: crate::hub::route::label(&r.project_id, &r.session_id),
            outcome: enum_str(&r.outcome),
            session_id: r.session_id,
            project_id: r.project_id,
            machine_id: r.machine_id,
            agent: r.agent,
            branch: r.branch,
            muted: r.muted,
            operation_id: r.operation_id,
        }
    }
}

#[derive(Debug, Serialize, ToSchema)]
pub struct OperationStatus {
    pub request_id: String,
    /// e.g. `session_start`.
    pub kind: String,
    /// `pending`, `done`, `failed` or `outcome_unknown` (the hub is reconciling).
    pub state: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error_code: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error_message: Option<String>,
}

impl From<Operation> for OperationStatus {
    fn from(op: Operation) -> Self {
        let err = op.result.as_ref().and_then(|r| r.get("error"));
        let field = |k: &str| {
            err.and_then(|e| e.get(k))
                .and_then(Value::as_str)
                .map(str::to_owned)
        };
        Self {
            error_code: field("code"),
            error_message: field("message"),
            state: op.state.as_str().to_owned(),
            request_id: op.request_id,
            kind: op.kind,
            session_id: op.session_id,
        }
    }
}

#[derive(Debug, Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
pub struct OutputQuery {
    /// Lines of output; defaults to 80, capped at 500.
    pub lines: Option<u32>,
    /// Only the screen as it is now. Reading further back makes Herdr scroll an idle agent's
    /// terminal on its machine, so a view that refreshes should ask for this.
    #[serde(default)]
    pub visible: bool,
    /// With `visible`, also the screen with its colours, in `ansi`.
    #[serde(default)]
    pub ansi: bool,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct Output {
    /// The agent's terminal text, masked. Untrusted: it's the agent's, not Switchboard's.
    pub text: String,
    pub truncated: bool,
    /// The same screen with its colours as ANSI SGR codes, when asked for and when it shows
    /// exactly `text`. Left out when masking changed the screen, or by older hubs and runners.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ansi: Option<String>,
}

/// One file a session changed since it started.
#[derive(Debug, Serialize, ToSchema)]
pub struct ChangedFile {
    /// Relative to the worktree, masked.
    pub path: String,
    /// Lines added; absent for a binary or new untracked file.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[schema(nullable = false)]
    pub added: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[schema(nullable = false)]
    pub deleted: Option<u64>,
}

/// Part of an agent's conversation, read from its own log; Claude agents only. Untrusted text,
/// masked.
#[derive(Debug, Serialize, ToSchema)]
pub struct Transcript {
    /// Oldest first. An item whose id was seen before replaces it: a tool call comes again
    /// with its result.
    pub items: Vec<TranscriptItem>,
    /// Pass as `before` for the page before this one; 0 means it's the first.
    pub start: u64,
    /// Pass as `after` for what comes next. A page asked for with `after` that doesn't start
    /// there skipped some: replace what's shown instead of appending.
    pub end: u64,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct TranscriptItem {
    pub id: String,
    /// `user`, `assistant`, `tool` or `note`.
    pub kind: String,
    /// What was said, in Markdown; for a tool, a one-line summary. Absent on a tool result
    /// whose call is on an earlier page.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
    /// The tool's name, e.g. `Bash` or `mcp__server__tool`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool: Option<String>,
    /// The tool's input as JSON, shortened.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub input: Option<String>,
    /// The tool's result, shortened.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub output: Option<String>,
    /// The tool's result has arrived.
    pub done: bool,
    pub failed: bool,
    /// When it was written, RFC 3339.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub at: Option<String>,
    /// Lines an edit adds and removes, when its input says. Older hubs leave them out.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub added: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub removed: Option<u32>,
    /// The reply the agent ended its turn with: its answer, even when work follows that something
    /// else started. Older hubs leave it out.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub ends_turn: bool,
}

impl From<TranscriptPage> for Transcript {
    fn from(p: TranscriptPage) -> Self {
        Self {
            items: p
                .items
                .into_iter()
                .map(|i| TranscriptItem {
                    id: i.id,
                    kind: i.kind,
                    text: i.text,
                    tool: i.tool,
                    input: i.input,
                    output: i.output,
                    done: i.done,
                    failed: i.failed,
                    at: i.at,
                    added: i.added,
                    removed: i.removed,
                    ends_turn: i.ends_turn,
                })
                .collect(),
            start: p.start,
            end: p.end,
        }
    }
}

#[derive(Debug, Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
pub struct TranscriptQuery {
    /// Continue from an earlier page's `end`.
    pub after: Option<u64>,
    /// Go back from an earlier page's `start`.
    pub before: Option<u64>,
}

#[derive(Debug, Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
pub struct AgentTranscriptQuery {
    pub machine_id: String,
    pub agent: String,
    /// Continue from an earlier page's `end`.
    pub after: Option<u64>,
    /// Go back from an earlier page's `start`.
    pub before: Option<u64>,
}

#[derive(Debug, Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
pub struct AgentQuery {
    pub machine_id: String,
    pub agent: String,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct Skills {
    pub skills: Vec<Skill>,
}

/// A skill or command, as typed: `/name`, or `$name` for a Codex skill.
#[derive(Debug, Serialize, ToSchema)]
pub struct Skill {
    pub name: String,
    /// One line. Untrusted text.
    pub description: String,
    /// `repo`, `user`, `plugin` or `builtin`.
    pub source: String,
}

impl From<Vec<crate::proto::SkillInfo>> for Skills {
    fn from(list: Vec<crate::proto::SkillInfo>) -> Self {
        Self {
            skills: list
                .into_iter()
                .map(|s| Skill {
                    name: s.name,
                    description: s.description,
                    source: s.source,
                })
                .collect(),
        }
    }
}

#[derive(Debug, Serialize, ToSchema)]
pub struct Changes {
    pub files: Vec<ChangedFile>,
    /// More files changed than are listed.
    pub truncated: bool,
}

impl From<ChangesResult> for Changes {
    fn from(c: ChangesResult) -> Self {
        Self {
            files: c
                .files
                .into_iter()
                .map(|f| ChangedFile {
                    path: f.path,
                    added: f.added,
                    deleted: f.deleted,
                })
                .collect(),
            truncated: c.truncated,
        }
    }
}

#[derive(Debug, Deserialize, ToSchema)]
pub struct PromptRequest {
    pub text: String,
    #[serde(default)]
    pub request_id: Option<String>,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct PromptResponse {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub agent_status: Option<String>,
}

impl From<PromptResult> for PromptResponse {
    fn from(p: PromptResult) -> Self {
        Self {
            agent_status: p.agent_status,
        }
    }
}

/// Something a message can go to: a session by id, or an agent Switchboard didn't start by
/// machine and name.
#[derive(Debug, Clone, Default, Deserialize, ToSchema)]
pub struct TargetRef {
    #[serde(default)]
    pub session_id: Option<String>,
    #[serde(default)]
    pub machine_id: Option<String>,
    #[serde(default)]
    pub agent: Option<String>,
}

#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct TargetView {
    /// `session` or `agent` (one Switchboard didn't start).
    pub kind: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,
    pub machine_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub agent: Option<String>,
    pub label: String,
}

impl From<&Target> for TargetView {
    fn from(t: &Target) -> Self {
        match t {
            Target::Session {
                id,
                label,
                machine,
                agent,
            } => Self {
                kind: "session".into(),
                session_id: Some(id.clone()),
                machine_id: machine.clone(),
                agent: agent.clone(),
                label: label.clone(),
            },
            Target::Agent { machine, name } => Self {
                kind: "agent".into(),
                session_id: None,
                machine_id: machine.clone(),
                agent: Some(name.clone()),
                label: name.clone(),
            },
        }
    }
}

#[derive(Debug, Deserialize, ToSchema)]
pub struct MessageRequest {
    pub text: String,
    /// Required, one per draft: a retried send with the same id is not sent twice.
    pub request_id: String,
    /// Where text that names no project goes.
    #[serde(default)]
    #[schema(nullable = false)]
    pub current: Option<TargetRef>,
    /// Answers a `choose_project`: start in this project.
    #[serde(default)]
    pub project_id: Option<String>,
    /// Answers a `choose_target`: send to this one.
    #[serde(default)]
    #[schema(nullable = false)]
    pub target: Option<TargetRef>,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct MessageChoice {
    pub label: String,
    /// Set for a project to start in.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub project_id: Option<String>,
    /// Set for something running to send to.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[schema(nullable = false)]
    pub target: Option<TargetView>,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct MessageResponse {
    /// `started`, `sent`, `selected`, `choose_project`, `choose_target` or `refused`.
    pub outcome: String,
    /// What happened, or the question to answer, in a sentence.
    pub message: String,
    /// Where text that names no project goes from now on.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[schema(nullable = false)]
    pub current: Option<TargetView>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[schema(nullable = false)]
    pub start: Option<StartResponse>,
    pub options: Vec<MessageChoice>,
}

#[derive(Debug, Deserialize, ToSchema)]
pub struct MessagePreviewRequest {
    pub text: String,
    /// Where text that names no project goes.
    #[serde(default)]
    #[schema(nullable = false)]
    pub current: Option<TargetRef>,
}

/// Where a message would go, without sending it.
#[derive(Debug, Serialize, ToSchema)]
pub struct MessagePreview {
    /// `start`, `send`, `select`, `choose_project`, `choose_target` or `refused`.
    pub outcome: String,
    /// What sending would do, or the question sending would ask, in a sentence.
    pub message: String,
    /// For `send` and `select`.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[schema(nullable = false)]
    pub target: Option<TargetView>,
    /// For `start`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub project_id: Option<String>,
    /// For `start`: the machine it would run on.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub machine_id: Option<String>,
    /// For `choose_project` and `choose_target`.
    pub options: Vec<MessageChoice>,
}

/// A Herdr agent Switchboard didn't start. It has no permission hooks, so its dialogs
/// are answered in Herdr.
#[derive(Debug, Serialize, ToSchema)]
pub struct AgentView {
    pub machine_id: String,
    /// `<herdr session>/<agent>` when the machine runs several Herdr sessions. An agent without
    /// a Herdr name goes by its pane id, e.g. `mono1/w1:p2`.
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub herdr_session: Option<String>,
    /// e.g. `claude` or `codex`, as Herdr reports it.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub kind: Option<String>,
    /// `idle`, `working`, `blocked`, `done` or `unknown`.
    pub agent_status: String,
    /// Its terminal title, usually what it's working on. Untrusted text.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct Agents {
    pub agents: Vec<AgentView>,
    /// Agents Switchboard didn't start that ended in the last day, latest first. Older hubs
    /// leave it out.
    #[schema(required = false)]
    pub ended: Vec<EndedAgent>,
}

/// An agent that has left its pane. Sessions Switchboard started keep their own records.
#[derive(Debug, Serialize, ToSchema)]
pub struct EndedAgent {
    pub machine_id: String,
    /// Its name when it ended, as in `AgentView`.
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub herdr_session: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub kind: Option<String>,
    /// Its last terminal title. Untrusted text.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    /// When the hub first saw it, in milliseconds.
    pub started_at: i64,
    pub ended_at: i64,
}

impl From<SeenAgentRecord> for EndedAgent {
    fn from(a: SeenAgentRecord) -> Self {
        Self {
            machine_id: a.machine_id,
            name: a.name,
            herdr_session: a.herdr_session,
            kind: a.kind,
            title: a.title,
            started_at: a.first_seen,
            ended_at: a.ended_at.unwrap_or(a.last_seen),
        }
    }
}

#[derive(Debug, Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
pub struct AgentOutputQuery {
    pub machine_id: String,
    pub agent: String,
    /// Lines of output; defaults to 80, capped at 500.
    pub lines: Option<u32>,
    /// Only the screen as it is now. Reading further back makes Herdr scroll an idle agent's
    /// terminal on its machine, so a view that refreshes should ask for this.
    #[serde(default)]
    pub visible: bool,
    /// With `visible`, also the screen with its colours, in `ansi`.
    #[serde(default)]
    pub ansi: bool,
}

#[derive(Debug, Default, Deserialize, ToSchema)]
pub struct StopRequest {
    /// Removes the worktree only if it is clean and merged.
    #[serde(default)]
    pub remove_worktree_if_clean: bool,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct StopResponse {
    pub worktree_removed: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub kept_because: Option<String>,
    pub already_stopped: bool,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct DeleteResponse {
    pub worktree_removed: bool,
    /// Why the worktree was kept: uncommitted or unmerged work, or a failed removal.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[schema(nullable = false)]
    pub kept_because: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[schema(nullable = false)]
    pub branch: Option<String>,
    pub already_deleted: bool,
}

impl From<DeleteResult> for DeleteResponse {
    fn from(d: DeleteResult) -> Self {
        Self {
            worktree_removed: d.worktree_removed,
            kept_because: d.kept_because,
            branch: d.branch,
            already_deleted: d.already_deleted,
        }
    }
}

impl From<StopResult> for StopResponse {
    fn from(s: StopResult) -> Self {
        Self {
            worktree_removed: s.worktree_removed,
            kept_because: s.kept_because,
            already_stopped: s.already_stopped,
        }
    }
}

#[derive(Debug, Deserialize, ToSchema)]
pub struct MuteRequest {
    pub muted: bool,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct MuteResponse {
    pub muted: bool,
    pub notifies: bool,
}

// Approvals.

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, ToSchema)]
#[serde(rename_all = "lowercase")]
pub enum DecisionKind {
    Allow,
    Deny,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct ApprovalChallenge {
    pub nonce: String,
    pub expires_at: i64,
    pub message_version: u32,
    pub hub_id: String,
    pub device_id: String,
    pub approval_id: String,
}

#[derive(Debug, Deserialize, ToSchema)]
pub struct DecisionRequest {
    pub decision: DecisionKind,
    /// Required to allow: the nonce from the approval's challenge.
    #[serde(default)]
    pub nonce: Option<String>,
    /// Required to allow: DER ECDSA P-256 over SHA-256 of the approval message, base64url,
    /// made with the approval key. The message is `switchboard-approval-v1`, then `hub=`,
    /// `device=`, `approval=`, `decision=allow`, `nonce=`, `expires=` lines, joined by `\n`.
    #[serde(default)]
    pub signature: Option<String>,
    #[serde(default)]
    pub message_version: Option<u32>,
    #[serde(default)]
    pub request_id: Option<String>,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct DecisionResponse {
    pub session_id: String,
    pub approval_id: String,
    /// The recorded outcome, `allow` or `deny`. An allow that never reached the agent is a deny.
    pub outcome: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    /// `delivered`, `not_delivered` or `unknown` (an allow may have run).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub delivery: Option<String>,
    /// Already decided; nothing was sent again.
    pub replayed: bool,
}

impl From<DecideResult> for DecisionResponse {
    fn from(d: DecideResult) -> Self {
        Self {
            session_id: d.session_id,
            approval_id: d.approval_id,
            outcome: d.outcome.as_str().to_owned(),
            reason: d.reason,
            delivery: d.delivery.map(|d| d.as_str().to_owned()),
            replayed: d.replayed,
        }
    }
}

// Devices.

#[derive(Debug, Serialize, ToSchema)]
pub struct Device {
    pub id: String,
    pub name: String,
    /// `pending`, `active`, `rejected` or `revoked`.
    pub state: String,
    pub fingerprint: String,
    pub created_at: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_seen_at: Option<i64>,
    /// The device making this request.
    pub this_device: bool,
    pub push_enabled: bool,
}

impl Device {
    pub fn of(d: DeviceRecord, me: &str) -> Self {
        Self {
            this_device: d.id == me,
            push_enabled: d.push_token.is_some(),
            state: d.state.as_str().to_owned(),
            id: d.id,
            name: d.name,
            fingerprint: d.fingerprint,
            created_at: d.created_at,
            last_seen_at: d.last_seen_at,
        }
    }
}

#[derive(Debug, Clone, Copy, Deserialize, ToSchema)]
#[serde(rename_all = "lowercase")]
pub enum PushEnvironment {
    Sandbox,
    Production,
}

#[derive(Debug, Deserialize, ToSchema)]
pub struct PushTokenRequest {
    /// The APNs device token as hex; null turns push off for this device.
    #[serde(default)]
    pub token: Option<String>,
    pub environment: PushEnvironment,
}

fn enum_str<T: Serialize>(v: &T) -> String {
    serde_json::to_value(v)
        .ok()
        .and_then(|v| v.as_str().map(str::to_owned))
        .unwrap_or_else(|| "unknown".into())
}
