//! Hand-written serde types for the Herdr methods Switchboard uses (protocol 22).
//! Unknown response fields are ignored; unknown statuses and result types map to fallbacks.

use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentStatus {
    Idle,
    Working,
    Blocked,
    Done,
    #[serde(other)]
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReadSource {
    Visible,
    Recent,
    RecentUnwrapped,
    Detection,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReadFormat {
    Text,
    Ansi,
}

// Requests.

/// Every method Switchboard sends. Serializes as `"method"` plus `"params"`.
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "method", content = "params")]
pub enum Method {
    #[serde(rename = "ping")]
    Ping(Empty),
    #[serde(rename = "session.snapshot")]
    SessionSnapshot(Empty),
    #[serde(rename = "worktree.create")]
    WorktreeCreate(WorktreeCreateParams),
    #[serde(rename = "worktree.list")]
    WorktreeList(WorktreeListParams),
    #[serde(rename = "worktree.remove")]
    WorktreeRemove(WorktreeRemoveParams),
    #[serde(rename = "agent.start")]
    AgentStart(AgentStartParams),
    #[serde(rename = "agent.prompt")]
    AgentPrompt(AgentPromptParams),
    #[serde(rename = "agent.wait")]
    AgentWait(AgentWaitParams),
    #[serde(rename = "agent.read")]
    AgentRead(AgentReadParams),
    #[serde(rename = "agent.send_keys")]
    AgentSendKeys(AgentSendKeysParams),
    #[serde(rename = "workspace.close")]
    WorkspaceClose(WorkspaceCloseParams),
    #[serde(rename = "events.subscribe")]
    EventsSubscribe(EventsSubscribeParams),
}

impl Method {
    pub fn name(&self) -> &'static str {
        match self {
            Method::Ping(_) => "ping",
            Method::SessionSnapshot(_) => "session.snapshot",
            Method::WorktreeCreate(_) => "worktree.create",
            Method::WorktreeList(_) => "worktree.list",
            Method::WorktreeRemove(_) => "worktree.remove",
            Method::AgentStart(_) => "agent.start",
            Method::AgentPrompt(_) => "agent.prompt",
            Method::AgentWait(_) => "agent.wait",
            Method::AgentRead(_) => "agent.read",
            Method::AgentSendKeys(_) => "agent.send_keys",
            Method::WorkspaceClose(_) => "workspace.close",
            Method::EventsSubscribe(_) => "events.subscribe",
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct Request<'a> {
    pub id: String,
    #[serde(flatten)]
    pub method: &'a Method,
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct Empty {}

#[derive(Debug, Clone, Default, Serialize)]
pub struct WorktreeCreateParams {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cwd: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub branch: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub base: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    pub focus: bool,
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct WorktreeListParams {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cwd: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub workspace_id: Option<String>,
}

/// Has no `force` field, so Switchboard cannot force-remove a worktree.
#[derive(Debug, Clone, Serialize)]
pub struct WorktreeRemoveParams {
    pub workspace_id: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct AgentStartParams {
    pub name: String,
    pub kind: String,
    pub pane_id: String,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub args: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub timeout_ms: Option<u64>,
}

#[derive(Debug, Clone, Serialize)]
pub struct AgentPromptParams {
    pub target: String,
    pub text: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub wait: Option<WaitOptions>,
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct WaitOptions {
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub until: Vec<AgentStatus>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub timeout_ms: Option<u64>,
}

#[derive(Debug, Clone, Serialize)]
pub struct AgentWaitParams {
    pub target: String,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub until: Vec<AgentStatus>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub timeout_ms: Option<u64>,
}

#[derive(Debug, Clone, Serialize)]
pub struct AgentReadParams {
    pub target: String,
    pub source: ReadSource,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub lines: Option<u32>,
    pub format: ReadFormat,
    pub strip_ansi: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct AgentSendKeysParams {
    pub target: String,
    pub keys: Vec<String>,
}

/// Has no `close_group` field: Switchboard only closes its own linked workspaces.
#[derive(Debug, Clone, Serialize)]
pub struct WorkspaceCloseParams {
    pub workspace_id: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct EventsSubscribeParams {
    pub subscriptions: Vec<Subscription>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "type")]
pub enum Subscription {
    #[serde(rename = "workspace.created")]
    WorkspaceCreated,
    #[serde(rename = "workspace.closed")]
    WorkspaceClosed,
    #[serde(rename = "worktree.created")]
    WorktreeCreated,
    #[serde(rename = "worktree.removed")]
    WorktreeRemoved,
    #[serde(rename = "pane.created")]
    PaneCreated,
    #[serde(rename = "pane.closed")]
    PaneClosed,
    #[serde(rename = "pane.exited")]
    PaneExited,
    #[serde(rename = "pane.agent_detected")]
    PaneAgentDetected,
    #[serde(rename = "pane.agent_status_changed")]
    PaneAgentStatusChanged {
        pane_id: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        agent_status: Option<AgentStatus>,
    },
}

impl Subscription {
    /// The unscoped lifecycle events Switchboard needs for reconciliation.
    pub fn lifecycle() -> Vec<Subscription> {
        vec![
            Subscription::WorkspaceClosed,
            Subscription::WorktreeRemoved,
            Subscription::PaneClosed,
            Subscription::PaneExited,
            Subscription::PaneAgentDetected,
        ]
    }
}

// Responses.

#[derive(Debug, Clone, Deserialize)]
pub struct Response {
    #[serde(default)]
    pub id: String,
    pub result: Option<ResponseResult>,
    pub error: Option<ErrorBody>,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct ErrorBody {
    pub code: String,
    pub message: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
#[allow(clippy::large_enum_variant)] // parsed once per response, never stored
pub enum ResponseResult {
    Pong {
        version: String,
        protocol: u32,
    },
    SessionSnapshot {
        snapshot: SessionSnapshot,
    },
    WorktreeList {
        source: WorktreeSourceInfo,
        worktrees: Vec<WorktreeInfo>,
    },
    WorktreeCreated {
        workspace: WorkspaceInfo,
        tab: TabInfo,
        root_pane: PaneInfo,
        worktree: WorktreeInfo,
    },
    WorktreeRemoved {
        workspace_id: String,
        path: String,
        forced: bool,
    },
    AgentInfo {
        agent: AgentInfo,
    },
    AgentStarted {
        agent: AgentInfo,
        #[serde(default)]
        argv: Vec<String>,
    },
    AgentPrompted {
        agent: AgentInfo,
    },
    PaneRead {
        read: PaneReadResult,
    },
    SubscriptionStarted,
    Ok,
    #[serde(other)]
    Other,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct WorkspaceInfo {
    pub workspace_id: String,
    pub label: String,
    pub agent_status: AgentStatus,
    #[serde(default)]
    pub worktree: Option<WorkspaceWorktreeInfo>,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct WorkspaceWorktreeInfo {
    pub repo_root: Option<String>,
    pub checkout_path: Option<String>,
    #[serde(default)]
    pub is_linked_worktree: bool,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct TabInfo {
    pub tab_id: String,
    pub workspace_id: String,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct PaneInfo {
    pub pane_id: String,
    pub workspace_id: String,
    pub tab_id: String,
    pub agent_status: AgentStatus,
    pub revision: u64,
    #[serde(default)]
    pub agent: Option<String>,
    #[serde(default)]
    pub cwd: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct AgentInfo {
    pub pane_id: String,
    pub workspace_id: String,
    pub tab_id: String,
    pub agent_status: AgentStatus,
    pub revision: u64,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub agent: Option<String>,
    #[serde(default)]
    pub interactive_ready: bool,
    #[serde(default)]
    pub launch_pending: bool,
    /// The title the agent gives its terminal, usually what it's working on.
    #[serde(default)]
    pub terminal_title_stripped: Option<String>,
    #[serde(default)]
    pub state_change_seq: u64,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct WorktreeInfo {
    pub path: String,
    #[serde(default)]
    pub branch: Option<String>,
    pub label: String,
    pub is_linked_worktree: bool,
    #[serde(default)]
    pub open_workspace_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct WorktreeSourceInfo {
    pub repo_root: String,
    pub source_checkout_path: String,
    #[serde(default)]
    pub source_workspace_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct PaneReadResult {
    pub pane_id: String,
    pub text: String,
    pub revision: u64,
    pub truncated: bool,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct SessionSnapshot {
    pub version: String,
    pub protocol: u32,
    pub workspaces: Vec<WorkspaceInfo>,
    pub panes: Vec<PaneInfo>,
    pub agents: Vec<AgentInfo>,
}

// Events.

/// One pushed line on a subscription connection.
#[derive(Debug, Clone, Deserialize)]
pub struct EventFrame {
    pub event: String,
    pub data: Value,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Event {
    WorkspaceClosed {
        workspace_id: String,
    },
    WorktreeRemoved {
        workspace_id: String,
        path: String,
    },
    PaneClosed {
        pane_id: String,
        workspace_id: String,
    },
    PaneExited {
        pane_id: String,
        workspace_id: String,
    },
    PaneCreated(PaneInfo),
    AgentDetected {
        pane_id: String,
        agent: Option<String>,
    },
    AgentStatusChanged {
        pane_id: String,
        workspace_id: String,
        agent_status: AgentStatus,
    },
    /// An event Switchboard does not track.
    Other(String),
}

impl EventFrame {
    pub fn into_event(self) -> Result<Event, serde_json::Error> {
        #[derive(Deserialize)]
        struct PaneRef {
            pane_id: String,
            workspace_id: String,
        }
        #[derive(Deserialize)]
        struct StatusChange {
            pane_id: String,
            workspace_id: String,
            agent_status: AgentStatus,
        }
        let d = self.data;
        // Lifecycle events use snake_case names; scoped subscription events use dotted names.
        Ok(match self.event.replace('.', "_").as_str() {
            "workspace_closed" => Event::WorkspaceClosed {
                workspace_id: field(&d, "workspace_id")?,
            },
            "worktree_removed" => Event::WorktreeRemoved {
                workspace_id: field(&d, "workspace_id")?,
                path: d
                    .pointer("/worktree/path")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_owned(),
            },
            "pane_closed" => {
                let p: PaneRef = serde_json::from_value(d)?;
                Event::PaneClosed {
                    pane_id: p.pane_id,
                    workspace_id: p.workspace_id,
                }
            }
            "pane_exited" => {
                let p: PaneRef = serde_json::from_value(d)?;
                Event::PaneExited {
                    pane_id: p.pane_id,
                    workspace_id: p.workspace_id,
                }
            }
            "pane_created" => Event::PaneCreated(serde_json::from_value(d["pane"].clone())?),
            "pane_agent_detected" => Event::AgentDetected {
                pane_id: field(&d, "pane_id")?,
                agent: d.get("agent").and_then(Value::as_str).map(str::to_owned),
            },
            "pane_agent_status_changed" => {
                let s: StatusChange = serde_json::from_value(d)?;
                Event::AgentStatusChanged {
                    pane_id: s.pane_id,
                    workspace_id: s.workspace_id,
                    agent_status: s.agent_status,
                }
            }
            _ => Event::Other(self.event),
        })
    }
}

fn field(d: &Value, name: &'static str) -> Result<String, serde_json::Error> {
    d.get(name)
        .and_then(Value::as_str)
        .map(str::to_owned)
        .ok_or_else(|| serde::de::Error::missing_field(name))
}
