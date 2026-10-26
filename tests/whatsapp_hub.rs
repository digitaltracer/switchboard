//! The WhatsApp front door end to end: the hub with the bridge in self-chat mode on a fake
//! WhatsApp transport, the real runner over WebSocket on 127.0.0.1, and a scripted Herdr that
//! also runs an agent Switchboard didn't create. Each test stays within the bridge's burst of
//! five messages, so the rate limit never slows it down.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde_json::{Value, json};
use switchboard::cli::Agent;
use switchboard::config::HubConfig;
use switchboard::herdr::Client;
use switchboard::hook::ask_runner;
use switchboard::hook::wire::HookReply;
use switchboard::hub::runners::Readiness;
use switchboard::hub::service::{Caller, Clock, SessionService, StartRequest};
use switchboard::hub::{Hub, serve};
use switchboard::proto::auth::hash_token;
use switchboard::runner::forward_named_agents;
use switchboard::runner::hooks::HookHub;
use switchboard::runner::link::{self, LinkConfig};
use switchboard::runner::sessions::{Sessions, SessionsConfig};
use switchboard::runner::watch::watch;
use switchboard::store::{Lifecycle, Store};
use switchboard::whatsapp::relay::{self, Relay};
use switchboard::whatsapp::selfchat::{MARKER, SelfChat};
use switchboard::whatsapp::transport::{FakeTransport, Inbound};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::{TcpListener, UnixListener};
use tokio::sync::mpsc;
use tokio::task::JoinHandle;
use tokio::time::Instant;
use url::Url;

const ME: &str = "447700900123";
const RUNNER_TOKEN: &str = "mac-runner-token";
const SECRET: &str = "ghp_abcdefghijklmnopqrstuvwxyz0123456789";
const FOREIGN: &str = "switchboard-impl";

#[derive(Clone)]
struct FakeAgent {
    name: String,
    pane_id: String,
    workspace_id: String,
    status: String,
    output: String,
}

/// A scripted Herdr session. It records every method and refuses prompts to blocked agents.
#[derive(Clone, Default)]
struct FakeHerdr {
    agents: Arc<Mutex<Vec<FakeAgent>>>,
    calls: Arc<Mutex<Vec<Value>>>,
    /// (target, text) of every prompt Herdr accepted.
    delivered: Arc<Mutex<Vec<(String, String)>>>,
    workspaces: Arc<Mutex<u32>>,
}

impl FakeHerdr {
    fn start(dir: &Path) -> (Self, PathBuf) {
        let socket = dir.join("herdr.sock");
        let listener = UnixListener::bind(&socket).unwrap();
        let herdr = Self::default();
        herdr.agents.lock().unwrap().push(FakeAgent {
            name: FOREIGN.into(),
            pane_id: "w1:p4".into(),
            workspace_id: "w1".into(),
            status: "idle".into(),
            output: "impl output\n".into(),
        });
        let h = herdr.clone();
        tokio::spawn(async move {
            loop {
                let (stream, _) = listener.accept().await.unwrap();
                let h = h.clone();
                tokio::spawn(async move {
                    let (read, mut write) = stream.into_split();
                    let mut line = String::new();
                    BufReader::new(read).read_line(&mut line).await.unwrap();
                    let req: Value = serde_json::from_str(&line).unwrap();
                    h.calls.lock().unwrap().push(req.clone());
                    let reply = match h.answer(&req) {
                        Ok(result) => json!({"id": req["id"], "result": result}),
                        Err((code, message)) => {
                            json!({"id": req["id"], "error": {"code": code, "message": message}})
                        }
                    };
                    let _ = write.write_all(format!("{reply}\n").as_bytes()).await;
                    if req["method"] == "events.subscribe" {
                        std::future::pending::<()>().await;
                    }
                });
            }
        });
        (herdr, socket)
    }

    fn info(a: &FakeAgent) -> Value {
        json!({"terminal_id": "t", "agent_status": a.status, "workspace_id": a.workspace_id,
               "tab_id": format!("{}:t1", a.workspace_id), "pane_id": a.pane_id, "focused": false,
               "revision": 1, "name": a.name, "agent": "claude",
               "interactive_ready": true, "launch_pending": false})
    }

    fn find(&self, target: &str) -> Option<FakeAgent> {
        self.agents
            .lock()
            .unwrap()
            .iter()
            .find(|a| a.name == target || a.pane_id == target)
            .cloned()
    }

    fn answer(&self, req: &Value) -> Result<Value, (&'static str, String)> {
        let p = &req["params"];
        let target = p["target"].as_str().unwrap_or_default();
        Ok(match req["method"].as_str().unwrap() {
            "events.subscribe" => json!({"type": "subscription_started"}),
            "session.snapshot" => {
                let agents = self.agents.lock().unwrap();
                let panes: Vec<Value> = agents
                    .iter()
                    .map(|a| {
                        json!({"pane_id": a.pane_id, "workspace_id": a.workspace_id,
                                    "tab_id": format!("{}:t1", a.workspace_id),
                                    "agent_status": a.status, "revision": 1, "agent": "claude"})
                    })
                    .collect();
                let workspaces: Vec<Value> = agents
                    .iter()
                    .map(|a| json!({"workspace_id": a.workspace_id, "label": "x", "agent_status": a.status}))
                    .collect();
                json!({"type": "session_snapshot", "snapshot": {"version": "0.9.1", "protocol": 22,
                       "workspaces": workspaces, "tabs": [], "panes": panes, "layouts": [],
                       "agents": agents.iter().map(Self::info).collect::<Vec<_>>()}})
            }
            "worktree.create" => {
                let mut n = self.workspaces.lock().unwrap();
                *n += 1;
                let ws = format!("w{}", *n + 1);
                json!({"type": "worktree_created",
                    "workspace": {"workspace_id": ws, "label": "x", "agent_status": "unknown"},
                    "tab": {"tab_id": format!("{ws}:t1"), "workspace_id": ws},
                    "root_pane": {"pane_id": format!("{ws}:p1"), "workspace_id": ws, "tab_id": format!("{ws}:t1"),
                                  "agent_status": "unknown", "revision": 0},
                    "worktree": {"path": format!("/wt/{ws}"), "branch": p["branch"], "label": "x",
                                 "is_linked_worktree": true}})
            }
            "agent.start" => {
                let pane = p["pane_id"].as_str().unwrap().to_owned();
                let a = FakeAgent {
                    name: p["name"].as_str().unwrap().into(),
                    workspace_id: pane.split(':').next().unwrap().into(),
                    pane_id: pane,
                    status: "idle".into(),
                    output: String::new(),
                };
                self.agents.lock().unwrap().push(a.clone());
                json!({"type": "agent_started", "agent": Self::info(&a), "argv": []})
            }
            "agent.prompt" => {
                let Some(a) = self.find(target) else {
                    return Err(("agent_not_found", format!("no agent {target}")));
                };
                if a.status == "blocked" {
                    return Err(("agent_blocked", "agent is blocked".into()));
                }
                let text = p["text"].as_str().unwrap_or_default().to_owned();
                self.delivered.lock().unwrap().push((a.name.clone(), text));
                json!({"type": "agent_prompted", "agent": Self::info(&a)})
            }
            "agent.read" => {
                let text = self.find(target).map(|a| a.output).unwrap_or_default();
                json!({"type": "pane_read", "read": {"pane_id": "p", "text": text, "revision": 3, "truncated": false}})
            }
            _ => json!({"type": "ok"}),
        })
    }

    fn set(&self, name: &str, status: &str, output: &str) {
        for a in self
            .agents
            .lock()
            .unwrap()
            .iter_mut()
            .filter(|a| a.name == name)
        {
            a.status = status.into();
            a.output = output.into();
        }
    }

    /// Texts of every prompt `target` accepted.
    fn prompts(&self, target: &str) -> Vec<String> {
        self.delivered
            .lock()
            .unwrap()
            .iter()
            .filter(|(t, _)| t == target)
            .map(|(_, text)| text.clone())
            .collect()
    }

    fn methods(&self) -> Vec<String> {
        self.calls
            .lock()
            .unwrap()
            .iter()
            .map(|c| c["method"].as_str().unwrap().to_owned())
            .collect()
    }
}

struct World {
    _dir: tempfile::TempDir,
    herdr: FakeHerdr,
    hub: Hub,
    state_dir: PathBuf,
    transport: Arc<FakeTransport>,
    inbound: mpsc::Sender<Inbound>,
    _bridge: JoinHandle<()>,
    next: u32,
}

impl World {
    async fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let (herdr, socket) = FakeHerdr::start(dir.path());
        let db = dir.path().join("hub.db");
        let cfg = HubConfig::parse(&format!(
            r#"
            public_url = "http://localhost:1"
            listen = "127.0.0.1:0"
            database = "{}"
            default_machine = "mac"
            [[machines]]
            id = "mac"
            token_hash = "{}"
            [[projects]]
            id = "demo"
            description = "scratch"
            repos = {{ mac = "/srv/demo" }}
            [[projects]]
            id = "other"
            description = "scratch too"
            repos = {{ mac = "/srv/other" }}
            "#,
            db.display(),
            hash_token(RUNNER_TOKEN)
        ))
        .unwrap();
        let svc = SessionService::new(cfg, Store::open(&db).await.unwrap(), Clock::System);
        svc.recover().await.unwrap();
        let hub = serve(svc.clone(), TcpListener::bind("127.0.0.1:0").await.unwrap()).unwrap();

        // The runner, as `switchboard runner` wires it.
        let hook_listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let hook_addr = hook_listener.local_addr().unwrap();
        let (events_tx, events_rx) = mpsc::channel(256);
        let hooks = HookHub::new(events_tx.clone(), Duration::from_secs(120));
        tokio::spawn(hooks.clone().serve(hook_listener));
        let state_dir = dir.path().join("runner-state");
        let sessions = Sessions::new(
            Client::new(&socket).with_timeout(Duration::from_secs(10)),
            hooks,
            events_tx,
            SessionsConfig {
                state_dir: state_dir.clone(),
                claude_config: dir.path().join("claude.json"),
                claude_mode: Default::default(),
                claude_projects: std::path::PathBuf::from("/nonexistent/claude/projects"),
                codex_sessions: std::path::PathBuf::from("/nonexistent/codex/sessions"),
                exe: PathBuf::from("/opt/sb/switchboard"),
                hook_addr,
                codex_version_cmd: vec!["false".into()],
                liveness_timeout: Duration::from_secs(10),
                startup_timeout: Duration::from_secs(10),
            },
        );
        tokio::spawn(watch(sessions.clone()));
        tokio::spawn(forward_named_agents(
            Client::new(&socket),
            sessions.clone(),
            Duration::from_millis(200),
        ));
        let link_cfg = LinkConfig {
            url: Url::parse(&format!("ws://{}/runner", hub.addr)).unwrap(),
            token: RUNNER_TOKEN.into(),
            machine_id: "mac".into(),
            herdr_version: Some("0.9.1".into()),
        };
        tokio::spawn(link::run(link_cfg, sessions, events_rx));
        tokio::spawn(claude_session_starts(state_dir.clone()));

        // The bridge in self-chat mode, as the hub runs it.
        let transport = Arc::new(FakeTransport::default());
        let (inbound, inbound_rx) = mpsc::channel(64);
        let sc = Arc::new(SelfChat::new(ME));
        let mut relay = Relay::self_chat(svc.clone(), sc);
        let events = svc.subscribe();
        let t = transport.clone();
        let bridge = tokio::spawn(async move {
            relay::run(&mut relay, &*t, inbound_rx, events).await;
        });

        let w = Self {
            _dir: dir,
            herdr,
            hub,
            state_dir,
            transport,
            inbound,
            _bridge: bridge,
            next: 0,
        };
        wait_for("runner ready", || async {
            w.svc().readiness("mac") == Readiness::Ready
        })
        .await;
        w
    }

    fn svc(&self) -> &Arc<SessionService> {
        &self.hub.service
    }

    /// You, typing in your own chat.
    async fn say(&mut self, text: &str) {
        self.next += 1;
        self.inbound
            .send(Inbound {
                id: format!("msg-{}", self.next),
                chat: format!("{ME}@s.whatsapp.net"),
                sender: format!("{ME}:0@s.whatsapp.net"),
                sender_alt: None,
                is_group: false,
                from_me: true,
                text: text.into(),
            })
            .await
            .unwrap();
    }

    /// The next message the bridge sent, without its self-chat marker.
    async fn heard(&self, seen: &mut usize) -> String {
        let deadline = Instant::now() + Duration::from_secs(20);
        loop {
            let sent = self.transport.sent();
            if let Some((to, text)) = sent.get(*seen) {
                *seen += 1;
                assert_eq!(to, ME);
                assert!(
                    text.starts_with(MARKER),
                    "self-chat marks every message: {text}"
                );
                assert!(!text.contains(SECRET), "unmasked secret sent: {text}");
                eprintln!("bridge: {text}");
                return text[MARKER.len()..].to_owned();
            }
            assert!(Instant::now() < deadline, "the bridge said nothing more");
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    }

    async fn start(&self, project: &str, prompt: &str) -> String {
        self.svc()
            .start(
                &Caller::new("mcp:test"),
                StartRequest {
                    project_id: project.into(),
                    prompt: prompt.into(),
                    machine_id: None,
                    agent: None,
                    notify: true,
                    request_id: None,
                },
            )
            .await
            .unwrap()
            .session_id
    }

    /// Stands in for Claude: its permission hook asks the runner and blocks for the answer.
    fn ask_permission(&self, session_id: &str, command: &str) -> JoinHandle<HookReply> {
        let secret = self
            .state_dir
            .join("sessions")
            .join(session_id)
            .join("secret.json");
        let payload = json!({"hook_event_name": "PermissionRequest", "tool_name": "Bash",
                             "tool_input": {"command": command}});
        tokio::task::spawn_blocking(move || {
            ask_runner(
                &secret,
                Agent::Claude,
                "PermissionRequest",
                &payload,
                Duration::from_secs(60),
            )
            .unwrap()
        })
    }

    fn no_keys_sent(&self) {
        let methods = self.herdr.methods();
        assert!(
            !methods.iter().any(|m| m.contains("keys")),
            "a key reached Herdr: {methods:?}"
        );
    }
}

async fn claude_session_starts(state_dir: PathBuf) {
    let mut acked = std::collections::HashSet::new();
    loop {
        if let Ok(entries) = std::fs::read_dir(state_dir.join("sessions")) {
            for e in entries.flatten() {
                let secret = e.path().join("secret.json");
                if acked.contains(&secret) || !secret.exists() {
                    continue;
                }
                let s = secret.clone();
                let reply = tokio::task::spawn_blocking(move || {
                    ask_runner(
                        &s,
                        Agent::Claude,
                        "SessionStart",
                        &json!({"source": "startup"}),
                        Duration::from_secs(2),
                    )
                })
                .await
                .unwrap();
                if matches!(reply, Ok(HookReply::Ack)) {
                    acked.insert(secret);
                }
            }
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

async fn wait_for<F, Fut>(what: &str, mut check: F)
where
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = bool>,
{
    let deadline = Instant::now() + Duration::from_secs(15);
    while !check().await {
        assert!(Instant::now() < deadline, "timed out waiting for {what}");
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

fn label(project: &str, id: &str) -> String {
    format!("{project}-{}", &id[..4])
}

/// Checks 1 and 3: a message naming no project asks with a numbered list, and the number
/// starts the session; a message naming a project starts one there directly.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_numbered_answer_or_a_project_name_starts_a_session() {
    let mut w = World::new().await;
    let mut seen = 0;
    w.say("fix the flaky test").await;
    assert_eq!(
        w.heard(&mut seen).await,
        "Which project should this start in? It names none I know, and no session is current.\n1. demo\n2. other\nReply with the number."
    );
    w.say("1").await;
    assert_eq!(w.heard(&mut seen).await, "Starting a session in demo…");
    let done = w.heard(&mut seen).await;
    let s = w.svc().list().await.unwrap().remove(0);
    assert_eq!(
        done,
        format!(
            "Started {} in demo, branch sb/{}. Messages that name no project now go to it.",
            label("demo", &s.id),
            s.id
        )
    );
    assert_eq!(
        (s.lifecycle, s.machine_id.as_str()),
        (Lifecycle::Running, "mac")
    );
    assert!(s.worktree_path.is_some(), "a worktree per session");
    let agent = s.agent_name.clone().unwrap();
    assert!(agent.starts_with("sb-"));
    assert_eq!(
        w.herdr.prompts(&agent),
        ["fix the flaky test"],
        "the prompt was delivered"
    );

    w.say("In other, write the changelog").await;
    assert_eq!(w.heard(&mut seen).await, "Starting a session in other…");
    let done = w.heard(&mut seen).await;
    let other = w
        .svc()
        .list()
        .await
        .unwrap()
        .into_iter()
        .find(|v| v.project_id == "other")
        .unwrap();
    assert!(
        done.starts_with(&format!(
            "Started {} in other, branch sb/{}",
            label("other", &other.id),
            other.id
        )),
        "{done}"
    );
    assert_eq!(
        w.herdr.prompts(other.agent_name.as_ref().unwrap()),
        ["In other, write the changelog"]
    );
    assert_eq!(
        w.herdr
            .methods()
            .iter()
            .filter(|m| *m == "worktree.create")
            .count(),
        2
    );
    w.no_keys_sent();
}

/// Check 2: `use this`, `use <x>` with a numbered answer when ambiguous, and an agent
/// Switchboard didn't create, reached through the runner.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn use_reaches_sessions_and_agents_switchboard_didnt_create() {
    let mut w = World::new().await;
    let a = w.start("demo", "first").await;
    let b = w.start("demo", "second").await;
    let mut seen = 0;
    w.say("use demo: which of you?").await;
    let list = w.heard(&mut seen).await;
    assert!(
        list.starts_with("That matches more than one. Which?\n1. "),
        "{list}"
    );
    let pick_b = if list.contains(&format!("1. {}", label("demo", &b))) {
        "1"
    } else {
        "2"
    };
    w.say(pick_b).await;
    assert_eq!(
        w.heard(&mut seen).await,
        format!("Sent to {}.", label("demo", &b))
    );
    w.say("use this session: and add a test").await;
    assert_eq!(
        w.heard(&mut seen).await,
        format!("Sent to {}.", label("demo", &b))
    );
    let agent_b = w
        .svc()
        .get(&b, None, Duration::ZERO)
        .await
        .unwrap()
        .agent_name
        .unwrap();
    assert_eq!(
        w.herdr.prompts(&agent_b),
        ["second", "which of you?", "and add a test"]
    );
    let agent_a = w
        .svc()
        .get(&a, None, Duration::ZERO)
        .await
        .unwrap()
        .agent_name
        .unwrap();
    assert_eq!(w.herdr.prompts(&agent_a), ["first"]);

    w.say(&format!("use {FOREIGN}: how is it going?")).await;
    assert_eq!(w.heard(&mut seen).await, format!("Sent to {FOREIGN}."));
    assert_eq!(w.herdr.prompts(FOREIGN), ["how is it going?"]);

    // Blocked at a dialog: nothing is sent, and the bridge says to answer it in the tab.
    w.herdr.set(FOREIGN, "blocked", "Allow this?\n");
    w.say("carry on").await;
    let refused = w.heard(&mut seen).await;
    assert!(
        refused.starts_with(&format!(
            "{FOREIGN} is waiting at a dialog, so I didn't send that. Answer it in its Herdr tab"
        )),
        "{refused}"
    );
    assert_eq!(w.herdr.prompts(FOREIGN).len(), 1);
    w.no_keys_sent();
}

/// Check 4: a permission request arrives with a number and masked input; approve and deny
/// go to the hook, a replayed number returns the recorded outcome, and no key is sent.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn approvals_arrive_by_number_and_are_decided_from_chat() {
    let mut w = World::new().await;
    let id = w.start("demo", "deploy").await;
    let mut seen = 0;

    let first = w.ask_permission(
        &id,
        &format!("curl -H 'Authorization: Bearer {SECRET}' https://x"),
    );
    let note = w.heard(&mut seen).await;
    let n = w
        .svc()
        .get(&id, None, Duration::ZERO)
        .await
        .unwrap()
        .pending_approvals[0]
        .number
        .unwrap();
    assert!(
        note.starts_with(&format!("{} asks to use Bash, approval {n}:\n\ncurl -H 'Authorization: Bearer [redacted]' https://x", label("demo", &id))),
        "{note}"
    );
    w.say(&format!("approve {n}")).await;
    assert_eq!(w.heard(&mut seen).await, format!("Approved {n}."));
    assert!(
        matches!(first.await.unwrap(), HookReply::Allow { .. }),
        "the tool runs"
    );

    let second = w.ask_permission(&id, "rm -rf build");
    let note = w.heard(&mut seen).await;
    let m = n + 1;
    assert!(
        note.contains(&format!("approval {m}:\n\nrm -rf build")),
        "{note}"
    );
    w.say(&format!("deny {m}")).await;
    assert_eq!(w.heard(&mut seen).await, format!("Denied {m}."));
    assert!(
        matches!(second.await.unwrap(), HookReply::Deny { .. }),
        "the tool is blocked"
    );

    w.say(&format!("approve {m}")).await;
    assert_eq!(
        w.heard(&mut seen).await,
        format!("Approval {m} was already denied.")
    );
    w.no_keys_sent();
}

/// Check 5: notifications from an opted-in agent, a mute from chat that the session service
/// sees, and the bridge's own messages coming back into self-chat are ignored.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn notifications_mutes_and_echoes() {
    let mut w = World::new().await;
    let mut seen = 0;
    w.say(&format!("watch {FOREIGN}")).await;
    assert_eq!(w.heard(&mut seen).await, format!("Watching {FOREIGN}."));
    assert!(
        w.svc()
            .notify_state()
            .await
            .unwrap()
            .watched
            .contains(FOREIGN)
    );

    w.herdr.set(FOREIGN, "working", "");
    tokio::time::sleep(Duration::from_secs(4)).await;
    w.herdr.set(
        FOREIGN,
        "idle",
        &format!("export GITHUB_TOKEN={SECRET}\nall done\n"),
    );
    let done = w.heard(&mut seen).await;
    assert_eq!(
        done,
        format!("{FOREIGN} finished:\n\nexport GITHUB_TOKEN=[redacted]\nall done")
    );

    w.say(&format!("mute {FOREIGN}")).await;
    assert_eq!(w.heard(&mut seen).await, format!("Muted {FOREIGN}."));
    assert!(
        w.svc()
            .notify_state()
            .await
            .unwrap()
            .muted
            .contains(FOREIGN),
        "visible through the session service"
    );

    // The bridge's own message echoed back into your chat gets no answer.
    let echo = format!("{MARKER}{FOREIGN} finished");
    w.say(&echo).await;
    w.herdr.set(FOREIGN, "working", "");
    tokio::time::sleep(Duration::from_secs(4)).await;
    w.herdr.set(FOREIGN, "idle", "again\n");
    tokio::time::sleep(Duration::from_secs(5)).await;
    assert_eq!(
        w.transport.sent().len(),
        seen,
        "muted, and echoes are ignored"
    );
    w.no_keys_sent();
}
