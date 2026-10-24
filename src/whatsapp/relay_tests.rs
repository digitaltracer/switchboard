use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use serde_json::json;

use super::*;
use crate::config::HubConfig;
use crate::hub::service::Clock;
use crate::proto::auth::hash_token;
use crate::proto::{
    AgentSummary, Event, Frame, Hello, Op, Outcome, PROTOCOL_VERSION, ProtoError, Resp,
};
use crate::store::{Lifecycle, Store};
use crate::whatsapp::limits::{BURST, HOLD, REFILL};
use crate::whatsapp::selfchat::{MARKER, SelfChat};
use crate::whatsapp::transport::FakeTransport;

const ME: &str = "447700900123";
const OTHER: &str = "15550001111";
const T0: i64 = 1_700_000_000_000;
const TOKEN: &str = "ghp_abcdefghijklmnopqrstuvwxyz0123456789";

fn config() -> HubConfig {
    HubConfig::parse(&format!(
        r#"
        public_url = "http://localhost:1"
        listen = "127.0.0.1:0"
        database = "/unused/hub.db"
        default_machine = "mac"
        [[machines]]
        id = "mac"
        token_hash = "{}"
        [[projects]]
        id = "switchboard"
        aliases = ["sb"]
        description = "this repo"
        repos = {{ mac = "/src/switchboard" }}
        [[projects]]
        id = "hub"
        description = "another"
        repos = {{ mac = "/src/hub" }}
        "#,
        hash_token("t")
    ))
    .unwrap()
}

/// What the scripted runner answers with.
#[derive(Clone, Default)]
struct Script {
    agents: Arc<Mutex<Vec<AgentSummary>>>,
    output: Arc<Mutex<HashMap<String, String>>>,
    screens: Arc<Mutex<HashMap<String, String>>>,
    blocked: Arc<Mutex<Vec<String>>>,
    /// (agent or session id, text) for every prompt that reached the runner.
    prompts: Arc<Mutex<Vec<(String, String)>>>,
    ops: Arc<Mutex<Vec<String>>>,
}

impl Script {
    fn agent(&self, name: &str, status: &str) {
        let mut agents = self.agents.lock().unwrap();
        let pane_id = format!("w{}:p1", agents.len() + 10);
        agents.push(AgentSummary {
            name: name.into(),
            pane_id,
            kind: Some("claude".into()),
            agent_status: status.into(),
            session_id: None,
            herdr_session: None,
            title: None,
        });
    }

    fn answer(&self, op: Op) -> Outcome {
        self.ops.lock().unwrap().push(format!("{op:?}"));
        let text = |map: &Arc<Mutex<HashMap<String, String>>>, k: &str| {
            map.lock().unwrap().get(k).cloned().unwrap_or_default()
        };
        match op {
            Op::Agents => Outcome::Ok(serde_json::to_value(&*self.agents.lock().unwrap()).unwrap()),
            Op::ReadAgent { agent, visible, .. } => {
                let map = if visible { &self.screens } else { &self.output };
                Outcome::Ok(json!({ "text": text(map, &agent) }))
            }
            Op::Read { session_id, .. } => {
                Outcome::Ok(json!({ "text": text(&self.output, &session_id) }))
            }
            Op::PromptAgent { agent, text: said } => {
                let out = if self.blocked.lock().unwrap().contains(&agent) {
                    AgentPromptOutcome::Blocked {
                        screen: text(&self.screens, &agent),
                    }
                } else if !self.agents.lock().unwrap().iter().any(|a| a.name == agent) {
                    AgentPromptOutcome::NotFound
                } else {
                    self.prompts.lock().unwrap().push((agent, said));
                    AgentPromptOutcome::Sent
                };
                Outcome::Ok(serde_json::to_value(out).unwrap())
            }
            Op::Prompt { session_id, text } => {
                if self.blocked.lock().unwrap().contains(&session_id) {
                    return Outcome::Err(ProtoError::new(ErrorCode::AgentBlocked, "agent_blocked"));
                }
                self.prompts.lock().unwrap().push((session_id, text));
                Outcome::Ok(json!({ "agent_status": "working" }))
            }
            Op::StartSession {
                session_id, branch, ..
            } => Outcome::Ok(json!({
                "session_id": session_id, "branch": branch, "worktree_path": "/wt",
                "workspace_id": "w9", "pane_id": "w9:p1",
            })),
            Op::Decide { .. } => Outcome::Ok(json!({ "applied": true })),
            Op::Stop { .. } => Outcome::Ok(json!({ "worktree_removed": false })),
            other => Outcome::Err(ProtoError::new(ErrorCode::BadRequest, format!("{other:?}"))),
        }
    }
}

struct Rig {
    svc: Arc<SessionService>,
    clock: Clock,
    script: Script,
    generation: u64,
}

impl Rig {
    async fn new() -> Self {
        let clock = Clock::manual(T0);
        let svc = SessionService::new(
            config(),
            Store::open_in_memory().await.unwrap(),
            clock.clone(),
        );
        let script = Script::default();
        let hello = Hello {
            machine_id: "mac".into(),
            protocol_version: PROTOCOL_VERSION,
            runner_version: "test".into(),
            herdr_version: None,
            sessions: vec![],
        };
        let (generation, mut outbound) = svc.runner_connected("mac", hello).await;
        let (svc2, s) = (svc.clone(), script.clone());
        tokio::spawn(async move {
            while let Some(frame) = outbound.recv().await {
                if let Frame::Req(req) = frame {
                    let outcome = s.answer(req.op);
                    svc2.runner_frame(
                        "mac",
                        generation,
                        Frame::Resp(Resp {
                            id: req.id,
                            outcome,
                        }),
                    )
                    .await;
                }
            }
        });
        Self {
            svc,
            clock,
            script,
            generation,
        }
    }

    fn relay(&self) -> Relay {
        Relay::new(self.svc.clone(), vec![ME.into()])
    }

    async fn event(&self, ev: Event) {
        self.svc
            .runner_frame("mac", self.generation, Frame::Event(ev))
            .await;
    }

    /// A permission request from a session's hook, as the runner reports it. Read back from the
    /// session, since only a session's oldest open approval is announced.
    async fn ask(&self, session_id: &str, approval_id: &str, command: &str) -> ApprovalView {
        self.event(Event::ApprovalRequested {
            session_id: session_id.into(),
            approval_id: approval_id.into(),
            tool_name: "Bash".into(),
            tool_input: json!({ "command": command }),
            deadline_ms: 280_000,
        })
        .await;
        let s = self
            .svc
            .get(session_id, None, Duration::ZERO)
            .await
            .unwrap();
        s.pending_approvals
            .into_iter()
            .find(|a| a.id == approval_id)
            .expect("the approval is pending")
    }
}

fn from(number: &str, text: &str) -> Inbound {
    static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
    Inbound {
        id: format!(
            "in-{}",
            NEXT.fetch_add(1, std::sync::atomic::Ordering::SeqCst)
        ),
        chat: format!("{number}@s.whatsapp.net"),
        sender: format!("{number}@s.whatsapp.net"),
        sender_alt: None,
        is_group: false,
        from_me: false,
        text: text.into(),
    }
}

async fn reply(r: &mut Relay, text: &str) -> String {
    let out = r.handle(&from(ME, text)).await;
    assert!(out.iter().all(|o| o.to == ME && o.about.is_none()));
    out.into_iter()
        .map(|o| o.text)
        .collect::<Vec<_>>()
        .join("\n---\n")
}

/// Waits for a background start and returns its reply.
async fn started(r: &mut Relay) -> String {
    let s = r.started_rx.as_mut().unwrap().recv().await.unwrap();
    r.started(s).text
}

/// Starts a session from chat and returns its id.
async fn start_from_chat(r: &mut Relay, text: &str) -> String {
    assert!(reply(r, text).await.starts_with("Starting a session in"));
    let done = started(r).await;
    assert!(done.starts_with("Started "), "{done}");
    match r.current() {
        Some(Target::Session { id, .. }) => id.clone(),
        other => panic!("not current: {other:?}"),
    }
}

#[tokio::test]
async fn naming_a_project_starts_a_session_and_makes_it_current() {
    let rig = Rig::new().await;
    let mut r = rig.relay();
    assert!(
        reply(&mut r, "In switchboard, fix the flaky hub test")
            .await
            .starts_with("Starting a session in switchboard…")
    );
    let done = started(&mut r).await;
    let s = &rig.svc.list().await.unwrap()[0];
    let label = route::label("switchboard", &s.id);
    assert_eq!(
        done,
        format!(
            "Started {label} in switchboard, branch sb/{}. Messages that name no project now go to it.",
            s.id
        )
    );
    assert_eq!(
        (s.machine_id.as_str(), s.lifecycle),
        ("mac", Lifecycle::Running),
        "the default machine"
    );
    assert!(
        rig.script
            .ops
            .lock()
            .unwrap()
            .iter()
            .any(|o| o.contains("fix the flaky hub test"))
    );

    // Text that names no project goes to the current session, and so does `use this`.
    assert_eq!(
        reply(&mut r, "also add a test").await,
        format!("Sent to {label}.")
    );
    assert_eq!(
        reply(&mut r, "use this session: and docs").await,
        format!("Sent to {label}.")
    );
    let prompts = rig.script.prompts.lock().unwrap().clone();
    assert_eq!(
        prompts,
        [
            (s.id.clone(), "also add a test".into()),
            (s.id.clone(), "and docs".into())
        ]
    );

    // Naming a project again starts another session rather than reusing this one.
    let second = start_from_chat(&mut r, "sb: tidy the README").await;
    assert_ne!(second, s.id);
    assert_eq!(rig.svc.list().await.unwrap().len(), 2);
}

#[tokio::test]
async fn no_project_and_no_session_asks_with_a_numbered_list() {
    let rig = Rig::new().await;
    let mut r = rig.relay();
    assert_eq!(
        reply(&mut r, "fix the flaky test").await,
        "Which project should this start in? It names none I know, and no session is current.\n1. switchboard\n2. hub\nReply with the number."
    );
    assert_eq!(reply(&mut r, "7").await, "Pick a number from 1 to 2.");
    assert_eq!(reply(&mut r, "2").await, "Starting a session in hub…");
    started(&mut r).await;
    let s = &rig.svc.list().await.unwrap()[0];
    assert_eq!(s.project_id, "hub");
    assert!(
        rig.script
            .ops
            .lock()
            .unwrap()
            .iter()
            .any(|o| o.contains("fix the flaky test"))
    );
    // With a session current, a bare number is just text for it.
    assert!(reply(&mut r, "2").await.starts_with("Sent to hub-"));
}

#[tokio::test]
async fn several_projects_ask_which_and_new_takes_the_project_explicitly() {
    let rig = Rig::new().await;
    let mut r = rig.relay();
    assert_eq!(
        reply(&mut r, "port the hub fix to switchboard").await,
        "That names more than one project. Which one should this start in?\n1. hub\n2. switchboard\nReply with the number."
    );
    assert_eq!(
        reply(&mut r, "2").await,
        "Starting a session in switchboard…"
    );
    started(&mut r).await;

    assert_eq!(
        reply(&mut r, "new hub write the changelog").await,
        "Starting a session in hub…"
    );
    started(&mut r).await;
    assert!(
        rig.script
            .ops
            .lock()
            .unwrap()
            .iter()
            .any(|o| o.contains("prompt: \"write the changelog\""))
    );
    assert!(
        reply(&mut r, "new nope do it")
            .await
            .starts_with("nope isn't a project I know. Which one?\n1. switchboard")
    );
}

#[tokio::test]
async fn use_picks_sessions_and_agents_and_asks_when_ambiguous() {
    let rig = Rig::new().await;
    rig.script.agent("switchboard-impl", "idle");
    let mut r = rig.relay();
    let a = start_from_chat(&mut r, "in switchboard, one").await;
    let b = start_from_chat(&mut r, "in switchboard, two").await;
    let (la, lb) = (
        route::label("switchboard", &a),
        route::label("switchboard", &b),
    );

    assert_eq!(
        reply(&mut r, "use switchboard: which one?").await,
        format!("That matches more than one. Which?\n1. {la}\n2. {lb}\nReply with the number.")
    );
    assert_eq!(reply(&mut r, "1").await, format!("Sent to {la}."));
    assert_eq!(
        rig.script.prompts.lock().unwrap().last().unwrap(),
        &(a.clone(), "which one?".into())
    );

    assert_eq!(
        reply(&mut r, &format!("use {lb}")).await,
        format!("Messages that name no project now go to {lb}.")
    );
    assert_eq!(
        reply(&mut r, "use nothing-here: hi").await,
        "Nothing running matches nothing-here. Send sessions to see what is."
    );

    // An agent Switchboard didn't create, reached through the runner.
    assert_eq!(
        reply(&mut r, "use switchboard-impl: status?").await,
        "Sent to switchboard-impl."
    );
    assert_eq!(
        rig.script.prompts.lock().unwrap().last().unwrap(),
        &("switchboard-impl".into(), "status?".into())
    );
    let audit = rig.svc.audit_log(None, 100).await.unwrap();
    assert!(
        audit
            .iter()
            .any(|e| e.kind == "agent_prompt" && e.actor == "whatsapp")
    );

    // `sessions` numbers what's running, and `use <n>` takes those numbers.
    let listing = reply(&mut r, "sessions").await;
    assert!(
        listing.contains(&format!("1. {la} · switchboard · sb/{a} · ")),
        "{listing}"
    );
    assert!(
        listing.contains("3. switchboard-impl · not started by Switchboard · idle ← current"),
        "{listing}"
    );
    assert_eq!(
        reply(&mut r, "use 2").await,
        format!("Messages that name no project now go to {lb}.")
    );
}

#[tokio::test]
async fn an_agent_at_a_dialog_is_told_apart_from_a_pending_approval() {
    let rig = Rig::new().await;
    rig.script.agent("switchboard-impl", "blocked");
    rig.script
        .blocked
        .lock()
        .unwrap()
        .push("switchboard-impl".into());
    rig.script.screens.lock().unwrap().insert(
        "switchboard-impl".into(),
        format!("Run curl -H 'Authorization: Bearer {TOKEN}'?\n❯ 1. Yes\n  2. No\n"),
    );
    let mut r = rig.relay();
    let refused = reply(&mut r, "use switchboard-impl: carry on").await;
    assert!(refused.starts_with("switchboard-impl is waiting at a dialog, so I didn't send that. Answer it in its Herdr tab"), "{refused}");
    assert!(
        refused.contains("❯ 1. Yes") && !refused.contains(TOKEN),
        "{refused}"
    );
    assert!(rig.script.prompts.lock().unwrap().is_empty());

    // A Switchboard session blocked on its approval points at the number instead.
    let id = start_from_chat(&mut r, "in hub, go").await;
    let a = rig.ask(&id, "ap-1", "ls").await;
    rig.script.blocked.lock().unwrap().push(id.clone());
    let n = a.number.unwrap();
    assert_eq!(
        reply(&mut r, "keep going").await,
        format!(
            "{} is waiting for approval {n} (Bash), so I didn't send that. Reply approve {n} or deny {n} first.",
            route::label("hub", &id)
        )
    );
}

#[tokio::test]
async fn approvals_by_number_decide_once_and_replay_the_recorded_outcome() {
    let rig = Rig::new().await;
    let mut r = rig.relay();
    let id = start_from_chat(&mut r, "in switchboard, deploy").await;
    let label = route::label("switchboard", &id);

    let a = rig
        .ask(
            &id,
            "ap-1",
            &format!("curl -H 'Authorization: Bearer {TOKEN}' https://x"),
        )
        .await;
    let n = a.number.unwrap();
    let note = r.approval(&id, &a).await;
    assert_eq!(note.len(), 1);
    let text = &note[0].text;
    assert!(
        text.starts_with(&format!(
            "{label} asks to use Bash, approval {n}:\n\ncurl -H 'Authorization: Bearer [redacted]'"
        )),
        "{text}"
    );
    assert!(
        text.ends_with(&format!(
            "Reply approve {n} or deny {n}. Unanswered, it's denied in about 5 min."
        )),
        "{text}"
    );
    assert!(!text.contains(TOKEN));

    let status = reply(&mut r, "status").await;
    assert!(
        status.contains(&format!(
            "  approval {n} waiting: Bash (approve {n} / deny {n})"
        )),
        "{status}"
    );

    assert_eq!(
        reply(&mut r, &format!("approve {n}")).await,
        format!("Approved {n}.")
    );
    assert!(
        rig.script
            .ops
            .lock()
            .unwrap()
            .iter()
            .any(|o| o.contains("Decide") && o.contains("allow: true"))
    );
    assert_eq!(
        reply(&mut r, &format!("deny {n}")).await,
        format!("Approval {n} was already approved.")
    );
    let decides = rig
        .script
        .ops
        .lock()
        .unwrap()
        .iter()
        .filter(|o| o.contains("Decide"))
        .count();
    assert_eq!(decides, 1, "a replay sends nothing");

    let b = rig.ask(&id, "ap-2", "rm -rf build").await.number.unwrap();
    assert_eq!(
        reply(&mut r, &format!("deny {b}")).await,
        format!("Denied {b}.")
    );
    let c = rig.ask(&id, "ap-3", "make").await.number.unwrap();
    rig.clock.advance(Duration::from_secs(300));
    assert_eq!(
        reply(&mut r, &format!("approve {c}")).await,
        format!("Approval {c} had expired, so it was denied.")
    );
    assert_eq!(
        reply(&mut r, &format!("approve {c}")).await,
        format!("Approval {c} was already denied (expired).")
    );
    assert_eq!(reply(&mut r, "approve 99").await, "There's no approval 99.");
}

#[tokio::test]
async fn mutes_go_through_the_session_service() {
    let rig = Rig::new().await;
    rig.script.agent("switchboard-impl", "idle");
    let mut r = rig.relay();
    let id = start_from_chat(&mut r, "in hub, go").await;
    let agent = crate::runner::sessions::agent_name(&id);
    let label = route::label("hub", &id);

    assert_eq!(
        reply(&mut r, &format!("mute {label}")).await,
        format!("Muted {agent}.")
    );
    let view = rig.svc.get(&id, None, Duration::ZERO).await.unwrap();
    assert!(
        view.muted && !view.notifies,
        "visible through the session service"
    );
    assert_eq!(
        reply(&mut r, &format!("unmute {agent}")).await,
        format!("Unmuted {agent}.")
    );
    assert!(
        rig.svc
            .get(&id, None, Duration::ZERO)
            .await
            .unwrap()
            .notifies
    );

    assert_eq!(
        reply(&mut r, "watch switchboard-impl").await,
        "Watching switchboard-impl."
    );
    assert!(
        rig.svc
            .notify_state()
            .await
            .unwrap()
            .watched
            .contains("switchboard-impl")
    );
    assert!(
        reply(&mut r, "mute all")
            .await
            .starts_with("Muted all notifications")
    );
    assert!(rig.svc.notify_state().await.unwrap().mute_all);
    let status = reply(&mut r, "status").await;
    assert!(
        status.starts_with("All notifications are muted. Send unmute all to resume.\n"),
        "{status}"
    );
    assert!(
        status.contains(&format!("{label}: unknown · muted (all) ← current")),
        "{status}"
    );
    assert!(
        status.contains("switchboard-impl: idle · muted (all)"),
        "{status}"
    );
    assert_eq!(
        reply(&mut r, "unmute switchboard-impl").await,
        "All notifications are muted. Send unmute all first."
    );
    assert!(
        reply(&mut r, "unmute all")
            .await
            .starts_with("Notifications are back on")
    );
    assert!(!rig.svc.notify_state().await.unwrap().mute_all);
}

/// Drives one status transition through the debouncer and returns the notifications.
async fn transition(r: &mut Relay, agent: &str, from: AgentStatus, to: AgentStatus) -> Vec<String> {
    let t0 = Instant::now();
    r.observe("mac", agent, from, t0);
    let _ = r.due(t0 + HOLD).await;
    r.observe("mac", agent, to, t0 + HOLD);
    r.due(t0 + HOLD * 2)
        .await
        .into_iter()
        .map(|o| o.text)
        .collect()
}

#[tokio::test]
async fn notifications_follow_watch_state_and_skip_blocks_that_have_an_approval() {
    let rig = Rig::new().await;
    rig.script.agent("switchboard-impl", "idle");
    rig.script.agent("reviewer", "idle");
    rig.script.output.lock().unwrap().insert(
        "switchboard-impl".into(),
        format!("pushed with {TOKEN}\nall done\n"),
    );
    rig.script
        .screens
        .lock()
        .unwrap()
        .insert("switchboard-impl".into(), "Allow edit?\n❯ 1. Yes\n".into());
    let mut r = rig.relay();
    assert!(
        transition(&mut r, "reviewer", AgentStatus::Working, AgentStatus::Idle)
            .await
            .is_empty(),
        "not watched"
    );
    reply(&mut r, "watch switchboard-impl").await;
    let done = transition(
        &mut r,
        "switchboard-impl",
        AgentStatus::Working,
        AgentStatus::Idle,
    )
    .await;
    assert_eq!(
        done,
        ["switchboard-impl finished:\n\npushed with [redacted]\nall done"]
    );
    let blocked = transition(
        &mut r,
        "switchboard-impl",
        AgentStatus::Working,
        AgentStatus::Blocked,
    )
    .await;
    assert_eq!(blocked.len(), 1);
    assert!(
        blocked[0].contains("Allow edit?") && blocked[0].ends_with(FOREIGN_DIALOG),
        "{blocked:?}"
    );

    // A session blocked on its own approval: the approval message says it all.
    let id = start_from_chat(&mut r, "in hub, go").await;
    let agent = crate::runner::sessions::agent_name(&id);
    rig.ask(&id, "ap-1", "ls").await;
    assert!(
        transition(&mut r, &agent, AgentStatus::Working, AgentStatus::Blocked)
            .await
            .is_empty()
    );
    reply(&mut r, &format!("mute {agent}")).await;
    assert!(
        transition(&mut r, &agent, AgentStatus::Blocked, AgentStatus::Idle)
            .await
            .is_empty(),
        "muted"
    );
    let a = rig.ask(&id, "ap-2", "ls").await;
    assert!(
        r.approval(&id, &a).await.is_empty(),
        "a muted session's approvals stay quiet"
    );
}

#[tokio::test]
async fn stop_keeps_the_worktree_and_refuses_agents_switchboard_didnt_start() {
    let rig = Rig::new().await;
    rig.script.agent("switchboard-impl", "idle");
    let mut r = rig.relay();
    let id = start_from_chat(&mut r, "in hub, go").await;
    assert_eq!(
        reply(&mut r, &format!("stop {}", route::label("hub", &id))).await,
        format!(
            "Stopped {}. Its worktree and branch sb/{id} are kept.",
            route::label("hub", &id)
        )
    );
    assert_eq!(
        rig.svc
            .get(&id, None, Duration::ZERO)
            .await
            .unwrap()
            .lifecycle,
        Lifecycle::Stopped
    );
    assert!(r.current().is_none());
    assert_eq!(
        reply(&mut r, "stop switchboard-impl").await,
        "Switchboard didn't start switchboard-impl, so it won't stop it. Close it in its Herdr tab."
    );
}

#[tokio::test]
async fn allowlist_drops_others_silently_and_logs_no_content() {
    let logs = Arc::new(Mutex::new(Vec::<u8>::new()));
    let sink = logs.clone();
    let subscriber = tracing_subscriber::fmt()
        .with_writer(move || LogSink(sink.clone()))
        .with_ansi(false)
        .finish();
    let _guard = tracing::subscriber::set_default(subscriber);

    let rig = Rig::new().await;
    let mut r = rig.relay();
    assert!(
        r.handle(&from(
            OTHER,
            "secret plan: in switchboard rm -rf everything"
        ))
        .await
        .is_empty()
    );
    assert!(r.handle(&from(OTHER, "mute all")).await.is_empty());
    assert!(
        !rig.svc.notify_state().await.unwrap().mute_all,
        "strangers can't change mutes"
    );
    let group = Inbound {
        is_group: true,
        ..from(ME, "status")
    };
    assert!(r.handle(&group).await.is_empty());
    let mine = Inbound {
        from_me: true,
        ..from(ME, "status")
    };
    assert!(r.handle(&mine).await.is_empty());
    let lid_only = Inbound {
        sender: "123456789012345@lid".into(),
        ..from(ME, "status")
    };
    assert!(
        r.handle(&lid_only).await.is_empty(),
        "a LID without the phone JID can't be matched"
    );

    let logged = String::from_utf8(logs.lock().unwrap().clone()).unwrap();
    assert!(
        logged.contains(&format!("{OTHER}@s.whatsapp.net")),
        "{logged}"
    );
    assert!(
        !logged.contains("secret plan"),
        "content must never be logged: {logged}"
    );
    assert!(rig.svc.list().await.unwrap().is_empty(), "nothing started");
}

struct LogSink(Arc<Mutex<Vec<u8>>>);

impl std::io::Write for LogSink {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(buf);
        Ok(buf.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

#[tokio::test]
async fn phone_jid_in_sender_alt_is_accepted() {
    let rig = Rig::new().await;
    let mut r = rig.relay();
    let lid = Inbound {
        sender: "123456789012345@lid".into(),
        sender_alt: Some(format!("{ME}:3@s.whatsapp.net")),
        ..from(ME, "help")
    };
    let out = r.handle(&lid).await;
    assert_eq!(out.len(), 1);
    assert_eq!(out[0].to, ME);
}

/// Runs the relay loop in the background against `transport`.
fn spawn_run(
    mut r: Relay,
    svc: &Arc<SessionService>,
    t: Arc<FakeTransport>,
) -> (mpsc::Sender<Inbound>, tokio::task::JoinHandle<Relay>) {
    let (in_tx, in_rx) = mpsc::channel(64);
    let events = svc.subscribe();
    let task = tokio::spawn(async move {
        run(&mut r, &*t, in_rx, events).await;
        r
    });
    (in_tx, task)
}

#[tokio::test(start_paused = true)]
async fn rate_limit_and_reconnect() {
    let svc = SessionService::new(
        config(),
        Store::open_in_memory().await.unwrap(),
        Clock::manual(T0),
    );
    let transport = Arc::new(FakeTransport::default());
    let (in_tx, task) = spawn_run(
        Relay::new(svc.clone(), vec![ME.into()]),
        &svc,
        transport.clone(),
    );
    let help = || (ME.to_string(), command::HELP.to_string());

    in_tx.send(from(OTHER, "help")).await.unwrap();
    in_tx.send(from(ME, "help")).await.unwrap();
    tokio::time::sleep(Duration::from_millis(10)).await;
    assert_eq!(transport.take(), [help()]);

    // A burst beyond the bucket is held back, then trickles out.
    for _ in 0..(BURST + 3) {
        in_tx.send(from(ME, "help")).await.unwrap();
    }
    tokio::time::sleep(Duration::from_millis(10)).await;
    assert_eq!(transport.take().len(), BURST as usize - 1);
    tokio::time::sleep(REFILL * 4 + Duration::from_secs(1)).await;
    assert_eq!(transport.take().len(), 4);

    // Link down: replies wait and go out after it comes back.
    transport.set_down(true);
    in_tx.send(from(ME, "help")).await.unwrap();
    tokio::time::sleep(Duration::from_secs(5)).await;
    assert!(transport.sent().is_empty());
    transport.set_down(false);
    tokio::time::sleep(REFILL + Duration::from_secs(1)).await;
    assert_eq!(transport.take(), [help()]);

    drop(in_tx);
    task.await.unwrap();
}

#[tokio::test]
async fn a_token_in_terminal_output_never_reaches_the_transport() {
    let rig = Rig::new().await;
    rig.script.agent("switchboard-impl", "idle");
    rig.script.output.lock().unwrap().insert(
        "switchboard-impl".into(),
        format!("$ export GITHUB_TOKEN={TOKEN}\n$ gh auth status\nlogged in with {TOKEN}\n"),
    );
    rig.script.screens.lock().unwrap().insert(
        "switchboard-impl".into(),
        format!(
            "Bash command\n  curl -H 'Authorization: Bearer {TOKEN}' x\nDo you want to proceed?\n"
        ),
    );
    rig.script
        .blocked
        .lock()
        .unwrap()
        .push("switchboard-impl".into());
    let transport = Arc::new(FakeTransport::default());
    let (in_tx, task) = spawn_run(rig.relay(), &rig.svc, transport.clone());
    in_tx.send(from(ME, "use switchboard-impl")).await.unwrap();
    in_tx.send(from(ME, "read 10")).await.unwrap();
    in_tx
        .send(from(ME, &format!("carry on, the token is {TOKEN}")))
        .await
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(5);
    while transport.sent().len() < 3 {
        assert!(Instant::now() < deadline, "{:?}", transport.sent());
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    let all: String = transport
        .take()
        .into_iter()
        .map(|(_, t)| t)
        .collect::<Vec<_>>()
        .join("\n---\n");
    assert!(!all.contains(TOKEN), "{all}");
    assert!(
        all.contains("GITHUB_TOKEN=[redacted]") && all.contains("Bearer [redacted]"),
        "{all}"
    );
    drop(in_tx);
    task.await.unwrap();
}

// Self-chat mode: linked to your own account, reading only your "Message yourself" chat.

const MY_LID: &str = "123456789012345";

/// A message you type in your own chat, from the phone.
fn in_own_chat(text: &str) -> Inbound {
    Inbound {
        from_me: true,
        sender: format!("{ME}:0@s.whatsapp.net"),
        ..from(ME, text)
    }
}

fn self_chat_relay(svc: &Arc<SessionService>) -> Relay {
    let sc = Arc::new(SelfChat::new(ME));
    sc.set_own_lid(&format!("{MY_LID}:4@lid"));
    Relay::self_chat(svc.clone(), sc)
}

#[tokio::test]
async fn self_chat_ignores_every_other_chat_and_logs_none_of_it() {
    let logs = Arc::new(Mutex::new(Vec::<u8>::new()));
    let sink = logs.clone();
    let subscriber = tracing_subscriber::fmt()
        .with_writer(move || LogSink(sink.clone()))
        .with_max_level(tracing::Level::TRACE)
        .with_ansi(false)
        .finish();
    let _guard = tracing::subscriber::set_default(subscriber);

    let rig = Rig::new().await;
    rig.script.agent("switchboard-impl", "idle");
    let mut r = self_chat_relay(&rig.svc);
    let others = [
        // A friend writing to you.
        from(OTHER, "dinner plans: 8pm at Luigi's"),
        // You writing to that friend, from your phone.
        Inbound {
            from_me: true,
            ..from(OTHER, "see you at Luigi's, in switchboard")
        },
        // A group you're in, from you and from someone else.
        Inbound {
            is_group: true,
            from_me: true,
            chat: "120363000000000000@g.us".into(),
            ..from(ME, "status")
        },
        Inbound {
            is_group: true,
            chat: "120363000000000000@g.us".into(),
            ..from(OTHER, "mute all")
        },
        // Someone else's privacy-id chat.
        Inbound {
            from_me: true,
            chat: "999999999999999@lid".into(),
            ..from(ME, "use switchboard-impl")
        },
    ];
    for m in &others {
        assert!(r.handle(m).await.is_empty(), "{m:?}");
    }
    assert_eq!(r.dropped(), others.len() as u64);
    assert!(!rig.svc.notify_state().await.unwrap().mute_all);
    assert!(r.current().is_none());
    assert!(rig.svc.list().await.unwrap().is_empty());

    // Your own chat works, by phone JID and by privacy id.
    let out = r.handle(&in_own_chat("status")).await;
    assert_eq!((out.len(), out[0].to.as_str()), (1, ME));
    let by_lid = Inbound {
        chat: format!("{MY_LID}@lid"),
        ..in_own_chat("use switchboard-impl")
    };
    assert_eq!(r.handle(&by_lid).await.len(), 1);
    assert_eq!(r.current().map(Target::label), Some("switchboard-impl"));

    let logged = String::from_utf8(logs.lock().unwrap().clone()).unwrap();
    for leak in [
        "Luigi",
        "dinner",
        OTHER,
        "120363000000000000",
        "999999999999999",
    ] {
        assert!(!logged.contains(leak), "{leak} logged: {logged}");
    }
    assert!(rig.script.prompts.lock().unwrap().is_empty());
}

#[tokio::test]
async fn self_chat_never_answers_its_own_messages() {
    let rig = Rig::new().await;
    rig.script.agent("switchboard-impl", "idle");
    rig.script
        .output
        .lock()
        .unwrap()
        .insert("switchboard-impl".into(), format!("pushed with {TOKEN}\n"));
    let transport = Arc::new(FakeTransport::default());
    let (in_tx, task) = spawn_run(self_chat_relay(&rig.svc), &rig.svc, transport.clone());
    let settle = || tokio::time::sleep(Duration::from_millis(300));

    in_tx
        .send(in_own_chat("use switchboard-impl"))
        .await
        .unwrap();
    in_tx.send(in_own_chat("read 5")).await.unwrap();
    settle().await;
    let sent = transport.sent();
    assert_eq!(sent.len(), 2, "{sent:?}");
    for (to, text) in &sent {
        assert_eq!(to, ME, "replies go to your own chat");
        assert!(text.starts_with(MARKER), "{text}");
        assert!(!text.contains(TOKEN), "masking is unchanged: {text}");
    }

    // The bridge's messages come back into your chat as messages from you.
    let ids = transport.ids();
    for (id, (_, text)) in ids.iter().zip(&sent) {
        // By id, even if the marker were lost on the way.
        let echo = Inbound {
            id: id.clone(),
            ..in_own_chat(text.trim_start_matches(MARKER))
        };
        in_tx.send(echo).await.unwrap();
        // By marker, even with an id the bridge never saw.
        in_tx.send(in_own_chat(text)).await.unwrap();
    }
    settle().await;
    assert_eq!(transport.sent().len(), 2, "no reply to its own messages");
    assert!(
        rig.script.prompts.lock().unwrap().is_empty(),
        "echoes never reach the agent"
    );

    // You can still talk to it.
    in_tx.send(in_own_chat("carry on")).await.unwrap();
    settle().await;
    assert_eq!(transport.sent().len(), 3);
    assert_eq!(rig.script.prompts.lock().unwrap().len(), 1);
    drop(in_tx);
    let r = task.await.unwrap();
    assert_eq!(r.dropped(), 4);
}

#[tokio::test]
async fn without_self_chat_nothing_is_marked_and_your_own_messages_are_ignored() {
    let rig = Rig::new().await;
    let transport = Arc::new(FakeTransport::default());
    let (in_tx, task) = spawn_run(rig.relay(), &rig.svc, transport.clone());
    in_tx.send(in_own_chat("help")).await.unwrap();
    in_tx.send(from(ME, "help")).await.unwrap();
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert_eq!(
        transport.take(),
        [(ME.to_string(), command::HELP.to_string())]
    );
    drop(in_tx);
    task.await.unwrap();
}
