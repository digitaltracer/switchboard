//! A scripted runner for the app's UI tests: it connects to a hub like a real one, accepts
//! starts, and answers reads with canned output, without Herdr or any agent. Each started
//! session reports working, then asks to run `cargo test --workspace`. Of the agents it lists,
//! the Claude one waits at a question and the Codex one at a permission dialog, until answered.
//! Three more live in the Herdr app's spaces. A start may choose the Herdr app or a terminal.
//!
//! Usage: cargo run --example fake_runner -- <hub ws url> <machine id> <token>

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use switchboard::proto::{ErrorCode, Event, Op, OwnedSession, ProtoError, StartBackend};
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
    /// Sessions started in the Herdr app, which list as its agents, each with the space it
    /// was started into.
    in_app: Mutex<Vec<(String, Option<String>)>>,
    /// Agents renamed from the phone, by pane, and spaces, by id.
    agent_names: Mutex<HashMap<String, String>>,
    space_names: Mutex<HashMap<String, String>>,
    /// Each ship skill sent: the conversation's end after it, and the user and reply items.
    shipped: Mutex<Vec<(u64, Vec<Value>)>>,
}

/// Unix time `ahead` seconds from now as the transcript writes it, `2026-11-21T09:30:00Z`.
fn now_iso(ahead: i64) -> String {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs() as i64)
        + ahead;
    let (days, rem) = (secs.div_euclid(86_400), secs.rem_euclid(86_400));
    // Howard Hinnant's civil-from-days.
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    format!(
        "{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}Z",
        rem / 3600,
        rem % 3600 / 60,
        rem % 60
    )
}

/// A two-hunk diff with a long unchanged run in the middle, for the diff screen.
fn diff(path: &str) -> Value {
    let mut lines = vec![
        json!({"kind": " ", "old": 1, "new": 1, "text": "use std::time::Duration;"}),
        json!({"kind": "+", "old": null, "new": 2, "text": "use crate::clock::sleep;"}),
        json!({"kind": " ", "old": 2, "new": 3, "text": ""}),
        json!({"kind": "-", "old": 3, "new": null, "text": "const RETRIES: u32 = 3;"}),
        json!({"kind": "+", "old": null, "new": 4, "text": "const RETRIES: u32 = 5;"}),
        json!({"kind": "+", "old": null, "new": 5, "text": "const API_KEY: &str = \"[redacted]\";"}),
    ];
    for i in 0..12 {
        lines.push(json!({"kind": " ", "old": 4 + i, "new": 6 + i, "text": format!("// unchanged line {}", 4 + i)}));
    }
    let second = vec![
        json!({"kind": "+", "old": null, "new": 42, "text": "    let delay = 2u64.pow(attempt) * 250;"}),
        json!({"kind": "+", "old": null, "new": 43, "text": "    sleep(Duration::from_millis(delay)).await;"}),
        json!({"kind": " ", "old": 39, "new": 44, "text": "    } else {"}),
        json!({"kind": "-", "old": 40, "new": null, "text": "        return Err(err);"}),
        json!({"kind": "+", "old": null, "new": 45, "text": "        if !retryable(&err) { return Err(err); }"}),
        json!({"kind": " ", "old": 41, "new": 46, "text": "    }"}),
    ];
    json!({ "path": path, "binary": false, "size": null, "added": 6, "deleted": 2, "hunk_count": 2, "next": null,
            "hunks": [
                { "header": "@@ -1,15 +1,17 @@", "old_start": 1, "new_start": 1, "lines": lines, "cut": false },
                { "header": "@@ -38,4 +42,5 @@ async fn with_retry", "old_start": 38, "new_start": 42, "lines": second, "cut": false },
            ] })
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
                        option("p2", "Yes, and don't ask again (p)", false, true),
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
                session_id,
                branch,
                backend,
                space,
                ..
            } => {
                let herdr_session = match backend {
                    Some(StartBackend::Hc) => {
                        self.in_app
                            .lock()
                            .unwrap()
                            .push((session_id.clone(), space));
                        Some("hc".to_owned())
                    }
                    Some(StartBackend::Herdr) => Some(format!("demo-{}", &session_id[..4])),
                    None => None,
                };
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
                           "workspace_id": "w1", "pane_id": "w1:p1", "herdr_session": herdr_session }),
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
            } => {
                let shipped = self.shipped.lock().unwrap();
                let items: Vec<Value> = shipped
                    .iter()
                    .filter(|(e, _)| *e > end)
                    .flat_map(|(_, i)| i.clone())
                    .collect();
                let last = shipped.last().map_or(end, |(e, _)| (*e).max(end));
                Ok(json!({ "items": items, "start": end, "end": last }))
            }
            Op::Transcript { .. } => Ok({
                let mut page = json!({ "start": 0, "end": 4096, "items": [
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
                ] });
                for (end, items) in self.shipped.lock().unwrap().iter() {
                    page["items"]
                        .as_array_mut()
                        .unwrap()
                        .extend(items.iter().cloned());
                    page["end"] = json!(end);
                }
                page
            }),
            Op::Changes { .. } => Ok(json!({ "files": [
                { "path": "crates/core/src/retry.rs", "added": 6, "deleted": 2 },
                { "path": "crates/core/tests/retry.rs", "added": 14, "deleted": 0 },
                { "path": "notes/flaky.md", "added": null, "deleted": null },
                { "path": "docs/rate-limit.png", "added": null, "deleted": null, "binary": true, "size": 24576 },
            ], "truncated": false })),
            Op::Diff { path, .. } if path.ends_with(".png") => Ok(json!({
                "path": path, "binary": true, "size": 24576, "hunk_count": 0, "hunks": [], "next": null })),
            Op::Diff { path, .. } => Ok(diff(&path)),
            // A ship skill: `/pr` ends with a pull request link, anything else without one.
            Op::Prompt { session_id, text } if text.starts_with('/') || text.starts_with('$') => {
                let reply = if text.starts_with("/pr") {
                    "Pushed the branch and opened [Fix the flaky retry test](https://github.com/acme/kraft/pull/482) as a draft against main."
                } else {
                    "Committed the fix, but the push was denied by your deny patterns, so no pull request was opened."
                };
                {
                    let mut shipped = self.shipped.lock().unwrap();
                    let n = shipped.len() as u64;
                    shipped.push((8192 + n * 1024, vec![
                        json!({ "id": format!("su{n}"), "kind": "user", "text": text, "done": false, "failed": false }),
                        json!({ "id": format!("sa{n}"), "kind": "assistant", "text": reply, "at": now_iso(2), "done": false, "failed": false }),
                    ]));
                }
                let events = self.events.clone();
                tokio::spawn(async move {
                    let _ = events
                        .send(Event::AgentStatus {
                            session_id: session_id.clone(),
                            agent_status: "working".into(),
                            revision: 3,
                        })
                        .await;
                    tokio::time::sleep(Duration::from_secs(2)).await;
                    let _ = events
                        .send(Event::AgentStatus {
                            session_id,
                            agent_status: "done".into(),
                            revision: 4,
                        })
                        .await;
                });
                Ok(json!({ "agent_status": "working" }))
            }
            Op::Prompt { .. } => Ok(json!({ "agent_status": "working" })),
            // A decided approval lets the agent finish its turn, so the session shows ship buttons.
            Op::Decide { session_id, .. } => {
                let events = self.events.clone();
                tokio::spawn(async move {
                    tokio::time::sleep(Duration::from_secs(1)).await;
                    let _ = events
                        .send(Event::AgentStatus {
                            session_id,
                            agent_status: "done".into(),
                            revision: 2,
                        })
                        .await;
                });
                Ok(json!({ "applied": true }))
            }
            Op::Stop { .. } => Ok(json!({ "worktree_removed": false })),
            Op::Delete { .. } => Ok(
                json!({ "worktree_removed": false, "kept_because": "unmerged", "herdr_session_deleted": true }),
            ),
            Op::ListSessions => Ok(json!([])),
            Op::Skills { .. } => Ok(json!({"skills": [
                {"name": "/pr", "description": "Create a GitHub pull request from the current branch", "source": "user"},
                {"name": "/ship", "description": "Commit, push and open a PR in one go", "source": "user"},
                {"name": "/commit", "description": "Create well-formatted commits", "source": "user"},
                {"name": "/code-review", "description": "Review the current diff", "source": "plugin"},
                {"name": "/triage-reviews", "description": "Answer a pull request's review comments", "source": "user"},
                {"name": "/council", "description": "Several agents review a plan", "source": "user"},
                {"name": "/compact", "description": "Summarise the conversation to free up context", "source": "builtin"}
            ]})),
            Op::Agents => {
                let codex = if self.answered.load(Ordering::SeqCst) {
                    "working"
                } else {
                    "blocked"
                };
                let agent = |session: &str, pane: &str, name: &str| {
                    let names = self.agent_names.lock().unwrap();
                    format!("{session}/{}", names.get(pane).map_or(name, String::as_str))
                };
                let space = |id: &str, name: &str| {
                    let names = self.space_names.lock().unwrap();
                    names.get(id).cloned().unwrap_or_else(|| name.to_owned())
                };
                let mut list = vec![
                    json!({"name": agent("crm", "w1:p1", "orchestrator"), "pane_id": "w1:p1", "kind": "claude",
                     "agent_status": "blocked", "session_id": null, "herdr_session": "crm",
                     "space": space("w1", "Imports"), "space_id": "w1", "can_rename": true}),
                    json!({"name": agent("crm", "w2:p2", "w1:p2"), "pane_id": "w1:p2", "kind": "codex", "agent_status": codex,
                     "session_id": null, "herdr_session": "crm", "title": "Review the import",
                     "space": space("w2", "1"), "space_id": "w2", "can_rename": true}),
                    json!({"name": agent("hc", "t2", "search"), "pane_id": "t2", "kind": "claude", "agent_status": "working",
                     "session_id": null, "herdr_session": "hc", "title": "Add typeahead to the search bar",
                     "space": space("sp-atlas", "atlas-web"), "space_id": "sp-atlas", "can_rename": true}),
                    // Named when it started, as `hc run --name docs` does: read-only from the phone.
                    json!({"name": "hc/docs", "pane_id": "t3", "kind": "claude", "agent_status": "done",
                     "session_id": null, "herdr_session": "hc", "title": "README covers the new search API",
                     "space": space("sp-atlas", "atlas-web"), "space_id": "sp-atlas"}),
                    json!({"name": agent("hc", "t4", "migrations"), "pane_id": "t4", "kind": "codex", "agent_status": "idle",
                     "session_id": null, "herdr_session": "hc", "space": space("sp-ledger", "ledger-api"),
                     "space_id": "sp-ledger", "can_rename": true}),
                ];
                for (i, (id, into)) in self.in_app.lock().unwrap().iter().enumerate() {
                    let (space_id, name) = match into.as_deref() {
                        Some("sp-atlas") => ("sp-atlas", "atlas-web"),
                        Some("sp-ledger") => ("sp-ledger", "ledger-api"),
                        _ => ("sp-demo", "demo"),
                    };
                    list.push(json!({"name": format!("sb-{id}"), "pane_id": format!("t{}", 10 + i), "kind": "claude",
                        "agent_status": "working", "session_id": id, "herdr_session": "hc",
                        "space": space(space_id, name), "space_id": space_id}));
                }
                Ok(Value::Array(list))
            }
            Op::RenameAgent { agent, name } => {
                let Some((session, old)) = agent.split_once('/') else {
                    return Err(ProtoError::new(ErrorCode::NotFound, "no such agent"));
                };
                if name == "taken" {
                    return Err(ProtoError::new(
                        ErrorCode::NameTaken,
                        "another agent is already called taken",
                    ));
                }
                let mut names = self.agent_names.lock().unwrap();
                let pane = [
                    ("w1:p1", "orchestrator"),
                    ("w2:p2", "w1:p2"),
                    ("t2", "search"),
                    ("t4", "migrations"),
                ]
                .into_iter()
                .find(|(pane, first)| names.get(*pane).map_or(*first, String::as_str) == old)
                .map(|(pane, _)| pane);
                match pane {
                    Some(pane) => {
                        names.insert(pane.to_owned(), name.clone());
                        Ok(json!({ "name": format!("{session}/{name}") }))
                    }
                    None => Err(ProtoError::new(ErrorCode::NotFound, "no such agent")),
                }
            }
            Op::RenameSpace { space_id, name, .. } => {
                if name == "taken" {
                    return Err(ProtoError::new(
                        ErrorCode::NameTaken,
                        "another space already has that name",
                    ));
                }
                self.space_names
                    .lock()
                    .unwrap()
                    .insert(space_id, name.clone());
                Ok(json!({ "name": name }))
            }
            Op::RenameSessionTab { .. } => Ok(json!({ "renamed": true })),
            Op::Dialog { agent } => Ok(dialog(&agent, self.answered.load(Ordering::SeqCst))),
            Op::StartOptions => Ok(json!({"default": "hc", "backends": [
                {"backend": "hc", "available": true}, {"backend": "herdr", "available": true}]})),
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
        in_app: Mutex::new(Vec::new()),
        agent_names: Mutex::default(),
        space_names: Mutex::default(),
        shipped: Mutex::new(Vec::new()),
    };
    link::run(cfg, Arc::new(fake), rx).await;
    Ok(())
}
