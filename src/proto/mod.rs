//! Hub-runner protocol: JSON text frames over WebSocket.

pub mod auth;
pub mod conn;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::cli::Agent;

pub const PROTOCOL_VERSION: u32 = 1;
pub const MAX_FRAME_BYTES: usize = 1 << 20;
/// A runner that answers `Op::Activity`.
pub const CAP_ACTIVITY: &str = "activity";
/// The runner answers `Op::Diff`.
pub const CAP_DIFF: &str = "diff";
/// The runner answers `Op::Summarize`.
pub const CAP_SUMMARY: &str = "summary";
/// A runner that hosts herdr-core agents, named `hc/<agent>`.
pub const CAP_HC: &str = "hc";
/// The `herdr_session` of a herdr-core agent or session.
pub const CORE_SERVER: &str = "hc";
/// A runner that takes `backend` on `Op::StartSession` and answers `Op::StartOptions`.
pub const CAP_START_BACKEND: &str = "start_backend";
/// A runner that lists `space_id` and `can_rename`, takes `space` on `Op::StartSession`, and
/// answers `Op::RenameAgent`, `Op::RenameSpace` and `Op::RenameSessionTab`.
pub const CAP_MANAGE: &str = "manage";

/// Where one session starts: a Herdr TUI session of its own, or the Herdr app (herdr-core).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum StartBackend {
    Herdr,
    Hc,
}

impl StartBackend {
    pub fn as_str(self) -> &'static str {
        match self {
            StartBackend::Herdr => "herdr",
            StartBackend::Hc => "hc",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "herdr" => Some(StartBackend::Herdr),
            "hc" => Some(StartBackend::Hc),
            _ => None,
        }
    }
}

/// `Op::StartOptions`'s answer: where a start with no backend would go, and what can be chosen.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StartOptions {
    pub default: StartBackend,
    pub backends: Vec<BackendOption>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BackendOption {
    pub backend: StartBackend,
    pub available: bool,
    /// Why it isn't available: `not_answering` (no herdr-core answers) or `not_followed` (this
    /// runner follows no herdr-core).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Frame {
    /// Runner's first frame on every connection.
    Hello(Hello),
    /// Hub to runner.
    Req(Req),
    /// Runner to hub, answering a `Req` by id.
    Resp(Resp),
    /// Runner to hub, unsolicited.
    Event(Event),
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Hello {
    pub machine_id: String,
    pub protocol_version: u32,
    pub runner_version: String,
    pub herdr_version: Option<String>,
    /// Workspaces this runner owns, each labelled with its session id.
    pub sessions: Vec<OwnedSession>,
    /// Ops beyond the first protocol this runner answers, e.g. `activity`. Older runners send
    /// none, and are never sent an op they can't decode.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub capabilities: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct OwnedSession {
    pub session_id: String,
    pub workspace_id: String,
    pub pane_id: Option<String>,
    pub agent_status: String,
    /// Stuck at a dialog the runner would not answer; the session status shows `hook_failed`.
    #[serde(default)]
    pub hook_failed: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Req {
    pub id: u64,
    /// Relative to receipt, so clock drift between machines doesn't matter.
    pub deadline_ms: u64,
    pub op: Op,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum Op {
    StartSession {
        session_id: String,
        project_id: String,
        repo_path: String,
        branch: String,
        agent: Agent,
        prompt: String,
        deny_patterns: Vec<String>,
        /// Overrides the runner's `new_sessions` for this start; only sent to runners advertising
        /// `CAP_START_BACKEND`. A backend the runner can't serve fails the start.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        backend: Option<StartBackend>,
        /// The Herdr app space the session's tab opens in, by id, instead of a new space; only
        /// sent to runners advertising `CAP_MANAGE`, and only for the Herdr app.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        space: Option<String>,
    },
    Prompt {
        session_id: String,
        text: String,
    },
    Decide {
        session_id: String,
        approval_id: String,
        allow: bool,
        /// For an allowed question, the chosen option ids; see `agents::claude::answers`.
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        choices: Vec<String>,
    },
    /// Picks one listed option of the dialog on an agent's screen, by keys. `occurrence` and
    /// `option` are the opaque ids `Dialog` gave; `label_sha256` is the masked label's hash.
    /// With `deny_only`, any option but the dialog's deny is refused.
    Answer {
        agent: String,
        occurrence: String,
        option: String,
        label_sha256: String,
        #[serde(default)]
        deny_only: bool,
    },
    /// Shows the previous question of a group again, by one left arrow. It submits nothing.
    Back {
        agent: String,
        occurrence: String,
    },
    Read {
        session_id: String,
        lines: u32,
    },
    Stop {
        session_id: String,
        remove_worktree_if_clean: bool,
    },
    /// Stops the session if it's still running, removes its worktree if clean and merged, then
    /// deletes its Herdr session. The hub sends what it recorded, since a stopped session is no
    /// longer the runner's.
    Delete {
        session_id: String,
        herdr_session: Option<String>,
        repo_path: Option<String>,
        worktree_path: Option<String>,
        branch: Option<String>,
    },
    /// Ask for the runner's view of its sessions, for reconciliation.
    ListSessions,
    /// Every named Herdr agent on the machine, Switchboard's or not.
    Agents,
    /// Every agent's last line, as `AgentActivity`s; only for runners advertising
    /// `CAP_ACTIVITY`. Read-only.
    Activity,
    /// Where a new session would start, as `StartOptions`; only for runners advertising
    /// `CAP_START_BACKEND`. Read-only.
    StartOptions,
    /// Reads any named agent; `visible` is the current screen, which shows a dialog. `ansi` adds
    /// that screen with its colours; older runners leave it out.
    ReadAgent {
        agent: String,
        lines: u32,
        visible: bool,
        #[serde(default)]
        ansi: bool,
    },
    /// Prompts an agent Switchboard didn't create. It has no hooks, so nothing answers its dialogs.
    PromptAgent {
        agent: String,
        text: String,
    },
    /// Files the session's worktree changed since it started. Read-only.
    Changes {
        session_id: String,
    },
    /// One changed file's unified diff since the session started, a page of hunks from `from`;
    /// only for runners advertising `CAP_DIFF`. Read-only.
    Diff {
        session_id: String,
        path: String,
        #[serde(default)]
        from: u32,
    },
    /// Summarises masked facts with `claude -p` and every tool, setting, hook, plugin and MCP
    /// server off, in an empty folder; only for runners advertising `CAP_SUMMARY`. The owner
    /// turns it on. Returns `{ "summary": "…" }`.
    Summarize {
        prompt: String,
        model: String,
    },
    /// An agent's conversation from its own log, as a `TranscriptPage`. With neither offset,
    /// the latest part; `after` continues from an earlier page's `end`, `before` goes back
    /// from its `start`. Claude agents only. Read-only.
    Transcript {
        agent: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        after: Option<u64>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        before: Option<u64>,
        /// With `after`, wait up to this long, capped at 20 s, for the log to grow past it.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        wait_ms: Option<u64>,
    },
    /// The skills and commands an agent can run, as `SkillInfo`s. Read-only.
    Skills {
        agent: String,
    },
    /// The dialog on an agent's screen, as `{"dialog": DialogView | null, "occurrence": id}`,
    /// where the occurrence names what the screen shows even when no dialog is recognised.
    /// Read-only.
    Dialog {
        agent: String,
    },
    /// Renames an agent Switchboard didn't start: `agent.rename` in a Herdr terminal, the tab's
    /// name in the Herdr app. Returns `{ "name": <as listed> }`. `CAP_MANAGE`.
    RenameAgent {
        agent: String,
        name: String,
    },
    /// Renames a Herdr app space or a Herdr workspace, by the id the listing gave.
    /// Returns `{ "name": … }`. `CAP_MANAGE`.
    RenameSpace {
        #[serde(default)]
        herdr_session: Option<String>,
        space_id: String,
        name: String,
    },
    /// Renames the tab a Herdr app session's terminal sits in, to match the hub's name for it.
    /// Returns `{ "renamed": bool }`; a Herdr terminal session has nothing to rename. `CAP_MANAGE`.
    RenameSessionTab {
        session_id: String,
        name: String,
    },
}

impl Op {
    /// Mutating ops in flight on a lost connection become `outcome_unknown`.
    pub fn is_mutating(&self) -> bool {
        !matches!(
            self,
            Op::Read { .. }
                | Op::ListSessions
                | Op::Agents
                | Op::Activity
                | Op::StartOptions
                | Op::ReadAgent { .. }
                | Op::Changes { .. }
                | Op::Diff { .. }
                | Op::Summarize { .. }
                | Op::Transcript { .. }
                | Op::Skills { .. }
                | Op::Dialog { .. }
        )
    }

    /// The agent an agent op names, if any.
    pub fn agent(&self) -> Option<&str> {
        match self {
            Op::Answer { agent, .. }
            | Op::Back { agent, .. }
            | Op::ReadAgent { agent, .. }
            | Op::PromptAgent { agent, .. }
            | Op::Transcript { agent, .. }
            | Op::Skills { agent }
            | Op::Dialog { agent }
            | Op::RenameAgent { agent, .. } => Some(agent),
            _ => None,
        }
    }

    /// Names a herdr-core agent, which only a runner advertising `CAP_HC` can reach.
    pub fn targets_core(&self) -> bool {
        self.agent()
            .is_some_and(|a| a.split_once('/').is_some_and(|(s, _)| s == CORE_SERVER))
    }
}

/// Part of an agent's conversation, from byte `start` to `end` of its log. A page asked for
/// with `after` whose `start` is later than that skipped some: replace, don't append.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TranscriptPage {
    pub items: Vec<TranscriptItem>,
    pub start: u64,
    pub end: u64,
    /// Messages waiting in the agent's queue as of `end`, oldest first, on pages that reach the
    /// log's end. Older runners leave it out.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub queued: Vec<TranscriptItem>,
}

/// One entry of a conversation. Items with the same id replace each other: a tool call shows
/// up first, then again with its result. Untrusted text.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TranscriptItem {
    pub id: String,
    /// `user`, `assistant`, `tool`, `note`, or `peer` for a message from another terminal.
    pub kind: String,
    /// What was said; for a tool, a one-line summary. None on a result whose call came earlier.
    #[serde(default)]
    pub text: Option<String>,
    #[serde(default)]
    pub tool: Option<String>,
    #[serde(default)]
    pub input: Option<String>,
    #[serde(default)]
    pub output: Option<String>,
    /// A tool's result has arrived.
    #[serde(default)]
    pub done: bool,
    #[serde(default)]
    pub failed: bool,
    /// When it was written, RFC 3339.
    #[serde(default)]
    pub at: Option<String>,
    /// Lines an edit adds and removes, for edits whose input says.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub added: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub removed: Option<u32>,
    /// A reply the agent ended its turn with, as its log says; work after it was started by something else.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub ends_turn: bool,
    /// Who sent a `peer` message: the sending terminal's agent name, else its id.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub from: Option<String>,
}

/// A dialog on an agent's screen as the runner recognised it. Masked; the ids are opaque.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DialogView {
    pub kind: crate::agents::dialog::DialogKind,
    pub prompt: String,
    pub options: Vec<DialogOptionView>,
    /// The option the screen has selected.
    #[serde(default)]
    pub selected: Option<String>,
    /// This showing of the dialog; an answer names it.
    pub occurrence: String,
    /// Keys may answer it now.
    pub answerable: bool,
    /// Why not, when not: `read_only`, `hooked`, `hook_pending`, `hook_failed`,
    /// `not_blocked` or `answered`.
    #[serde(default)]
    pub reason: Option<String>,
    /// A group's questions in order, as tabs; empty for a dialog that stands alone.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub steps: Vec<crate::agents::dialog::DialogStep>,
    /// A left arrow may show the group's previous question again.
    #[serde(default)]
    pub back: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DialogOptionView {
    pub id: String,
    pub label: String,
    /// SHA-256 hex of `label`, which an answer repeats.
    pub label_sha256: String,
    pub is_deny: bool,
    /// May be chosen from the phone; free-text, chat and standing-permission rows may not.
    pub offered: bool,
    /// It answers one question of a group and submits nothing, so it needs no signature.
    #[serde(default)]
    pub provisional: bool,
}

/// One skill or command, as typed: `/name`, or `$name` for a Codex skill.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SkillInfo {
    pub name: String,
    pub description: String,
    /// `repo`, `user`, `plugin` or `builtin`.
    pub source: String,
}

/// One agent from `Op::Agents`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentSummary {
    /// Its Herdr name, or its pane id when it has none.
    pub name: String,
    pub pane_id: String,
    pub kind: Option<String>,
    pub agent_status: String,
    /// Set when the agent is one of this runner's sessions.
    pub session_id: Option<String>,
    /// The Herdr session it runs in, when the runner follows several.
    #[serde(default)]
    pub herdr_session: Option<String>,
    /// Its terminal title. Untrusted text.
    #[serde(default)]
    pub title: Option<String>,
    /// The label of the Herdr workspace it runs in. Untrusted text.
    #[serde(default)]
    pub space: Option<String>,
    /// It stopped on an error (herdr-core only); `agent_status` says done.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub error: bool,
    /// The id of the space it runs in: a Herdr app space in the layout, or a Herdr workspace.
    /// `Op::RenameSpace` and a start's `space` take it. `CAP_MANAGE`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub space_id: Option<String>,
    /// `Op::RenameAgent` can rename it. `CAP_MANAGE`.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub can_rename: bool,
}

/// One agent from `Op::Activity`: the first line of the last thing it said or did, masked.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentActivity {
    /// As in `AgentSummary`.
    pub name: String,
    #[serde(default)]
    pub session_id: Option<String>,
    /// None when its log has none yet. Untrusted text.
    #[serde(default)]
    pub line: Option<String>,
    /// When the line was written, as the log says it.
    #[serde(default)]
    pub at: Option<String>,
}

impl AgentSummary {
    /// Whether it has a Herdr name, rather than going by its pane.
    pub fn named(&self) -> bool {
        self.name.rsplit('/').next() != Some(self.pane_id.as_str())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "outcome", rename_all = "snake_case")]
pub enum AgentPromptOutcome {
    Sent,
    /// Herdr refused because the agent sits at a dialog; nothing was sent.
    Blocked {
        screen: String,
    },
    NotFound,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Resp {
    pub id: u64,
    #[serde(flatten)]
    pub outcome: Outcome,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Outcome {
    Ok(Value),
    Err(ProtoError),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, thiserror::Error)]
#[error("{code:?}: {message}")]
pub struct ProtoError {
    pub code: ErrorCode,
    pub message: String,
}

impl ProtoError {
    pub fn new(code: ErrorCode, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ErrorCode {
    /// The change may or may not have happened; reconcile before retrying.
    OutcomeUnknown,
    /// The start's chosen backend can't take a session on this machine now; nothing was started.
    BackendUnavailable,
    DeadlineExceeded,
    RunnerDisconnected,
    Busy,
    NotFound,
    AgentBlocked,
    HooksUntrusted,
    /// The agent's startup screen was not recognised or not cleared; the prompt was not sent.
    StartupFailed,
    UnsupportedAgentVersion,
    /// The option approves and the caller may only deny.
    ApproveNotAllowed,
    /// The dialog changed, was answered, or isn't one the runner may answer; nothing was sent.
    DialogChanged,
    /// Another agent or space already has the name; nothing was renamed.
    NameTaken,
    Herdr,
    BadRequest,
    Internal,
    #[serde(other)]
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "event", rename_all = "snake_case")]
pub enum Event {
    AgentStatus {
        session_id: String,
        agent_status: String,
        revision: u64,
    },
    ApprovalRequested {
        session_id: String,
        approval_id: String,
        tool_name: String,
        tool_input: Value,
        deadline_ms: u64,
    },
    /// The approval can no longer be decided: the hook helper went away or the deadline passed.
    ApprovalClosed {
        session_id: String,
        approval_id: String,
        reason: String,
    },
    /// The session is stuck at a dialog the runner will not answer; surfaced as `hook_failed`.
    HookFailed {
        session_id: String,
        reason: String,
        screen_excerpt: Option<String>,
    },
    /// Something the runner decided on its own, for the hub's audit log.
    Audit {
        session_id: String,
        kind: String,
        detail: Value,
    },
    SessionExited {
        session_id: String,
    },
    /// The runner lost Herdr events and re-synced from a snapshot.
    Resynced {
        sessions: Vec<OwnedSession>,
    },
    /// A named agent's status, sent on every sighting; the bridge debounces it.
    NamedAgentStatus {
        agent_name: String,
        agent_status: String,
    },
    /// An agent has been blocked for the hold, at `occurrence`, with no hook call pending; the
    /// hub queues one `waiting` push for it. Sent once per occurrence.
    Waiting {
        agent_name: String,
        occurrence: String,
        /// The pane incarnation, so a push never resolves to a replacement agent.
        terminal_id: String,
        #[serde(default)]
        herdr_session: Option<String>,
        #[serde(default)]
        session_id: Option<String>,
    },
}

#[derive(Debug, thiserror::Error)]
pub enum CodecError {
    #[error("frame of {0} bytes exceeds the {MAX_FRAME_BYTES} byte limit")]
    TooLarge(usize),
    #[error("malformed frame: {0}")]
    Malformed(#[from] serde_json::Error),
    #[error("runner speaks protocol {got}, hub speaks {PROTOCOL_VERSION}")]
    VersionMismatch { got: u32 },
    #[error("expected hello as the first frame")]
    ExpectedHello,
    #[error("hello for machine {got:?} on a connection authenticated as {expected:?}")]
    WrongMachine { expected: String, got: String },
}

pub fn encode(frame: &Frame) -> String {
    serde_json::to_string(frame).expect("frames always serialize")
}

pub fn decode(text: &str) -> Result<Frame, CodecError> {
    if text.len() > MAX_FRAME_BYTES {
        return Err(CodecError::TooLarge(text.len()));
    }
    Ok(serde_json::from_str(text)?)
}

/// Validates the first frame of a connection authenticated as `machine_id`.
pub fn accept_hello(text: &str, machine_id: &str) -> Result<Hello, CodecError> {
    let Frame::Hello(hello) = decode(text)? else {
        return Err(CodecError::ExpectedHello);
    };
    if hello.protocol_version != PROTOCOL_VERSION {
        return Err(CodecError::VersionMismatch {
            got: hello.protocol_version,
        });
    }
    if hello.machine_id != machine_id {
        return Err(CodecError::WrongMachine {
            expected: machine_id.to_owned(),
            got: hello.machine_id,
        });
    }
    Ok(hello)
}

#[cfg(test)]
mod tests;
