//! A scripted runner for the app's UI tests: it connects to a hub like a real one, accepts
//! starts, and answers reads with canned output, without Herdr or any agent. Each started
//! session reports working, then asks to run `cargo test --workspace`.
//!
//! Usage: cargo run --example fake_runner -- <hub ws url> <machine id> <token>

use std::sync::Arc;
use std::time::Duration;

use serde_json::{Value, json};
use switchboard::proto::{ErrorCode, Event, Op, OwnedSession, ProtoError};
use switchboard::runner::link::{self, Handler, LinkConfig};
use tokio::sync::mpsc;

const OUTPUT: &str = "› fix the flaky retry test
Reading crates/core/src/retry.rs and its test.
The retry test sleeps for a fixed 50 ms, so it races the scheduler on a busy machine.
Pausing tokio's clock makes it deterministic.
● Update  crates/core/src/retry.rs
  +6 −2
● Bash  cargo test -p kraft-core
  Compiling kraft-core v0.4.1";

struct Fake {
    events: mpsc::Sender<Event>,
}

impl Handler for Fake {
    async fn handle(&self, op: Op) -> Result<Value, ProtoError> {
        match op {
            Op::StartSession {
                session_id, branch, ..
            } => {
                let events = self.events.clone();
                let id = session_id.clone();
                tokio::spawn(async move {
                    tokio::time::sleep(Duration::from_secs(1)).await;
                    let _ = events
                        .send(Event::AgentStatus {
                            session_id: id.clone(),
                            agent_status: "working".into(),
                            revision: 1,
                        })
                        .await;
                    tokio::time::sleep(Duration::from_secs(2)).await;
                    let _ = events
                        .send(Event::ApprovalRequested {
                            session_id: id,
                            approval_id: format!("a-{}", rand_suffix()),
                            tool_name: "Bash".into(),
                            tool_input: json!({ "command": "cargo test --workspace" }),
                            deadline_ms: 280_000,
                        })
                        .await;
                });
                Ok(
                    json!({ "session_id": session_id, "branch": branch, "worktree_path": "/tmp/fake-worktree",
                           "workspace_id": "w1", "pane_id": "w1:p1" }),
                )
            }
            Op::Read { .. } => Ok(json!({ "text": OUTPUT, "truncated": false, "untrusted": true })),
            Op::ReadAgent { .. } => Ok(json!({ "text": OUTPUT, "untrusted": true })),
            Op::Transcript {
                after: Some(end), ..
            } => Ok(json!({ "items": [], "start": end, "end": end })),
            Op::Transcript { .. } => Ok(json!({ "start": 0, "end": 4096, "items": [
                { "id": "u1", "kind": "user", "text": "fix the flaky retry test", "done": false, "failed": false },
                { "id": "a1", "kind": "assistant", "done": false, "failed": false, "text":
                  "The retry test sleeps for a fixed **50 ms**, so it races the scheduler on a busy machine.\n\n- Pause tokio's clock\n- Advance it by hand" },
                { "id": "t1", "kind": "tool", "tool": "Edit", "text": "Edited crates/core/src/retry.rs",
                  "input": "{ \"file_path\": \"crates/core/src/retry.rs\" }", "output": "+6 −2", "done": true, "failed": false },
                { "id": "t2", "kind": "tool", "tool": "Bash", "text": "Ran cargo test -p kraft-core",
                  "input": "{ \"command\": \"cargo test -p kraft-core\" }", "done": false, "failed": false },
            ] })),
            Op::Changes { .. } => Ok(json!({ "files": [
                { "path": "crates/core/src/retry.rs", "added": 6, "deleted": 2 },
                { "path": "crates/core/tests/retry.rs", "added": 14, "deleted": 0 },
                { "path": "notes/flaky.md", "added": null, "deleted": null },
            ], "truncated": false })),
            Op::Prompt { .. } => Ok(json!({ "agent_status": "working" })),
            Op::Decide { .. } => Ok(json!({ "applied": true })),
            Op::Stop { .. } => Ok(json!({ "worktree_removed": false })),
            Op::Delete { .. } => Ok(
                json!({ "worktree_removed": false, "kept_because": "unmerged", "herdr_session_deleted": true }),
            ),
            Op::ListSessions => Ok(json!([])),
            Op::Agents => Ok(json!([
                {"name": "crm/orchestrator", "pane_id": "w1:p1", "kind": "claude",
                 "agent_status": "working", "session_id": null, "herdr_session": "crm"},
                {"name": "crm/w1:p2", "pane_id": "w1:p2", "kind": "codex", "agent_status": "idle",
                 "session_id": null, "herdr_session": "crm", "title": "Review the import"}
            ])),
            other => Err(ProtoError::new(
                ErrorCode::BadRequest,
                format!("the fake runner doesn't do {other:?}"),
            )),
        }
    }

    async fn owned(&self) -> Vec<OwnedSession> {
        vec![]
    }
}

fn rand_suffix() -> u128 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos()
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let args: Vec<String> = std::env::args().collect();
    let [_, url, machine, token] = args.as_slice() else {
        anyhow::bail!("usage: fake_runner <hub ws url> <machine id> <token>");
    };
    let (tx, rx) = mpsc::channel(64);
    let cfg = LinkConfig {
        url: url.parse()?,
        token: token.clone(),
        machine_id: machine.clone(),
        herdr_version: None,
    };
    link::run(cfg, Arc::new(Fake { events: tx }), rx).await;
    Ok(())
}
