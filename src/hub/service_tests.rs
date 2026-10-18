use std::time::Duration;

use serde_json::json;
use tokio::task::JoinHandle;

use super::*;
use crate::proto::auth::hash_token;
use crate::proto::{Outcome, PROTOCOL_VERSION, Req, Resp};

const T0: i64 = 1_700_000_000_000;

fn config() -> HubConfig {
    let vps = hash_token("vps-token");
    let mac = hash_token("mac-token");
    HubConfig::parse(&format!(
        r#"
        public_url = "https://hub.example.com"
        listen = "127.0.0.1:0"
        database = "/var/lib/switchboard/hub.db"
        [auth]
        passphrase_hash = "$argon2id$v=19$m=19456,t=2,p=1$c2FsdA$aGFzaA"
        [[machines]]
        id = "vps"
        token_hash = "{vps}"
        [[machines]]
        id = "mac"
        token_hash = "{mac}"
        [[projects]]
        id = "switchboard"
        aliases = ["sb"]
        description = "this repo"
        repos = {{ vps = "/srv/switchboard", mac = "/Users/me/switchboard" }}
        deny_patterns = ["rm -rf /"]
        [[projects]]
        id = "maconly"
        description = "only on the mac"
        repos = {{ mac = "/Users/me/maconly" }}
        default_agent = "codex"
        "#
    ))
    .unwrap()
}

struct Rig {
    svc: Arc<SessionService>,
    clock: Clock,
    bot: Caller,
}

async fn rig_with(store: Store) -> Rig {
    let clock = Clock::manual(T0);
    Rig {
        svc: SessionService::new(config(), store, clock.clone()),
        clock,
        bot: Caller::new("mcp:test"),
    }
}

async fn rig() -> Rig {
    rig_with(Store::open_in_memory().await.unwrap()).await
}

struct FakeRunner {
    svc: Arc<SessionService>,
    machine: String,
    generation: u64,
    outbound: mpsc::Receiver<Frame>,
}

fn owned(session_id: &str, status: &str) -> OwnedSession {
    OwnedSession {
        session_id: session_id.into(),
        workspace_id: "w2".into(),
        pane_id: Some("w2:p1".into()),
        agent_status: status.into(),
        hook_failed: false,
    }
}

async fn connect(
    svc: &Arc<SessionService>,
    machine: &str,
    sessions: Vec<OwnedSession>,
) -> FakeRunner {
    let hello = Hello {
        machine_id: machine.into(),
        protocol_version: PROTOCOL_VERSION,
        runner_version: "test".into(),
        herdr_version: Some("0.9.1".into()),
        sessions,
    };
    let (generation, outbound) = svc.runner_connected(machine, hello).await;
    FakeRunner {
        svc: svc.clone(),
        machine: machine.into(),
        generation,
        outbound,
    }
}

impl FakeRunner {
    async fn req(&mut self) -> Req {
        match tokio::time::timeout(Duration::from_secs(5), self.outbound.recv())
            .await
            .expect("a request reaches the runner")
            .expect("connection open")
        {
            Frame::Req(r) => r,
            other => panic!("unexpected {other:?}"),
        }
    }

    async fn no_req(&mut self) {
        let got = tokio::time::timeout(Duration::from_millis(100), self.outbound.recv()).await;
        assert!(got.is_err(), "unexpected frame {got:?}");
    }

    async fn reply(&self, id: u64, outcome: Outcome) {
        self.svc
            .runner_frame(
                &self.machine,
                self.generation,
                Frame::Resp(Resp { id, outcome }),
            )
            .await;
    }

    async fn ok(&self, id: u64, v: Value) {
        self.reply(id, Outcome::Ok(v)).await;
    }

    async fn event(&self, ev: Event) {
        self.svc
            .runner_frame(&self.machine, self.generation, Frame::Event(ev))
            .await;
    }

    async fn drop_conn(&self) {
        self.svc
            .runner_disconnected(&self.machine, self.generation)
            .await;
    }

    /// Answers the next request, which must be a start, as the real runner would.
    async fn accept_start(&mut self) -> String {
        let req = self.req().await;
        let Op::StartSession {
            session_id, branch, ..
        } = req.op
        else {
            panic!("expected a start, got {:?}", req.op);
        };
        self.ok(
            req.id,
            json!({"session_id": session_id, "branch": branch, "worktree_path": "/wt/x",
                   "workspace_id": "w2", "pane_id": "w2:p1"}),
        )
        .await;
        session_id
    }
}

fn start_req(machine: &str, prompt: &str, request_id: Option<&str>) -> StartRequest {
    StartRequest {
        project_id: "switchboard".into(),
        prompt: prompt.into(),
        machine_id: Some(machine.into()),
        agent: None,
        notify: true,
        request_id: request_id.map(str::to_owned),
    }
}

fn spawn_start(r: &Rig, req: StartRequest) -> JoinHandle<Result<StartResult, HubError>> {
    let (svc, bot) = (r.svc.clone(), r.bot.clone());
    tokio::spawn(async move { svc.start(&bot, req).await })
}

async fn started(r: &Rig, runner: &mut FakeRunner, req: StartRequest) -> StartResult {
    let task = spawn_start(r, req);
    runner.accept_start().await;
    task.await.unwrap().unwrap()
}

async fn approval_requested(
    runner: &FakeRunner,
    session_id: &str,
    approval_id: &str,
    input: Value,
) {
    runner
        .event(Event::ApprovalRequested {
            session_id: session_id.into(),
            approval_id: approval_id.into(),
            tool_name: "Bash".into(),
            tool_input: input,
            deadline_ms: 280_000,
        })
        .await;
}

/// A running session with a fresh prompt, so the dedupe window never matches.
async fn running(r: &Rig, runner: &mut FakeRunner) -> String {
    let prompt = format!("do it {}", random_id());
    started(r, runner, start_req(&runner.machine.clone(), &prompt, None))
        .await
        .session_id
}

// Routing and validation.

#[tokio::test]
async fn start_goes_to_the_requested_machine_and_is_recorded_before_the_runner_is_asked() {
    let r = rig().await;
    let mut vps = connect(&r.svc, "vps", vec![]).await;
    let mut mac = connect(&r.svc, "mac", vec![]).await;
    let task = spawn_start(&r, start_req("mac", "fix the bug", Some("rq-1")));
    let req = mac.req().await;
    let Op::StartSession {
        session_id,
        repo_path,
        deny_patterns,
        agent,
        ..
    } = &req.op
    else {
        panic!("{:?}", req.op);
    };
    assert_eq!(repo_path, "/Users/me/switchboard");
    assert_eq!(deny_patterns, &["rm -rf /".to_string()]);
    assert_eq!(*agent, Agent::Claude);
    let op = r.svc.store().get_operation("rq-1").await.unwrap().unwrap();
    assert_eq!(op.state, OpState::Pending);
    assert_eq!(op.session_id.as_deref(), Some(session_id.as_str()));
    let s = r
        .svc
        .store()
        .get_session(session_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        (s.machine_id.as_str(), s.lifecycle),
        ("mac", Lifecycle::Starting)
    );
    vps.no_req().await;

    mac.ok(
        req.id,
        json!({"branch": format!("sb/{session_id}"), "workspace_id": "w2", "pane_id": "w2:p1"}),
    )
    .await;
    let out = task.await.unwrap().unwrap();
    assert_eq!(out.outcome, StartOutcome::Started);
    assert_eq!(out.machine_id, "mac");
    let s = r
        .svc
        .get(&out.session_id, None, Duration::ZERO)
        .await
        .unwrap();
    assert_eq!(s.lifecycle, Lifecycle::Running);
    assert_eq!(s.agent_name, Some(agent_name(&out.session_id)));
    let op = r.svc.store().get_operation("rq-1").await.unwrap().unwrap();
    assert_eq!(op.state, OpState::Done);
}

#[tokio::test]
async fn unknown_ids_and_offline_machines_fail_fast() {
    let r = rig().await;
    let mut req = start_req("vps", "x", None);
    req.project_id = "nope".into();
    assert_eq!(
        r.svc.start(&r.bot, req).await.unwrap_err().code(),
        "unknown_project"
    );
    let req = start_req("pi", "x", None);
    assert_eq!(
        r.svc.start(&r.bot, req).await.unwrap_err().code(),
        "unknown_machine"
    );
    let mut req = start_req("vps", "x", None);
    req.project_id = "maconly".into();
    assert_eq!(
        r.svc.start(&r.bot, req).await.unwrap_err().code(),
        "project_not_on_machine"
    );
    // The mac is configured but its runner is not connected: no record, no waiting.
    let err = r
        .svc
        .start(&r.bot, start_req("mac", "x", Some("rq-off")))
        .await
        .unwrap_err();
    assert_eq!(err.code(), "machine_offline");
    assert!(
        r.svc
            .store()
            .get_operation("rq-off")
            .await
            .unwrap()
            .is_none()
    );
    assert!(r.svc.list().await.unwrap().is_empty());
    let mut req = start_req("vps", "  ", None);
    req.project_id = "sb".into();
    assert_eq!(
        r.svc.start(&r.bot, req).await.unwrap_err().code(),
        "bad_request"
    );
}

#[tokio::test]
async fn aliases_resolve_to_the_project_and_the_default_machine_is_vps() {
    let r = rig().await;
    let mut vps = connect(&r.svc, "vps", vec![]).await;
    let mut req = start_req("vps", "x", None);
    req.project_id = "sb".into();
    req.machine_id = None;
    let out = started(&r, &mut vps, req).await;
    assert_eq!(
        (out.project_id.as_str(), out.machine_id.as_str()),
        ("switchboard", "vps")
    );
}

// Idempotency.

#[tokio::test]
async fn retried_request_id_returns_the_same_session_and_reuse_is_rejected() {
    let r = rig().await;
    let mut vps = connect(&r.svc, "vps", vec![]).await;
    let first = started(&r, &mut vps, start_req("vps", "task", Some("rq-1"))).await;
    // Past the dedupe window, so only the request id can match.
    r.clock.advance(Duration::from_secs(120));
    let again = r
        .svc
        .start(&r.bot, start_req("vps", "task", Some("rq-1")))
        .await
        .unwrap();
    assert_eq!(again.session_id, first.session_id);
    assert_eq!(again.outcome, StartOutcome::Replayed);
    vps.no_req().await;
    let err = r
        .svc
        .start(&r.bot, start_req("vps", "another task", Some("rq-1")))
        .await
        .unwrap_err();
    assert_eq!(err.code(), "request_id_reused");
}

#[tokio::test]
async fn retry_while_the_first_start_is_in_flight_does_not_start_twice() {
    let r = rig().await;
    let mut vps = connect(&r.svc, "vps", vec![]).await;
    let first = spawn_start(&r, start_req("vps", "task", Some("rq-1")));
    let req = vps.req().await;
    let err = r
        .svc
        .start(&r.bot, start_req("vps", "task", Some("rq-1")))
        .await
        .unwrap_err();
    assert_eq!(err.code(), "outcome_unknown");
    vps.no_req().await;
    vps.ok(req.id, json!({})).await;
    first.await.unwrap().unwrap();
}

#[tokio::test]
async fn start_dedupe_is_scoped_by_project_machine_agent_and_prompt_for_60_seconds() {
    let r = rig().await;
    let mut vps = connect(&r.svc, "vps", vec![]).await;
    let mut mac = connect(&r.svc, "mac", vec![]).await;
    let first = started(&r, &mut vps, start_req("vps", "task", None)).await;

    r.clock.advance(Duration::from_secs(59));
    let dup = r
        .svc
        .start(&r.bot, start_req("vps", "task", Some("rq-dup")))
        .await
        .unwrap();
    assert_eq!(
        (dup.session_id.as_str(), dup.outcome),
        (first.session_id.as_str(), StartOutcome::Deduplicated)
    );
    vps.no_req().await;
    // The deduplicated answer is recorded under the request id too.
    let replay = r
        .svc
        .start(&r.bot, start_req("vps", "task", Some("rq-dup")))
        .await
        .unwrap();
    assert_eq!(replay.session_id, first.session_id);

    let other_prompt = started(&r, &mut vps, start_req("vps", "task 2", None)).await;
    assert_ne!(other_prompt.session_id, first.session_id);
    let other_machine = started(&r, &mut mac, start_req("mac", "task", None)).await;
    assert_ne!(other_machine.session_id, first.session_id);
    let mut codex = start_req("vps", "task", None);
    codex.agent = Some(Agent::Codex);
    let other_agent = started(&r, &mut vps, codex).await;
    assert_ne!(other_agent.session_id, first.session_id);

    r.clock.advance(Duration::from_secs(2));
    let later = started(&r, &mut vps, start_req("vps", "task", None)).await;
    assert_ne!(later.session_id, first.session_id);
}

#[tokio::test]
async fn a_failed_start_is_not_deduplicated_and_its_failure_is_replayed() {
    let r = rig().await;
    let mut vps = connect(&r.svc, "vps", vec![]).await;
    let task = spawn_start(&r, start_req("vps", "task", Some("rq-1")));
    let req = vps.req().await;
    vps.reply(
        req.id,
        Outcome::Err(ProtoError::new(
            ErrorCode::HooksUntrusted,
            "no SessionStart",
        )),
    )
    .await;
    let err = task.await.unwrap().unwrap_err();
    assert_eq!(err.code(), "hooks_untrusted");
    let err = r
        .svc
        .start(&r.bot, start_req("vps", "task", Some("rq-1")))
        .await
        .unwrap_err();
    assert_eq!(err.code(), "hooks_untrusted");
    let retry = started(&r, &mut vps, start_req("vps", "task", None)).await;
    assert_eq!(retry.outcome, StartOutcome::Started);
}

// Connection loss during a start, and reconciliation.

#[tokio::test]
async fn drop_mid_start_is_outcome_unknown_and_the_hello_reconciles_it() {
    let r = rig().await;
    let mut mac = connect(&r.svc, "mac", vec![]).await;
    let task = spawn_start(&r, start_req("mac", "task", Some("rq-1")));
    let req = mac.req().await;
    let Op::StartSession { session_id, .. } = req.op else {
        panic!()
    };
    mac.drop_conn().await;
    let err = task.await.unwrap().unwrap_err();
    let HubError::OutcomeUnknown {
        operation_id,
        session_id: sid,
    } = &err
    else {
        panic!("{err:?}");
    };
    assert_eq!(
        (operation_id.as_str(), sid.as_deref()),
        ("rq-1", Some(session_id.as_str()))
    );
    assert_eq!(
        r.svc
            .start(&r.bot, start_req("mac", "other", None))
            .await
            .unwrap_err()
            .code(),
        "machine_offline"
    );
    assert_eq!(
        r.svc
            .start(&r.bot, start_req("mac", "task", Some("rq-1")))
            .await
            .unwrap_err()
            .code(),
        "outcome_unknown"
    );

    // The runner finished the start anyway; its hello says so.
    let mut mac = connect(&r.svc, "mac", vec![owned(&session_id, "working")]).await;
    let s = r.svc.get(&session_id, None, Duration::ZERO).await.unwrap();
    assert_eq!(
        (s.lifecycle, s.agent_status.as_str()),
        (Lifecycle::Running, "working")
    );
    let replay = r
        .svc
        .start(&r.bot, start_req("mac", "task", Some("rq-1")))
        .await
        .unwrap();
    assert_eq!(replay.session_id, session_id);
    let next = started(&r, &mut mac, start_req("mac", "other", None)).await;
    assert_ne!(next.session_id, session_id);
    mac.no_req().await;
}

#[tokio::test]
async fn a_start_missing_from_the_hello_blocks_new_starts_until_settled() {
    let r = rig().await;
    let mut mac = connect(&r.svc, "mac", vec![]).await;
    let task = spawn_start(&r, start_req("mac", "task", Some("rq-1")));
    let req = mac.req().await;
    let Op::StartSession { session_id, .. } = req.op else {
        panic!()
    };
    mac.drop_conn().await;
    task.await.unwrap().unwrap_err();

    let mut mac = connect(&r.svc, "mac", vec![]).await;
    assert_eq!(
        r.svc
            .start(&r.bot, start_req("mac", "other", None))
            .await
            .unwrap_err()
            .code(),
        "reconciling"
    );
    // Too early to call it failed: the runner may still be creating the worktree.
    let svc = r.svc.clone();
    let check = tokio::spawn(async move { svc.recheck("mac").await });
    let list = mac.req().await;
    assert_eq!(list.op, Op::ListSessions);
    mac.ok(list.id, json!([])).await;
    check.await.unwrap().unwrap();
    assert_eq!(r.svc.readiness("mac"), Readiness::Ready);
    assert_eq!(
        r.svc
            .start(&r.bot, start_req("mac", "other", None))
            .await
            .unwrap_err()
            .code(),
        "reconciling"
    );

    r.clock.advance(START_TIMEOUT);
    let svc = r.svc.clone();
    let check = tokio::spawn(async move { svc.recheck("mac").await });
    let list = mac.req().await;
    mac.ok(list.id, json!([])).await;
    check.await.unwrap().unwrap();
    let s = r.svc.get(&session_id, None, Duration::ZERO).await.unwrap();
    assert_eq!(s.lifecycle, Lifecycle::Failed);
    let err = r
        .svc
        .start(&r.bot, start_req("mac", "task", Some("rq-1")))
        .await
        .unwrap_err();
    assert_eq!(err.code(), "start_failed");
    started(&r, &mut mac, start_req("mac", "other", None)).await;
}

#[tokio::test]
async fn the_runners_own_audit_settles_an_unresolved_start() {
    let r = rig().await;
    let mut mac = connect(&r.svc, "mac", vec![]).await;
    let task = spawn_start(&r, start_req("mac", "task", Some("rq-1")));
    let Op::StartSession { session_id, .. } = mac.req().await.op else {
        panic!()
    };
    mac.drop_conn().await;
    task.await.unwrap().unwrap_err();
    let mac = connect(&r.svc, "mac", vec![]).await;
    mac.event(Event::Audit {
        session_id: session_id.clone(),
        kind: "prompt".into(),
        detail: json!({"initial": true}),
    })
    .await;
    let s = r.svc.get(&session_id, None, Duration::ZERO).await.unwrap();
    assert_eq!(s.lifecycle, Lifecycle::Running);
    let replay = r
        .svc
        .start(&r.bot, start_req("mac", "task", Some("rq-1")))
        .await
        .unwrap();
    assert_eq!(replay.session_id, session_id);
}

#[tokio::test]
async fn hello_settles_running_and_stopping_sessions_the_runner_no_longer_has() {
    let r = rig().await;
    let mut vps = connect(&r.svc, "vps", vec![]).await;
    let a = running(&r, &mut vps).await;
    let b = running(&r, &mut vps).await;
    let svc = r.svc.clone();
    let bot = r.bot.clone();
    let b2 = b.clone();
    let stop = tokio::spawn(async move { svc.stop(&bot, &b2, false).await });
    vps.req().await;
    vps.drop_conn().await;
    assert_eq!(stop.await.unwrap().unwrap_err().code(), "outcome_unknown");
    connect(&r.svc, "vps", vec![]).await;
    let a = r.svc.get(&a, None, Duration::ZERO).await.unwrap();
    assert_eq!(a.lifecycle, Lifecycle::Exited);
    let b = r.svc.get(&b, None, Duration::ZERO).await.unwrap();
    assert_eq!(b.lifecycle, Lifecycle::Stopped);
}

#[tokio::test]
async fn stale_generation_replies_and_events_are_ignored() {
    let r = rig().await;
    let mut old = connect(&r.svc, "vps", vec![]).await;
    let sid = running(&r, &mut old).await;
    let svc = r.svc.clone();
    let s2 = sid.clone();
    let prompt = tokio::spawn(async move { svc.read(&s2, 10).await });
    let req = old.req().await;
    let new = connect(&r.svc, "vps", vec![owned(&sid, "idle")]).await;
    // The old connection's reply is dropped; the read already failed when the new one attached.
    old.ok(req.id, json!({"text": "stale"})).await;
    assert_eq!(prompt.await.unwrap().unwrap_err().code(), "machine_offline");
    old.event(Event::AgentStatus {
        session_id: sid.clone(),
        agent_status: "blocked".into(),
        revision: 0,
    })
    .await;
    assert_eq!(
        r.svc
            .get(&sid, None, Duration::ZERO)
            .await
            .unwrap()
            .agent_status,
        "idle"
    );
    // Losing the old connection doesn't take the machine offline.
    old.drop_conn().await;
    assert_eq!(r.svc.readiness("vps"), Readiness::Ready);
    new.event(Event::AgentStatus {
        session_id: sid.clone(),
        agent_status: "blocked".into(),
        revision: 0,
    })
    .await;
    assert_eq!(
        r.svc
            .get(&sid, None, Duration::ZERO)
            .await
            .unwrap()
            .agent_status,
        "blocked"
    );
}

#[tokio::test]
async fn a_runner_cannot_touch_another_machines_sessions() {
    let r = rig().await;
    let mut vps = connect(&r.svc, "vps", vec![]).await;
    let mut mac = connect(&r.svc, "mac", vec![]).await;
    let sid = running(&r, &mut vps).await;
    mac.event(Event::AgentStatus {
        session_id: sid.clone(),
        agent_status: "done".into(),
        revision: 0,
    })
    .await;
    mac.event(Event::SessionExited {
        session_id: sid.clone(),
    })
    .await;
    approval_requested(&mac, &sid, "a1", json!({"command": "ls"})).await;
    let s = r.svc.get(&sid, None, Duration::ZERO).await.unwrap();
    assert_eq!(
        (s.lifecycle, s.agent_status.as_str()),
        (Lifecycle::Running, "unknown")
    );
    assert!(s.pending_approvals.is_empty());
    let deny = mac.req().await;
    assert!(matches!(deny.op, Op::Decide { allow: false, .. }));
}

// Approvals.

async fn with_approval(r: &Rig) -> (FakeRunner, String) {
    let mut vps = connect(&r.svc, "vps", vec![]).await;
    let sid = running(r, &mut vps).await;
    approval_requested(&vps, &sid, "a1", json!({"command": "touch x"})).await;
    (vps, sid)
}

async fn decide(r: &Rig, sid: &str, allow: bool) -> JoinHandle<Result<DecideResult, HubError>> {
    let (svc, bot, sid) = (r.svc.clone(), r.bot.clone(), sid.to_owned());
    tokio::spawn(async move { svc.decide(&bot, &sid, "a1", allow, None).await })
}

#[tokio::test]
async fn approve_then_replay_returns_the_recorded_outcome_without_resending() {
    let r = rig().await;
    let (mut vps, sid) = with_approval(&r).await;
    let s = r.svc.get(&sid, None, Duration::ZERO).await.unwrap();
    assert_eq!(s.pending_approvals.len(), 1);
    assert_eq!(
        s.pending_approvals[0].tool_input,
        json!({"command": "touch x"})
    );
    assert_eq!(s.pending_approvals[0].deadline, T0 + 280_000 - 5_000);

    let task = decide(&r, &sid, true).await;
    let req = vps.req().await;
    assert_eq!(
        req.op,
        Op::Decide {
            session_id: sid.clone(),
            approval_id: "a1".into(),
            allow: true
        }
    );
    vps.ok(req.id, json!({"applied": true})).await;
    let first = task.await.unwrap().unwrap();
    assert_eq!(
        (first.outcome, first.delivery, first.replayed),
        (Decision::Allow, Some(Delivery::Delivered), false)
    );

    let replay = r.svc.decide(&r.bot, &sid, "a1", false, None).await.unwrap();
    assert_eq!((replay.outcome, replay.replayed), (Decision::Allow, true));
    vps.no_req().await;
}

#[tokio::test]
async fn deny_is_delivered_and_recorded() {
    let r = rig().await;
    let (mut vps, sid) = with_approval(&r).await;
    let task = decide(&r, &sid, false).await;
    let req = vps.req().await;
    assert!(matches!(req.op, Op::Decide { allow: false, .. }));
    vps.ok(req.id, json!({})).await;
    let out = task.await.unwrap().unwrap();
    assert_eq!(out.outcome, Decision::Deny);
}

#[tokio::test]
async fn unanswered_approval_is_denied_at_its_deadline_and_the_runner_told() {
    let r = rig().await;
    let (mut vps, sid) = with_approval(&r).await;
    r.clock.advance(Duration::from_millis(274_999));
    assert_eq!(r.svc.sweep().await.unwrap(), 0);
    r.clock.advance(Duration::from_millis(1));
    assert_eq!(r.svc.sweep().await.unwrap(), 1);
    let a = r.svc.store().get_approval("a1").await.unwrap().unwrap();
    assert_eq!(
        (a.outcome, a.reason.as_deref()),
        (Some(Decision::Deny), Some("expired"))
    );
    let req = vps.req().await;
    assert!(matches!(req.op, Op::Decide { allow: false, .. }));
    // A late approve gets the recorded deny.
    let late = r.svc.decide(&r.bot, &sid, "a1", true, None).await.unwrap();
    assert_eq!((late.outcome, late.replayed), (Decision::Deny, true));
}

#[tokio::test]
async fn deciding_an_expired_approval_before_the_sweep_denies_it() {
    let r = rig().await;
    let (mut vps, sid) = with_approval(&r).await;
    r.clock.advance(Duration::from_secs(276));
    let task = decide(&r, &sid, true).await;
    let req = vps.req().await;
    assert!(
        matches!(req.op, Op::Decide { allow: false, .. }),
        "an expired allow is sent as a deny"
    );
    vps.ok(req.id, json!({})).await;
    let out = task.await.unwrap().unwrap();
    assert_eq!(
        (out.outcome, out.reason.as_deref()),
        (Decision::Deny, Some("expired"))
    );
}

#[tokio::test]
async fn runner_loss_denies_pending_approvals_and_redelivers_on_reconnect() {
    let r = rig().await;
    let (vps, sid) = with_approval(&r).await;
    vps.drop_conn().await;
    let a = r.svc.store().get_approval("a1").await.unwrap().unwrap();
    assert_eq!(
        (a.outcome, a.reason.as_deref()),
        (Some(Decision::Deny), Some("runner_lost"))
    );
    let late = r.svc.decide(&r.bot, &sid, "a1", true, None).await.unwrap();
    assert_eq!((late.outcome, late.replayed), (Decision::Deny, true));

    let mut vps = connect(&r.svc, "vps", vec![owned(&sid, "blocked")]).await;
    let req = vps.req().await;
    assert_eq!(
        req.op,
        Op::Decide {
            session_id: sid.clone(),
            approval_id: "a1".into(),
            allow: false
        }
    );
    vps.ok(req.id, json!({})).await;
    tokio::time::sleep(Duration::from_millis(50)).await;
    let a = r.svc.store().get_approval("a1").await.unwrap().unwrap();
    assert_eq!(a.delivery, Some(Delivery::Delivered));
}

#[tokio::test]
async fn hub_restart_denies_pending_approvals_and_marks_starts_unknown() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("hub.db");
    let r = rig_with(Store::open(&path).await.unwrap()).await;
    let (mut vps, sid) = with_approval(&r).await;
    let _task = spawn_start(&r, start_req("vps", "second", Some("rq-2")));
    let Op::StartSession {
        session_id: second, ..
    } = vps.req().await.op
    else {
        panic!()
    };
    drop(r);

    let r = rig_with(Store::open(&path).await.unwrap()).await;
    r.svc.recover().await.unwrap();
    let a = r.svc.store().get_approval("a1").await.unwrap().unwrap();
    assert_eq!(
        (a.outcome, a.reason.as_deref()),
        (Some(Decision::Deny), Some("hub_restart"))
    );
    let op = r.svc.store().get_operation("rq-2").await.unwrap().unwrap();
    assert_eq!(op.state, OpState::OutcomeUnknown);
    // The runner reconnects to the new hub, still holding the approval and the finished start.
    let mut vps = connect(
        &r.svc,
        "vps",
        vec![owned(&sid, "blocked"), owned(&second, "working")],
    )
    .await;
    let req = vps.req().await;
    assert!(matches!(req.op, Op::Decide { allow: false, .. }));
    let replay = r
        .svc
        .start(&r.bot, start_req("vps", "second", Some("rq-2")))
        .await
        .unwrap();
    assert_eq!(replay.session_id, second);
    let audit = r.svc.audit_log(Some(&sid), 50).await.unwrap();
    assert!(
        audit
            .iter()
            .any(|e| e.kind == "approval_denied" && e.detail["reason"] == "hub_restart")
    );
}

#[tokio::test]
async fn an_allow_recorded_before_a_hub_crash_is_reported_as_unknown_delivery() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("hub.db");
    let r = rig_with(Store::open(&path).await.unwrap()).await;
    let (mut vps, sid) = with_approval(&r).await;
    let _task = decide(&r, &sid, true).await;
    vps.req().await;
    drop(r);

    let r = rig_with(Store::open(&path).await.unwrap()).await;
    r.svc.recover().await.unwrap();
    let a = r.svc.store().get_approval("a1").await.unwrap().unwrap();
    assert_eq!(
        (a.outcome, a.delivery),
        (Some(Decision::Allow), Some(Delivery::Unknown))
    );
}

#[tokio::test]
async fn an_allow_the_runner_had_already_closed_is_recorded_as_deny() {
    let r = rig().await;
    let (mut vps, sid) = with_approval(&r).await;
    let task = decide(&r, &sid, true).await;
    let req = vps.req().await;
    vps.reply(
        req.id,
        Outcome::Err(ProtoError::new(
            ErrorCode::NotFound,
            "approval a1 is not pending",
        )),
    )
    .await;
    let out = task.await.unwrap().unwrap();
    assert_eq!(
        (out.outcome, out.reason.as_deref(), out.delivery),
        (
            Decision::Deny,
            Some("not_delivered"),
            Some(Delivery::NotDelivered)
        )
    );
    let replay = r.svc.decide(&r.bot, &sid, "a1", true, None).await.unwrap();
    assert_eq!(replay.outcome, Decision::Deny);
}

#[tokio::test]
async fn an_allow_lost_with_the_connection_is_reported_as_unknown_delivery() {
    let r = rig().await;
    let (mut vps, sid) = with_approval(&r).await;
    let task = decide(&r, &sid, true).await;
    vps.req().await;
    vps.drop_conn().await;
    let out = task.await.unwrap().unwrap();
    assert_eq!(
        (out.outcome, out.delivery),
        (Decision::Allow, Some(Delivery::Unknown))
    );
}

#[tokio::test]
async fn approvals_the_runner_closes_are_denied() {
    let r = rig().await;
    let (vps, sid) = with_approval(&r).await;
    vps.event(Event::ApprovalClosed {
        session_id: sid.clone(),
        approval_id: "a1".into(),
        reason: "hook_helper_gone".into(),
    })
    .await;
    let out = r.svc.decide(&r.bot, &sid, "a1", true, None).await.unwrap();
    assert_eq!(
        (out.outcome, out.reason.as_deref(), out.replayed),
        (Decision::Deny, Some("hook_helper_gone"), true)
    );
}

#[tokio::test]
async fn decide_rejects_unknown_and_mismatched_approval_ids() {
    let r = rig().await;
    let (mut vps, sid) = with_approval(&r).await;
    let other = running(&r, &mut vps).await;
    let err = r
        .svc
        .decide(&r.bot, &sid, "zzz", true, None)
        .await
        .unwrap_err();
    assert_eq!(err.code(), "unknown_approval");
    let err = r
        .svc
        .decide(&r.bot, &other, "a1", true, None)
        .await
        .unwrap_err();
    assert_eq!(err.code(), "unknown_approval");
    let err = r
        .svc
        .decide(&r.bot, "nope", "a1", true, None)
        .await
        .unwrap_err();
    assert_eq!(err.code(), "unknown_session");
}

#[tokio::test]
async fn decide_request_id_replays_and_rejects_reuse() {
    let r = rig().await;
    let (mut vps, sid) = with_approval(&r).await;
    let (svc, bot, s2) = (r.svc.clone(), r.bot.clone(), sid.clone());
    let task = tokio::spawn(async move { svc.decide(&bot, &s2, "a1", false, Some("rq-d")).await });
    let req = vps.req().await;
    vps.ok(req.id, json!({})).await;
    task.await.unwrap().unwrap();
    let replay = r
        .svc
        .decide(&r.bot, &sid, "a1", false, Some("rq-d"))
        .await
        .unwrap();
    assert!(replay.replayed);
    let err = r
        .svc
        .decide(&r.bot, &sid, "a1", true, Some("rq-d"))
        .await
        .unwrap_err();
    assert_eq!(err.code(), "request_id_reused");
}

#[tokio::test]
async fn stop_and_exit_deny_pending_approvals() {
    let r = rig().await;
    let (mut vps, sid) = with_approval(&r).await;
    let (svc, bot, s2) = (r.svc.clone(), r.bot.clone(), sid.clone());
    let stop = tokio::spawn(async move { svc.stop(&bot, &s2, true).await });
    let req = vps.req().await;
    assert_eq!(
        req.op,
        Op::Stop {
            session_id: sid.clone(),
            remove_worktree_if_clean: true
        }
    );
    let a = r.svc.store().get_approval("a1").await.unwrap().unwrap();
    assert_eq!(
        (a.outcome, a.reason.as_deref()),
        (Some(Decision::Deny), Some("stopped"))
    );
    vps.ok(
        req.id,
        json!({"worktree_removed": false, "kept_because": "dirty"}),
    )
    .await;
    let out = stop.await.unwrap().unwrap();
    assert_eq!(out.kept_because.as_deref(), Some("dirty"));
    assert!(
        r.svc
            .stop(&r.bot, &sid, false)
            .await
            .unwrap()
            .already_stopped
    );

    let other = running(&r, &mut vps).await;
    approval_requested(&vps, &other, "a2", json!({})).await;
    vps.event(Event::SessionExited {
        session_id: other.clone(),
    })
    .await;
    let a = r.svc.store().get_approval("a2").await.unwrap().unwrap();
    assert_eq!(a.reason.as_deref(), Some("session_exited"));
    assert_eq!(
        r.svc
            .get(&other, None, Duration::ZERO)
            .await
            .unwrap()
            .lifecycle,
        Lifecycle::Exited
    );
}

// Prompts and reads.

#[tokio::test]
async fn prompt_requires_a_running_session_and_replays_by_request_id() {
    let r = rig().await;
    let mut vps = connect(&r.svc, "vps", vec![]).await;
    let sid = running(&r, &mut vps).await;
    let (svc, bot, s2) = (r.svc.clone(), r.bot.clone(), sid.clone());
    let task = tokio::spawn(async move { svc.prompt(&bot, &s2, "next step", Some("rq-p")).await });
    let req = vps.req().await;
    assert_eq!(
        req.op,
        Op::Prompt {
            session_id: sid.clone(),
            text: "next step".into()
        }
    );
    vps.ok(req.id, json!({"agent_status": "working"})).await;
    task.await.unwrap().unwrap();
    let replay = r
        .svc
        .prompt(&r.bot, &sid, "next step", Some("rq-p"))
        .await
        .unwrap();
    assert_eq!(replay.agent_status.as_deref(), Some("working"));
    vps.no_req().await;

    let (svc, bot, s2) = (r.svc.clone(), r.bot.clone(), sid.clone());
    let task = tokio::spawn(async move { svc.prompt(&bot, &s2, "again", Some("rq-q")).await });
    vps.req().await;
    vps.drop_conn().await;
    assert_eq!(task.await.unwrap().unwrap_err().code(), "outcome_unknown");
    assert_eq!(
        r.svc
            .prompt(&r.bot, &sid, "again", Some("rq-q"))
            .await
            .unwrap_err()
            .code(),
        "outcome_unknown",
        "a prompt that may have been delivered is never resent"
    );
    assert_eq!(
        r.svc
            .prompt(&r.bot, &sid, "x", None)
            .await
            .unwrap_err()
            .code(),
        "machine_offline"
    );
}

#[tokio::test]
async fn get_waits_for_a_newer_revision() {
    let r = rig().await;
    let mut vps = connect(&r.svc, "vps", vec![]).await;
    let sid = running(&r, &mut vps).await;
    let rev = r
        .svc
        .get(&sid, None, Duration::ZERO)
        .await
        .unwrap()
        .revision;
    let unchanged = r
        .svc
        .get(&sid, Some(rev), Duration::from_millis(50))
        .await
        .unwrap();
    assert_eq!(unchanged.revision, rev);
    let (svc, s2) = (r.svc.clone(), sid.clone());
    let waiter = tokio::spawn(async move { svc.get(&s2, Some(rev), Duration::from_secs(5)).await });
    tokio::time::sleep(Duration::from_millis(20)).await;
    vps.event(Event::AgentStatus {
        session_id: sid.clone(),
        agent_status: "blocked".into(),
        revision: 0,
    })
    .await;
    let got = waiter.await.unwrap().unwrap();
    assert!(got.revision > rev);
    assert_eq!(got.agent_status, "blocked");
}

// Audit.

#[tokio::test]
async fn audit_records_prompts_decisions_and_runner_events_with_secrets_redacted() {
    let r = rig().await;
    let mut vps = connect(&r.svc, "vps", vec![]).await;
    let first = started(
        &r,
        &mut vps,
        start_req("vps", "deploy with API_KEY=abc123", None),
    )
    .await;
    let sid = first.session_id;
    approval_requested(
        &vps,
        &sid,
        "a1",
        json!({"command": "curl -H 'Authorization: Bearer abcdefghijkl' x", "password": "pw"}),
    )
    .await;
    let task = decide(&r, &sid, false).await;
    let req = vps.req().await;
    vps.ok(req.id, json!({})).await;
    task.await.unwrap().unwrap();
    vps.event(Event::HookFailed {
        session_id: sid.clone(),
        reason: "unrecognised".into(),
        screen_excerpt: Some("export TOKEN=xyz".into()),
    })
    .await;

    let log = r.svc.audit_log(Some(&sid), 50).await.unwrap();
    let text = serde_json::to_string(&log.iter().map(|e| &e.detail).collect::<Vec<_>>()).unwrap();
    for secret in ["abc123", "abcdefghijkl", "\"pw\"", "xyz"] {
        assert!(!text.contains(secret), "{secret} leaked: {text}");
    }
    let kinds: Vec<&str> = log.iter().map(|e| e.kind.as_str()).collect();
    for kind in [
        "session_start",
        "approval_requested",
        "decision",
        "hook_failed",
    ] {
        assert!(kinds.contains(&kind), "{kind} missing from {kinds:?}");
    }
    let start = log.iter().find(|e| e.kind == "session_start").unwrap();
    assert_eq!(start.actor, "mcp:test");
    assert_eq!(start.detail["prompt"], "deploy with API_KEY=[redacted]");
    let requested = log.iter().find(|e| e.kind == "approval_requested").unwrap();
    assert_eq!(requested.actor, "runner:vps");
    // Only the masked input is stored, and that is what a bot decides on.
    let stored = r.svc.store().get_approval("a1").await.unwrap().unwrap();
    assert_eq!(stored.tool_input["password"], "[redacted]");
    assert!(
        r.svc
            .get(&sid, None, Duration::ZERO)
            .await
            .unwrap()
            .hook_failed
    );
}

// Notifications.

#[tokio::test]
async fn notify_state_is_shared_and_survives_a_restart() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("hub.db");
    let r = rig_with(Store::open(&path).await.unwrap()).await;
    let mut vps = connect(&r.svc, "vps", vec![]).await;
    let mut quiet = start_req("vps", "quiet", None);
    quiet.notify = false;
    let quiet = started(&r, &mut vps, quiet).await;
    assert!(quiet.muted);
    let loud = started(&r, &mut vps, start_req("vps", "loud", None)).await;
    let view = r
        .svc
        .get(&loud.session_id, None, Duration::ZERO)
        .await
        .unwrap();
    assert!(view.notifies);

    let n = r.svc.notify(&r.bot, &loud.session_id, true).await.unwrap();
    assert!(n.muted && !n.notifies);
    let n = r.svc.notify(&r.bot, &loud.session_id, false).await.unwrap();
    assert!(!n.muted && n.notifies);

    let wa = Caller::new("whatsapp");
    let loud_agent = loud.agent_name.clone().unwrap();
    let state = r.svc.set_agent_muted(&wa, &loud_agent, true).await.unwrap();
    assert!(state.muted.contains(&loud_agent));
    assert!(
        r.svc
            .get(&loud.session_id, None, Duration::ZERO)
            .await
            .unwrap()
            .muted,
        "a WhatsApp mute shows up for bots"
    );
    r.svc
        .set_agent_muted(&wa, &loud_agent, false)
        .await
        .unwrap();
    r.svc
        .set_agent_watched(&wa, "switchboard-impl", true)
        .await
        .unwrap();
    r.svc.set_agent_muted(&wa, "reviewer", true).await.unwrap();
    let state = r.svc.set_mute_all(&wa, true).await.unwrap();
    assert!(state.mute_all);
    assert!(
        !state.notifies(&loud_agent),
        "mute all covers every session"
    );
    let n = r.svc.notify(&r.bot, &loud.session_id, false).await.unwrap();
    assert!(!n.muted && n.mute_all && !n.notifies);
    drop(r);

    let r = rig_with(Store::open(&path).await.unwrap()).await;
    let state = r.svc.notify_state().await.unwrap();
    assert!(state.mute_all);
    assert!(state.watched.contains("switchboard-impl"));
    assert!(state.muted.contains(quiet.agent_name.as_ref().unwrap()));
    assert!(state.muted.contains("reviewer"));
    assert!(!state.muted.contains(&loud_agent));
}

#[tokio::test]
async fn the_bridge_watch_file_is_imported_once() {
    let r = rig().await;
    let mut vps = connect(&r.svc, "vps", vec![]).await;
    let s = started(&r, &mut vps, start_req("vps", "x", None)).await;
    let name = s.agent_name.clone().unwrap();
    let mut file = WatchState::default();
    file.watched.insert("switchboard-impl".into());
    file.muted.insert(name.clone());
    file.muted.insert("other".into());
    let wa = Caller::new("whatsapp");
    assert!(r.svc.import_watch_state(&wa, &file).await.unwrap());
    assert!(
        r.svc
            .get(&s.session_id, None, Duration::ZERO)
            .await
            .unwrap()
            .muted
    );
    let state = r.svc.notify_state().await.unwrap();
    assert!(state.watched.contains("switchboard-impl") && state.muted.contains("other"));
    file.mute_all = true;
    assert!(!r.svc.import_watch_state(&wa, &file).await.unwrap());
    assert!(!r.svc.notify_state().await.unwrap().mute_all);
}

#[tokio::test]
async fn machines_and_projects_are_listed() {
    let r = rig().await;
    let _vps = connect(&r.svc, "vps", vec![]).await;
    let machines = r.svc.machines();
    let vps = machines.iter().find(|m| m.id == "vps").unwrap();
    assert!(vps.connected && vps.readiness == Readiness::Ready);
    assert_eq!(
        vps.runner.as_ref().unwrap().herdr_version.as_deref(),
        Some("0.9.1")
    );
    assert!(!machines.iter().find(|m| m.id == "mac").unwrap().connected);
    let projects = r.svc.projects();
    let sb = projects.iter().find(|p| p.id == "switchboard").unwrap();
    assert_eq!(sb.machines, ["mac", "vps"]);
}

// Masking: nothing that leaves the hub, or is stored by it, carries a secret.

const TOKEN: &str = "ghp_abcdefghijklmnopqrstuvwxyz0123456789";

fn bytes_contain(path: &std::path::Path, needle: &str) -> bool {
    std::fs::read(path)
        .map(|b| b.windows(needle.len()).any(|w| w == needle.as_bytes()))
        .unwrap_or(false)
}

#[tokio::test]
async fn a_token_in_a_command_or_terminal_output_never_reaches_callers_or_the_database() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("hub.db");
    let r = rig_with(Store::open(&path).await.unwrap()).await;
    let mut vps = connect(&r.svc, "vps", vec![]).await;
    let sid = running(&r, &mut vps).await;

    approval_requested(
        &vps,
        &sid,
        "a1",
        json!({"command": format!("curl -H 'Authorization: Bearer {TOKEN}' https://api"),
               "env": {"GITHUB_TOKEN": TOKEN}}),
    )
    .await;
    vps.event(Event::HookFailed {
        session_id: sid.clone(),
        reason: "unrecognised".into(),
        screen_excerpt: Some(format!("$ export GITHUB_TOKEN={TOKEN}")),
    })
    .await;
    let view =
        serde_json::to_string(&r.svc.get(&sid, None, Duration::ZERO).await.unwrap()).unwrap();
    assert!(!view.contains(TOKEN), "{view}");
    assert!(view.contains("Bearer [redacted]"), "{view}");

    let (svc, s2) = (r.svc.clone(), sid.clone());
    let read = tokio::spawn(async move { svc.read(&s2, 20).await });
    let req = vps.req().await;
    vps.ok(
        req.id,
        json!({"text": format!("$ echo {TOKEN}\n{TOKEN}\n")}),
    )
    .await;
    let tail = read.await.unwrap().unwrap();
    assert!(!tail.text.contains(TOKEN), "{}", tail.text);

    let (svc, bot, s2) = (r.svc.clone(), r.bot.clone(), sid.clone());
    let prompt = tokio::spawn(async move { svc.prompt(&bot, &s2, "go on", None).await });
    let req = vps.req().await;
    let screen = format!("agent is blocked; screen: export API_KEY={TOKEN}");
    vps.reply(
        req.id,
        Outcome::Err(ProtoError::new(ErrorCode::AgentBlocked, screen)),
    )
    .await;
    let err = prompt.await.unwrap().unwrap_err().to_string();
    assert!(!err.contains(TOKEN), "{err}");

    let (svc, bot, s2) = (r.svc.clone(), r.bot.clone(), sid.clone());
    let decide = tokio::spawn(async move { svc.decide(&bot, &s2, "a1", false, None).await });
    let req = vps.req().await;
    vps.ok(req.id, json!({})).await;
    let out = serde_json::to_string(&decide.await.unwrap().unwrap()).unwrap();
    assert!(!out.contains(TOKEN));
    let audit = serde_json::to_string(
        &r.svc
            .audit_log(None, 500)
            .await
            .unwrap()
            .iter()
            .map(|e| &e.detail)
            .collect::<Vec<_>>(),
    )
    .unwrap();
    assert!(!audit.contains(TOKEN), "{audit}");

    drop(r);
    for file in ["hub.db", "hub.db-wal"] {
        assert!(
            !bytes_contain(&dir.path().join(file), TOKEN),
            "token stored in {file}"
        );
    }
}
