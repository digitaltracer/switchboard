//! The real runner (sessions, hook endpoint, WebSocket link) against an in-process hub over
//! real WebSocket on 127.0.0.1, with a scripted Herdr. A TCP proxy between them cuts the
//! connection to inject faults; the hub restarts on its persisted database.

use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde_json::{Value, json};
use switchboard::cli::Agent;
use switchboard::config::HubConfig;
use switchboard::herdr::Client;
use switchboard::hook::ask_runner;
use switchboard::hook::wire::HookReply;
use switchboard::hub::runners::Readiness;
use switchboard::hub::service::{
    Caller, Clock, HubError, SessionService, StartOutcome, StartRequest,
};
use switchboard::hub::{Hub, serve};
use switchboard::proto::auth::hash_token;
use switchboard::runner::hooks::HookHub;
use switchboard::runner::link::{self, LinkConfig};
use switchboard::runner::sessions::{Sessions, SessionsConfig};
use switchboard::store::{Decision, Delivery, Lifecycle, OpState, Store};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::{TcpListener, TcpStream, UnixListener};
use tokio::sync::mpsc;
use tokio::task::JoinHandle;
use tokio::time::Instant;
use url::Url;

const TOKEN: &str = "mac-runner-token";

/// A hub whose runner dials in with a token, or one that drives its own machine (`local`).
fn hub_config(db: &Path, local: bool) -> HubConfig {
    let machine = if local {
        format!(
            "[machines.local]\nstate_dir = \"{}\"",
            db.parent().unwrap().join("runner-state").display()
        )
    } else {
        format!("token_hash = \"{}\"", hash_token(TOKEN))
    };
    HubConfig::parse(&format!(
        r#"
        public_url = "https://hub.example.com"
        listen = "127.0.0.1:0"
        database = "{}"
        [auth]
        passphrase_hash = "$argon2id$v=19$m=19456,t=2,p=1$c2FsdA$aGFzaA"
        [[machines]]
        id = "mac"
        {}
        [[projects]]
        id = "demo"
        description = "scratch project"
        repos = {{ mac = "/srv/demo" }}
        "#,
        db.display(),
        machine
    ))
    .unwrap()
}

/// A scripted Herdr session: records methods, can hold `worktree.create` to widen a start.
struct FakeHerdr {
    socket: PathBuf,
    calls: Arc<Mutex<Vec<String>>>,
    worktree_delay_ms: Arc<AtomicU64>,
    /// What `agent.read` returns.
    screen: Arc<Mutex<String>>,
}

fn agent_info(status: &str) -> Value {
    json!({"terminal_id": "t", "agent_status": status, "workspace_id": "w2", "tab_id": "w2:t1",
           "pane_id": "w2:p1", "focused": false, "revision": 1,
           "interactive_ready": true, "launch_pending": false})
}

impl FakeHerdr {
    fn start(dir: &Path) -> Self {
        let socket = dir.join("herdr.sock");
        let listener = UnixListener::bind(&socket).unwrap();
        let calls = Arc::new(Mutex::new(Vec::new()));
        let worktree_delay_ms = Arc::new(AtomicU64::new(0));
        let screen = Arc::new(Mutex::new("all done\n".to_owned()));
        let (log, delay, text) = (calls.clone(), worktree_delay_ms.clone(), screen.clone());
        tokio::spawn(async move {
            loop {
                let (stream, _) = listener.accept().await.unwrap();
                let (log, delay, text) = (log.clone(), delay.clone(), text.clone());
                tokio::spawn(async move {
                    let (read, mut write) = stream.into_split();
                    let mut line = String::new();
                    BufReader::new(read).read_line(&mut line).await.unwrap();
                    let req: Value = serde_json::from_str(&line).unwrap();
                    let method = req["method"].as_str().unwrap().to_owned();
                    log.lock().unwrap().push(method.clone());
                    let result = match method.as_str() {
                        "worktree.create" => {
                            let ms = delay.load(Ordering::SeqCst);
                            tokio::time::sleep(Duration::from_millis(ms)).await;
                            json!({"type": "worktree_created",
                                "workspace": {"workspace_id": "w2", "label": "x", "agent_status": "unknown"},
                                "tab": {"tab_id": "w2:t1", "workspace_id": "w2"},
                                "root_pane": {"pane_id": "w2:p1", "workspace_id": "w2", "tab_id": "w2:t1",
                                              "agent_status": "unknown", "revision": 0},
                                "worktree": {"path": "/wt/demo", "branch": "sb/x", "label": "demo",
                                             "is_linked_worktree": true}})
                        }
                        "agent.start" => {
                            json!({"type": "agent_started", "agent": agent_info("idle"), "argv": []})
                        }
                        "agent.prompt" => {
                            json!({"type": "agent_prompted", "agent": agent_info("working")})
                        }
                        "agent.read" => json!({"type": "pane_read",
                            "read": {"pane_id": "w2:p1", "text": text.lock().unwrap().clone(), "revision": 3, "truncated": false}}),
                        "session.snapshot" => json!({"type": "session_snapshot", "snapshot": {
                            "version": "0.9.1", "protocol": 22, "workspaces": [], "tabs": [],
                            "panes": [], "layouts": [], "agents": [agent_info("idle")]}}),
                        _ => json!({"type": "ok"}),
                    };
                    let resp = json!({"id": req["id"], "result": result});
                    let _ = write.write_all(format!("{resp}\n").as_bytes()).await;
                });
            }
        });
        Self {
            socket,
            calls,
            worktree_delay_ms,
            screen,
        }
    }

    fn count(&self, method: &str) -> usize {
        self.calls
            .lock()
            .unwrap()
            .iter()
            .filter(|m| *m == method)
            .count()
    }
}

/// Forwards TCP to whichever hub is current; `cut` drops every open connection.
struct Proxy {
    addr: SocketAddr,
    upstream: Arc<Mutex<SocketAddr>>,
    conns: Arc<Mutex<Vec<JoinHandle<()>>>>,
}

impl Proxy {
    async fn start(upstream: SocketAddr) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let upstream = Arc::new(Mutex::new(upstream));
        let conns: Arc<Mutex<Vec<JoinHandle<()>>>> = Arc::default();
        let (up, list) = (upstream.clone(), conns.clone());
        tokio::spawn(async move {
            loop {
                let (mut client, _) = listener.accept().await.unwrap();
                let target = *up.lock().unwrap();
                let task = tokio::spawn(async move {
                    if let Ok(mut server) = TcpStream::connect(target).await {
                        let _ = tokio::io::copy_bidirectional(&mut client, &mut server).await;
                    }
                });
                list.lock().unwrap().push(task);
            }
        });
        Self {
            addr,
            upstream,
            conns,
        }
    }

    fn cut(&self) {
        for task in self.conns.lock().unwrap().drain(..) {
            task.abort();
        }
    }

    fn point_at(&self, upstream: SocketAddr) {
        *self.upstream.lock().unwrap() = upstream;
    }
}

struct World {
    dir: tempfile::TempDir,
    herdr: FakeHerdr,
    proxy: Proxy,
    hub: Option<Hub>,
    state_dir: PathBuf,
    bot: Caller,
}

async fn start_hub(db: &Path, local: bool) -> Hub {
    let store = Store::open(db).await.unwrap();
    let svc = SessionService::new(hub_config(db, local), store, Clock::System);
    svc.recover().await.unwrap();
    serve(svc, TcpListener::bind("127.0.0.1:0").await.unwrap()).unwrap()
}

impl World {
    /// The runner dials the hub over WebSocket, through a proxy that can cut it.
    async fn new() -> Self {
        Self::with(false).await
    }

    /// The hub drives its own machine through the built-in runner.
    async fn local() -> Self {
        Self::with(true).await
    }

    async fn with(local: bool) -> Self {
        let dir = tempfile::tempdir().unwrap();
        let herdr = FakeHerdr::start(dir.path());
        let hub = start_hub(&dir.path().join("hub.db"), local).await;
        let proxy = Proxy::start(hub.addr).await;

        let hook_listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let hook_addr = hook_listener.local_addr().unwrap();
        let (events_tx, events_rx) = mpsc::channel(256);
        let hooks = HookHub::new(events_tx.clone(), Duration::from_secs(120));
        tokio::spawn(hooks.clone().serve(hook_listener));
        let state_dir = dir.path().join("runner-state");
        let sessions = Sessions::new(
            Client::new(&herdr.socket).with_timeout(Duration::from_secs(10)),
            hooks,
            events_tx,
            SessionsConfig {
                state_dir: state_dir.clone(),
                claude_config: dir.path().join("claude.json"),
                claude_mode: Default::default(),
                exe: PathBuf::from("/opt/sb/switchboard"),
                hook_addr,
                codex_version_cmd: vec!["false".into()],
                liveness_timeout: Duration::from_secs(10),
                startup_timeout: Duration::from_secs(10),
            },
        );
        let link_cfg = LinkConfig {
            url: Url::parse(&format!("ws://{}/runner", proxy.addr)).unwrap(),
            token: TOKEN.into(),
            machine_id: "mac".into(),
            herdr_version: Some("0.9.1".into()),
        };
        if local {
            tokio::spawn(switchboard::runner::local::run(
                hub.service.clone(),
                "mac".into(),
                Some("0.9.1".into()),
                sessions,
                events_rx,
            ));
        } else {
            tokio::spawn(link::run(link_cfg, sessions, events_rx));
        }
        tokio::spawn(claude_session_starts(state_dir.clone()));
        let w = Self {
            dir,
            herdr,
            proxy,
            hub: Some(hub),
            state_dir,
            bot: Caller::new("mcp:e2e"),
        };
        w.wait_ready().await;
        w
    }

    fn svc(&self) -> &Arc<SessionService> {
        &self.hub.as_ref().unwrap().service
    }

    async fn wait_ready(&self) {
        wait_for("runner ready", || async {
            self.svc().readiness("mac") == Readiness::Ready
        })
        .await;
    }

    fn secret(&self, session_id: &str) -> PathBuf {
        self.state_dir
            .join("sessions")
            .join(session_id)
            .join("secret.json")
    }

    /// Stands in for the agent: its hook helper asks the runner and blocks for the answer.
    fn ask_permission(&self, session_id: &str, command: &str) -> JoinHandle<HookReply> {
        let secret = self.secret(session_id);
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

    async fn pending_approval(&self, session_id: &str) -> String {
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            let s = self
                .svc()
                .get(session_id, None, Duration::ZERO)
                .await
                .unwrap();
            if let Some(a) = s.pending_approvals.first() {
                return a.id.clone();
            }
            assert!(Instant::now() < deadline, "no pending approval");
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    }

    async fn start(
        &self,
        prompt: &str,
        request_id: &str,
    ) -> Result<switchboard::hub::service::StartResult, HubError> {
        self.svc()
            .start(
                &self.bot,
                StartRequest {
                    project_id: "demo".into(),
                    prompt: prompt.into(),
                    machine_id: Some("mac".into()),
                    agent: Some(Agent::Claude),
                    notify: true,
                    request_id: Some(request_id.into()),
                },
            )
            .await
    }
}

/// Claude's SessionStart hook: proves liveness for every session directory the runner creates.
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

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn start_approve_deny_prompt_read_and_stop_over_real_websocket() {
    full_flow(World::new().await).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn start_approve_deny_prompt_read_and_stop_with_the_built_in_runner() {
    let w = World::local().await;
    assert_eq!(w.svc().default_machine(), "mac", "the hub's own machine");
    full_flow(w).await;
}

async fn full_flow(w: World) {
    let started = w.start("build the thing", "rq-1").await.unwrap();
    assert_eq!(started.outcome, StartOutcome::Started);
    let sid = started.session_id.clone();
    assert_eq!(w.herdr.count("worktree.create"), 1);
    assert_eq!(w.herdr.count("agent.prompt"), 1);
    let s = w.svc().get(&sid, None, Duration::ZERO).await.unwrap();
    assert_eq!(
        (s.lifecycle, s.worktree_path.as_deref()),
        (Lifecycle::Running, Some("/wt/demo"))
    );

    let helper = w.ask_permission(&sid, "touch probe");
    let approval = w.pending_approval(&sid).await;
    let out = w
        .svc()
        .decide(&w.bot, &sid, &approval, true, Some("rq-d1"))
        .await
        .unwrap();
    assert_eq!(
        (out.outcome, out.delivery),
        (Decision::Allow, Some(Delivery::Delivered))
    );
    assert!(matches!(helper.await.unwrap(), HookReply::Allow { .. }));
    let replay = w
        .svc()
        .decide(&w.bot, &sid, &approval, false, None)
        .await
        .unwrap();
    assert_eq!((replay.outcome, replay.replayed), (Decision::Allow, true));

    let helper = w.ask_permission(&sid, "rm -rf build");
    let approval = w.pending_approval(&sid).await;
    let out = w
        .svc()
        .decide(&w.bot, &sid, &approval, false, None)
        .await
        .unwrap();
    assert_eq!(out.outcome, Decision::Deny);
    assert!(matches!(helper.await.unwrap(), HookReply::Deny { .. }));

    let p = w
        .svc()
        .prompt(&w.bot, &sid, "now the tests", Some("rq-p1"))
        .await
        .unwrap();
    assert_eq!(p.agent_status.as_deref(), Some("working"));
    let tail = w.svc().read(&sid, 20).await.unwrap();
    assert_eq!((tail.text.as_str(), tail.untrusted), ("all done\n", true));

    let replay = w.start("build the thing", "rq-1").await.unwrap();
    assert_eq!(
        (replay.session_id.as_str(), replay.outcome),
        (sid.as_str(), StartOutcome::Replayed)
    );
    assert_eq!(w.herdr.count("worktree.create"), 1);

    let stopped = w.svc().stop(&w.bot, &sid, false).await.unwrap();
    assert!(!stopped.worktree_removed);
    assert_eq!(
        w.svc()
            .get(&sid, None, Duration::ZERO)
            .await
            .unwrap()
            .lifecycle,
        Lifecycle::Stopped
    );
    let kinds: Vec<String> = w
        .svc()
        .audit_log(Some(&sid), 100)
        .await
        .unwrap()
        .into_iter()
        .map(|e| e.kind)
        .collect();
    for kind in [
        "session_start",
        "approval_requested",
        "decision",
        "prompt",
        "stop",
    ] {
        assert!(
            kinds.iter().any(|k| k == kind),
            "{kind} missing from {kinds:?}"
        );
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn connection_dropped_mid_start_reconciles_without_a_second_agent() {
    let w = World::new().await;
    w.herdr.worktree_delay_ms.store(1_500, Ordering::SeqCst);
    let svc = w.svc().clone();
    let bot = w.bot.clone();
    let start = tokio::spawn(async move {
        svc.start(
            &bot,
            StartRequest {
                project_id: "demo".into(),
                prompt: "long task".into(),
                machine_id: Some("mac".into()),
                agent: None,
                notify: true,
                request_id: Some("rq-2".into()),
            },
        )
        .await
    });
    wait_for("worktree.create", || async {
        w.herdr.count("worktree.create") == 1
    })
    .await;
    w.proxy.cut();
    let err = start.await.unwrap().unwrap_err();
    let HubError::OutcomeUnknown {
        operation_id,
        session_id: Some(sid),
    } = err
    else {
        panic!("{err:?}");
    };
    assert_eq!(operation_id, "rq-2");

    // The runner finishes the start on its own and reconnects; the hub settles it from the runner.
    wait_for("start reconciled", || async {
        w.svc()
            .get(&sid, None, Duration::ZERO)
            .await
            .unwrap()
            .lifecycle
            == Lifecycle::Running
    })
    .await;
    let replay = w.start("long task", "rq-2").await.unwrap();
    assert_eq!(replay.session_id, sid);
    let op = w
        .svc()
        .store()
        .get_operation("rq-2")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(op.state, OpState::Done);
    assert_eq!(w.herdr.count("worktree.create"), 1);
    assert_eq!(w.herdr.count("agent.start"), 1);
    assert_eq!(w.herdr.count("agent.prompt"), 1);
    let kinds: Vec<String> = w
        .svc()
        .audit_log(Some(&sid), 100)
        .await
        .unwrap()
        .into_iter()
        .map(|e| e.kind)
        .collect();
    assert!(kinds.iter().any(|k| k == "start_outcome_unknown"));
    assert!(kinds.iter().any(|k| k == "start_reconciled"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn connection_dropped_mid_approval_ends_in_deny() {
    let w = World::new().await;
    let sid = w.start("task", "rq-3").await.unwrap().session_id;
    let helper = w.ask_permission(&sid, "touch probe");
    let approval = w.pending_approval(&sid).await;
    w.proxy.cut();
    wait_for("approval denied", || async {
        let a = w
            .svc()
            .store()
            .get_approval(&approval)
            .await
            .unwrap()
            .unwrap();
        a.outcome == Some(Decision::Deny)
    })
    .await;
    let a = w
        .svc()
        .store()
        .get_approval(&approval)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(a.reason.as_deref(), Some("runner_lost"));
    // The runner reconnects still holding the approval; the hub hands it the deny at once.
    let reply = tokio::time::timeout(Duration::from_secs(10), helper)
        .await
        .unwrap()
        .unwrap();
    assert!(matches!(reply, HookReply::Deny { .. }), "{reply:?}");
    let late = w
        .svc()
        .decide(&w.bot, &sid, &approval, true, None)
        .await
        .unwrap();
    assert_eq!((late.outcome, late.replayed), (Decision::Deny, true));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn hub_restart_on_its_database_keeps_sessions_and_denies_the_open_approval() {
    let mut w = World::new().await;
    let sid = w.start("task", "rq-4").await.unwrap().session_id;
    let helper = w.ask_permission(&sid, "touch probe");
    let approval = w.pending_approval(&sid).await;

    w.hub.take().unwrap().shutdown().await;
    let hub = start_hub(&w.dir.path().join("hub.db"), false).await;
    w.proxy.point_at(hub.addr);
    w.proxy.cut();
    w.hub = Some(hub);
    w.wait_ready().await;

    let reply = tokio::time::timeout(Duration::from_secs(10), helper)
        .await
        .unwrap()
        .unwrap();
    assert!(matches!(reply, HookReply::Deny { .. }), "{reply:?}");
    let a = w
        .svc()
        .store()
        .get_approval(&approval)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(a.outcome, Some(Decision::Deny));
    let replay = w.start("task", "rq-4").await.unwrap();
    assert_eq!(
        (replay.session_id.as_str(), replay.outcome),
        (sid.as_str(), StartOutcome::Replayed)
    );
    let s = w.svc().get(&sid, None, Duration::ZERO).await.unwrap();
    assert_eq!(s.lifecycle, Lifecycle::Running);
    let p = w
        .svc()
        .prompt(&w.bot, &sid, "continue", None)
        .await
        .unwrap();
    assert_eq!(p.agent_status.as_deref(), Some("working"));
    assert_eq!(w.herdr.count("worktree.create"), 1);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn wrong_token_is_refused_at_the_upgrade() {
    let w = World::new().await;
    let url = format!("ws://{}/runner", w.hub.as_ref().unwrap().addr);
    let mut req = handshake::request(&url, "Bearer wrong-token");
    let err = handshake::connect(&mut req).await;
    assert!(err.contains("401"), "{err}");
    let mut req = handshake::request(&url, "");
    let err = handshake::connect(&mut req).await;
    assert!(err.contains("401"), "{err}");
}

/// A plain client handshake, so the test checks the hub and not the runner's own link code.
mod handshake {
    use tokio_tungstenite::tungstenite::client::IntoClientRequest;
    use tokio_tungstenite::tungstenite::handshake::client::Request;

    pub fn request(url: &str, auth: &str) -> Request {
        let mut req = url.into_client_request().unwrap();
        if !auth.is_empty() {
            req.headers_mut()
                .insert("authorization", auth.parse().unwrap());
        }
        req
    }

    pub async fn connect(req: &mut Request) -> String {
        match tokio_tungstenite::connect_async(req.clone()).await {
            Ok(_) => "connected".into(),
            Err(e) => e.to_string(),
        }
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_runner_cannot_connect_as_the_hubs_own_machine() {
    let w = World::local().await;
    let url = format!("ws://{}/runner", w.hub.as_ref().unwrap().addr);
    let mut req = handshake::request(&url, &format!("Bearer {TOKEN}"));
    let err = handshake::connect(&mut req).await;
    assert!(err.contains("401"), "{err}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_token_in_a_command_or_terminal_output_never_reaches_the_hub_api_or_database() {
    tokens_stay_masked(World::new().await).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_built_in_runner_masks_tokens_too() {
    tokens_stay_masked(World::local().await).await;
}

async fn tokens_stay_masked(w: World) {
    const TOKEN: &str = "ghp_abcdefghijklmnopqrstuvwxyz0123456789";
    let sid = w.start("task", "rq-5").await.unwrap().session_id;
    *w.herdr.screen.lock().unwrap() =
        format!("$ export GITHUB_TOKEN={TOKEN}\nlogged in with {TOKEN}\n");

    // The agent's hook hands the runner the raw command; the runner keeps it to itself.
    let helper = w.ask_permission(&sid, &format!("gh auth login --with-token {TOKEN}"));
    let approval = w.pending_approval(&sid).await;
    let view =
        serde_json::to_string(&w.svc().get(&sid, None, Duration::ZERO).await.unwrap()).unwrap();
    assert!(!view.contains(TOKEN), "{view}");
    assert!(view.contains("--with-token [redacted]"), "{view}");
    let tail = w.svc().read(&sid, 20).await.unwrap();
    assert!(!tail.text.contains(TOKEN), "{}", tail.text);
    w.svc()
        .decide(&w.bot, &sid, &approval, false, None)
        .await
        .unwrap();
    assert!(matches!(helper.await.unwrap(), HookReply::Deny { .. }));
    let audit = format!("{:?}", w.svc().audit_log(None, 500).await.unwrap());
    assert!(!audit.contains(TOKEN));

    for file in ["hub.db", "hub.db-wal"] {
        let bytes = std::fs::read(w.dir.path().join(file)).unwrap_or_default();
        assert!(
            !bytes.windows(TOKEN.len()).any(|b| b == TOKEN.as_bytes()),
            "token stored in {file}"
        );
    }
}
