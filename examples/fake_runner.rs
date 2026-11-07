//! A scripted runner for the app's UI tests: it connects to a hub like a real one, accepts
//! starts, and answers reads with canned output, without Herdr or any agent. Each started
//! session reports working, then asks to run `cargo test --workspace`. Of the agents it lists,
//! the Claude one waits at a question and the Codex one at a permission dialog, until answered.
//!
//! Usage: cargo run --example fake_runner -- <hub ws url> <machine id> <token>

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use serde_json::{Value, json};
use sha2::{Digest, Sha256};
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
    /// The Codex agent's permission dialog was answered.
    answered: AtomicBool,
}

fn option(id: &str, label: &str, is_deny: bool, offered: bool) -> Value {
    let sha = hex::encode(Sha256::digest(label.as_bytes()));
    json!({ "id": id, "label": label, "label_sha256": sha, "is_deny": is_deny, "offered": offered })
}

fn dialog(agent: &str, answered: bool) -> Value {
    let d = match agent {
        "crm/orchestrator" => json!({
            "kind": "question", "prompt": "Which colour should the banner be?", "occurrence": "occ-q",
            "options": [option("q1", "Red", false, true), option("q2", "Blue", false, true),
                        option("q3", "Type something.", false, false)],
            "selected": "q1", "answerable": true,
        }),
        "crm/w1:p2" if !answered => json!({
            "kind": "permission", "occurrence": "occ-p",
            "prompt": "Would you like to run the following command?\n$ cargo test --workspace",
            "options": [option("p1", "Yes, proceed (y)", false, true),
                        option("p2", "Yes, and don't ask again (p)", false, false),
                        option("p3", "No, and tell Codex what to do differently (esc)", true, true)],
            "selected": "p1", "answerable": true,
        }),
        _ => Value::Null,
    };
    let occurrence = d["occurrence"].as_str().unwrap_or("none").to_owned();
    json!({ "dialog": d, "occurrence": occurrence })
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
            Op::ReadAgent { ansi, .. } => {
                let mut out = json!({ "text": OUTPUT, "untrusted": true });
                if ansi {
                    // The prompt on its shaded band, as Claude draws it.
                    let (first, rest) = OUTPUT.split_once('\n').unwrap_or((OUTPUT, ""));
                    out["ansi"] = json!(format!("\x1b[48;2;55;55;55m{first}\x1b[0m\n{rest}"));
                }
                Ok(out)
            }
            Op::Transcript {
                after: Some(end), ..
            } => Ok(json!({ "items": [], "start": end, "end": end })),
            Op::Transcript { .. } => Ok(json!({ "start": 0, "end": 4096, "items": [
                { "id": "u0", "kind": "user", "text": "review the retry module", "done": false, "failed": false },
                { "id": "t0", "kind": "tool", "tool": "Read", "text": "Read crates/core/src/retry.rs", "done": true, "failed": false },
                { "id": "a0", "kind": "assistant", "done": false, "failed": false, "text":
                  "## Review · retry module\n\nNo high defects. All **12** retry tests pass.\n\n### Findings\n\n- **retry-1** · `crates/core/src/retry.rs:42` · medium · flaky test\n  - The test sleeps a fixed 50 ms, so it races the scheduler on a busy machine.\n  - Fix: pause tokio's clock and advance it by hand.\n- **retry-2** · `crates/core/tests/retry.rs:9` · low · naming\n\nShall I fix retry-1?" },
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
            Op::Skills { .. } => Ok(json!({"skills": [
                {"name": "/commit", "description": "Create well-formatted commits", "source": "user"},
                {"name": "/council", "description": "Several agents review a plan", "source": "user"},
                {"name": "/compact", "description": "Summarise the conversation to free up context", "source": "builtin"}
            ]})),
            Op::Agents => {
                let codex = if self.answered.load(Ordering::SeqCst) {
                    "working"
                } else {
                    "blocked"
                };
                Ok(json!([
                    {"name": "crm/orchestrator", "pane_id": "w1:p1", "kind": "claude",
                     "agent_status": "blocked", "session_id": null, "herdr_session": "crm",
                     "space": "Imports"},
                    {"name": "crm/w1:p2", "pane_id": "w1:p2", "kind": "codex", "agent_status": codex,
                     "session_id": null, "herdr_session": "crm", "title": "Review the import",
                     "space": "1"}
                ]))
            }
            Op::Dialog { agent } => Ok(dialog(&agent, self.answered.load(Ordering::SeqCst))),
            Op::Answer {
                agent,
                occurrence,
                option,
                deny_only,
                ..
            } => {
                let current = dialog(&agent, self.answered.load(Ordering::SeqCst));
                let chosen = current["dialog"]["options"]
                    .as_array()
                    .and_then(|o| o.iter().find(|o| o["id"] == option.as_str()).cloned());
                match chosen {
                    _ if current["occurrence"] != occurrence.as_str() => Err(ProtoError::new(
                        ErrorCode::DialogChanged,
                        "the dialog changed",
                    )),
                    Some(o) if deny_only && o["is_deny"] != true => Err(ProtoError::new(
                        ErrorCode::ApproveNotAllowed,
                        "only the deny may be chosen without a signature",
                    )),
                    Some(_) => {
                        if agent == "crm/w1:p2" {
                            self.answered.store(true, Ordering::SeqCst);
                        }
                        Ok(json!({ "answered": true }))
                    }
                    None => Err(ProtoError::new(ErrorCode::DialogChanged, "no such option")),
                }
            }
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
    let fake = Fake {
        events: tx,
        answered: AtomicBool::new(false),
    };
    link::run(cfg, Arc::new(fake), rx).await;
    Ok(())
}
