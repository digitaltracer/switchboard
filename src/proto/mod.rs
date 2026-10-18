//! Hub-runner protocol: JSON text frames over WebSocket.

pub mod auth;
pub mod conn;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::cli::Agent;

pub const PROTOCOL_VERSION: u32 = 1;
pub const MAX_FRAME_BYTES: usize = 1 << 20;

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
    },
    Prompt {
        session_id: String,
        text: String,
    },
    Decide {
        session_id: String,
        approval_id: String,
        allow: bool,
    },
    Answer {
        session_id: String,
        dialog_id: String,
        option: String,
    },
    Read {
        session_id: String,
        lines: u32,
    },
    Stop {
        session_id: String,
        remove_worktree_if_clean: bool,
    },
    /// Ask for the runner's view of its sessions, for reconciliation.
    ListSessions,
}

impl Op {
    /// Mutating ops in flight on a lost connection become `outcome_unknown`.
    pub fn is_mutating(&self) -> bool {
        !matches!(self, Op::Read { .. } | Op::ListSessions)
    }
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
    DeadlineExceeded,
    RunnerDisconnected,
    Busy,
    NotFound,
    AgentBlocked,
    HooksUntrusted,
    /// The agent's startup screen was not recognised or not cleared; the prompt was not sent.
    StartupFailed,
    UnsupportedAgentVersion,
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
