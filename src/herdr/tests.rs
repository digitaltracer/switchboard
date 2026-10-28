use std::sync::{Arc, Mutex};

use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::UnixListener;

use super::*;

type Handler = dyn Fn(&Value) -> Reply + Send + Sync;

enum Reply {
    /// Lines to write, then close.
    Lines(Vec<Value>),
    /// Read the request, then drop the connection without answering.
    Drop,
    /// Write raw text verbatim, then close.
    Raw(String),
}

/// A fake Herdr server: one request per connection, like the real one.
struct Fake {
    _dir: tempfile::TempDir,
    socket: PathBuf,
    seen: Arc<Mutex<Vec<Value>>>,
}

impl Fake {
    fn start(handler: impl Fn(&Value) -> Reply + Send + Sync + 'static) -> Self {
        let dir = tempfile::tempdir().unwrap();
        let socket = dir.path().join("herdr.sock");
        let listener = UnixListener::bind(&socket).unwrap();
        let seen = Arc::new(Mutex::new(Vec::new()));
        let handler: Arc<Handler> = Arc::new(handler);
        let log = seen.clone();
        tokio::spawn(async move {
            loop {
                let (stream, _) = listener.accept().await.unwrap();
                let handler = handler.clone();
                let log = log.clone();
                tokio::spawn(async move {
                    let (read, mut write) = stream.into_split();
                    let mut line = String::new();
                    BufReader::new(read).read_line(&mut line).await.unwrap();
                    let req: Value = serde_json::from_str(&line).unwrap();
                    log.lock().unwrap().push(req.clone());
                    match handler(&req) {
                        Reply::Lines(lines) => {
                            for l in lines {
                                let l = l.to_string().replace("$ID", req["id"].as_str().unwrap());
                                write.write_all(format!("{l}\n").as_bytes()).await.unwrap();
                            }
                        }
                        Reply::Raw(text) => write.write_all(text.as_bytes()).await.unwrap(),
                        Reply::Drop => {}
                    }
                });
            }
        });
        Self {
            _dir: dir,
            socket,
            seen,
        }
    }

    fn client(&self) -> Client {
        Client::new(&self.socket).with_timeout(Duration::from_secs(2))
    }

    fn requests(&self) -> Vec<Value> {
        self.seen.lock().unwrap().clone()
    }
}

fn ok(result: Value) -> Reply {
    Reply::Lines(vec![json!({"id": "$ID", "result": result})])
}

fn pane(id: &str, ws: &str, status: &str) -> Value {
    json!({"pane_id": id, "terminal_id": "t", "workspace_id": ws, "tab_id": format!("{ws}:t1"),
           "focused": false, "agent_status": status, "revision": 0, "some_future_field": 1})
}

fn snapshot(panes: Vec<Value>, workspaces: &[&str]) -> Value {
    let ws: Vec<Value> = workspaces
        .iter()
        .map(|w| {
            json!({"workspace_id": w, "number": 1, "label": w, "focused": false,
                        "pane_count": 1, "tab_count": 1, "active_tab_id": format!("{w}:t1"),
                        "agent_status": "idle"})
        })
        .collect();
    json!({"type": "session_snapshot", "snapshot": {"version": "0.9.1", "protocol": 22,
           "workspaces": ws, "tabs": [], "panes": panes, "layouts": [], "agents": []}})
}

#[tokio::test]
async fn ping_and_protocol_check() {
    let fake = Fake::start(|_| ok(json!({"type": "pong", "version": "0.9.1", "protocol": 22})));
    assert_eq!(fake.client().check_protocol().await.unwrap(), "0.9.1");
    let req = &fake.requests()[0];
    assert_eq!(req["method"], "ping");
    assert_eq!(req["params"], json!({}));
}

#[tokio::test]
async fn refuses_unknown_protocol() {
    let fake = Fake::start(|_| ok(json!({"type": "pong", "version": "0.10.0", "protocol": 23})));
    let err = fake.client().check_protocol().await.unwrap_err();
    assert!(
        matches!(err, HerdrError::Incompatible { protocol: 23, .. }),
        "{err}"
    );
}

#[tokio::test]
async fn server_error_is_typed_and_not_outcome_unknown() {
    let fake = Fake::start(|_| {
        Reply::Lines(vec![
            json!({"id": "$ID", "error": {"code": "agent_blocked", "message": "at dialog"}}),
        ])
    });
    let err = fake
        .client()
        .agent_prompt(AgentPromptParams {
            target: "a".into(),
            text: "hi".into(),
            wait: None,
        })
        .await
        .unwrap_err();
    assert_eq!(err.server_code(), Some("agent_blocked"));
    assert!(!err.outcome_unknown());
}

#[tokio::test]
async fn dropped_connection_after_send_is_outcome_unknown() {
    let fake = Fake::start(|_| Reply::Drop);
    let err = fake.client().workspace_close("w2").await.unwrap_err();
    assert!(err.outcome_unknown(), "{err}");
}

#[tokio::test]
async fn connect_failure_is_not_outcome_unknown() {
    let dir = tempfile::tempdir().unwrap();
    let err = Client::new(dir.path().join("missing.sock"))
        .ping()
        .await
        .unwrap_err();
    assert!(matches!(err, HerdrError::Connect { .. }));
    assert!(!err.outcome_unknown());
}

#[tokio::test]
async fn mismatched_id_and_garbage_are_protocol_errors() {
    let fake = Fake::start(|_| ok(json!({"type": "ok"})));
    let client = fake.client();
    // The fake answers with the request id; force a mismatch by parsing directly.
    let err =
        parse_response("ping", "sb-1", r#"{"id":"other","result":{"type":"ok"}}"#).unwrap_err();
    assert!(matches!(err, HerdrError::Protocol { .. }));
    client.workspace_close("w2").await.unwrap();

    let garbage = Fake::start(|_| Reply::Raw("not json\n".into()));
    let err = garbage.client().ping().await.unwrap_err();
    assert!(matches!(err, HerdrError::Protocol { .. }), "{err}");
}

#[tokio::test]
async fn unknown_result_type_and_status_fall_back() {
    let fake = Fake::start(|req| match req["method"].as_str().unwrap() {
        "agent.wait" => ok(json!({"type": "agent_info", "agent": {
            "terminal_id": "t", "agent_status": "thinking_hard", "workspace_id": "w2",
            "tab_id": "w2:t1", "pane_id": "w2:p1", "focused": false, "revision": 3}})),
        _ => ok(json!({"type": "brand_new_result"})),
    });
    let client = fake.client();
    let agent = client
        .agent_wait(AgentWaitParams {
            target: "w2:p1".into(),
            until: vec![],
            timeout_ms: None,
        })
        .await
        .unwrap();
    assert_eq!(agent.agent_status, AgentStatus::Unknown);
    let err = client.ping().await.unwrap_err();
    assert!(err.to_string().contains("Other"), "{err}");
}

#[tokio::test]
async fn worktree_create_and_remove_requests() {
    let fake = Fake::start(|req| match req["method"].as_str().unwrap() {
        "worktree.create" => ok(json!({"type": "worktree_created",
            "workspace": {"workspace_id": "w2", "number": 2, "label": "sb-s1", "focused": false,
                          "pane_count": 1, "tab_count": 1, "active_tab_id": "w2:t1", "agent_status": "unknown"},
            "tab": {"tab_id": "w2:t1", "workspace_id": "w2", "number": 1, "label": "1",
                    "focused": false, "pane_count": 1, "agent_status": "unknown"},
            "root_pane": pane("w2:p1", "w2", "unknown"),
            "worktree": {"path": "/wt/sb-s1", "branch": "sb/s1", "is_bare": false, "is_detached": false,
                         "is_prunable": false, "is_linked_worktree": true, "open_workspace_id": "w2",
                         "label": "repo"}})),
        "worktree.remove" => ok(json!({"type": "worktree_removed", "workspace_id": "w2",
                                       "path": "/wt/sb-s1", "forced": false})),
        m => panic!("unexpected {m}"),
    });
    let client = fake.client();
    let created = client
        .worktree_create(WorktreeCreateParams {
            cwd: Some("/repo".into()),
            branch: Some("sb/s1".into()),
            ..Default::default()
        })
        .await
        .unwrap();
    assert_eq!(created.root_pane.pane_id, "w2:p1");
    assert_eq!(created.worktree.branch.as_deref(), Some("sb/s1"));
    assert_eq!(client.worktree_remove("w2").await.unwrap(), "/wt/sb-s1");

    let reqs = fake.requests();
    assert_eq!(
        reqs[0]["params"],
        json!({"cwd": "/repo", "branch": "sb/s1", "focus": false})
    );
    assert_eq!(reqs[1]["params"], json!({"workspace_id": "w2"}));
}

#[tokio::test]
async fn agent_start_validates_before_sending() {
    let fake = Fake::start(|_| ok(json!({"type": "ok"})));
    let client = fake.client();
    let mut params = AgentStartParams {
        name: "Bad Name".into(),
        kind: "claude".into(),
        pane_id: "w2:p1".into(),
        args: vec![],
        timeout_ms: None,
    };
    assert!(matches!(
        client.agent_start(params.clone()).await,
        Err(HerdrError::InvalidArgument(_))
    ));
    params.name = "sb-s1".into();
    params.timeout_ms = Some(3000);
    assert!(matches!(
        client.agent_start(params).await,
        Err(HerdrError::InvalidArgument(_))
    ));
    assert!(fake.requests().is_empty());
}

#[tokio::test]
async fn subscription_streams_events_then_signals_loss() {
    let fake = Fake::start(|_| {
        Reply::Lines(vec![
            json!({"id": "$ID", "result": {"type": "subscription_started"}}),
            json!({"event": "pane.agent_status_changed",
                   "data": {"pane_id": "w2:p1", "workspace_id": "w2", "agent_status": "blocked"}}),
            json!({"event": "pane_exited", "data": {"type": "pane_exited", "pane_id": "w2:p1", "workspace_id": "w2"}}),
            json!({"event": "tab_renamed", "data": {"type": "tab_renamed"}}),
        ])
    });
    let mut stream = fake
        .client()
        .subscribe(Subscription::lifecycle())
        .await
        .unwrap();
    let e1 = stream.next().await.unwrap().unwrap();
    assert_eq!(
        e1,
        Event::AgentStatusChanged {
            pane_id: "w2:p1".into(),
            workspace_id: "w2".into(),
            agent_status: AgentStatus::Blocked
        }
    );
    assert!(matches!(
        stream.next().await,
        Some(Ok(Event::PaneExited { .. }))
    ));
    assert_eq!(
        stream.next().await.unwrap().unwrap(),
        Event::Other("tab_renamed".into())
    );
    assert!(
        stream.next().await.is_none(),
        "closed stream means events may be lost"
    );
}

#[tokio::test]
async fn sync_installs_snapshot_after_ack() {
    let fake = Fake::start(|req| match req["method"].as_str().unwrap() {
        "events.subscribe" => Reply::Lines(vec![
            json!({"id": "$ID", "result": {"type": "subscription_started"}}),
            json!({"event": "pane.agent_status_changed",
                   "data": {"pane_id": "w2:p1", "workspace_id": "w2", "agent_status": "working"}}),
        ]),
        "session.snapshot" => ok(snapshot(vec![pane("w2:p1", "w2", "idle")], &["w2"])),
        m => panic!("unexpected {m}"),
    });
    let client = fake.client();
    let mut mirror = Mirror::new();
    let (mut stream, changes) = client
        .sync(&mut mirror, Subscription::lifecycle())
        .await
        .unwrap();
    assert!(changes.is_empty());
    assert!(mirror.is_synced());
    let methods: Vec<_> = fake
        .requests()
        .iter()
        .map(|r| r["method"].clone())
        .collect();
    assert_eq!(
        methods,
        vec![json!("events.subscribe"), json!("session.snapshot")]
    );

    let ev = stream.next().await.unwrap().unwrap();
    assert_eq!(
        mirror.apply(&ev),
        vec![Change::Status {
            pane_id: "w2:p1".into(),
            from: Some(AgentStatus::Idle),
            to: AgentStatus::Working
        }]
    );
}

#[test]
fn mirror_reconcile_reports_missed_changes() {
    let mut m = Mirror::new();
    let snap =
        |v: Value| -> SessionSnapshot { serde_json::from_value(v["snapshot"].clone()).unwrap() };
    m.install(&snap(snapshot(
        vec![pane("w2:p1", "w2", "working"), pane("w3:p1", "w3", "idle")],
        &["w2", "w3"],
    )));
    m.mark_lost();
    assert!(!m.is_synced());
    let changes = m.install(&snap(snapshot(
        vec![pane("w2:p1", "w2", "blocked")],
        &["w2"],
    )));
    assert_eq!(
        changes,
        vec![
            Change::WorkspaceGone {
                workspace_id: "w3".into()
            },
            Change::Status {
                pane_id: "w2:p1".into(),
                from: Some(AgentStatus::Working),
                to: AgentStatus::Blocked
            },
            Change::PaneGone {
                pane_id: "w3:p1".into()
            },
        ]
    );
    assert!(m.is_synced());
}

#[test]
fn mirror_applies_lifecycle_events() {
    let mut m = Mirror::new();
    let snap: SessionSnapshot = serde_json::from_value(
        snapshot(
            vec![pane("w2:p1", "w2", "idle"), pane("w2:p2", "w2", "idle")],
            &["w2"],
        )["snapshot"]
            .clone(),
    )
    .unwrap();
    m.install(&snap);
    let same = Event::AgentStatusChanged {
        pane_id: "w2:p1".into(),
        workspace_id: "w2".into(),
        agent_status: AgentStatus::Idle,
    };
    assert!(m.apply(&same).is_empty());
    let exit = Event::PaneExited {
        pane_id: "w2:p1".into(),
        workspace_id: "w2".into(),
    };
    assert_eq!(
        m.apply(&exit),
        vec![Change::PaneExited {
            pane_id: "w2:p1".into()
        }]
    );
    assert!(m.apply(&exit).is_empty());
    assert!(m.pane("w2:p1").unwrap().exited);
    let closed = Event::WorktreeRemoved {
        workspace_id: "w2".into(),
        path: "/wt".into(),
    };
    assert_eq!(
        m.apply(&closed),
        vec![
            Change::WorkspaceGone {
                workspace_id: "w2".into()
            },
            Change::PaneGone {
                pane_id: "w2:p1".into()
            },
            Change::PaneGone {
                pane_id: "w2:p2".into()
            },
        ]
    );
    assert!(m.pane("w2:p2").is_none());
    assert!(!m.has_workspace("w2"));
}

#[test]
fn session_socket_paths() {
    let base = Path::new("/home/u/.config/herdr");
    assert_eq!(session_socket(base, "default"), base.join("herdr.sock"));
    assert_eq!(
        session_socket(base, "sb"),
        base.join("sessions/sb/herdr.sock")
    );
}

#[test]
fn finds_the_socket_like_the_herdr_cli() {
    let base = Path::new("/home/u/.config/herdr");
    let env = |vars: &'static [(&'static str, &'static str)]| {
        move |k: &str| {
            vars.iter()
                .find(|(n, _)| *n == k)
                .map(|(_, v)| (*v).to_owned())
        }
    };
    let both = env(&[("HERDR_SOCKET_PATH", "/run/h.sock"), ("HERDR_SESSION", "x")]);
    assert_eq!(find_socket(base, None, both), Path::new("/run/h.sock"));
    let session = env(&[("HERDR_SESSION", "x"), ("HERDR_SOCKET_PATH", "")]);
    assert_eq!(
        find_socket(base, None, session),
        base.join("sessions/x/herdr.sock")
    );
    assert_eq!(find_socket(base, None, env(&[])), base.join("herdr.sock"));
    assert_eq!(
        find_socket(base, Some("test"), both),
        base.join("sessions/test/herdr.sock"),
        "a configured session overrides the environment"
    );
}

#[test]
fn a_panes_foreground_processes_parse() {
    let r: ResponseResult = serde_json::from_value(serde_json::json!({
        "type": "pane_process_info",
        "process_info": {
            "pane_id": "w2:p1", "shell_pid": 60021, "foreground_process_group_id": 31681,
            "foreground_processes": [
                {"pid": 31869, "name": "docker", "argv": ["docker", "run"]},
                {"pid": 31681, "name": "2.1.312", "argv": ["claude"], "cwd": null}
            ]
        }
    }))
    .unwrap();
    let ResponseResult::PaneProcessInfo { process_info } = r else {
        panic!("expected process info")
    };
    let pids: Vec<u32> = process_info
        .foreground_processes
        .iter()
        .map(|p| p.pid)
        .collect();
    assert_eq!(pids, vec![31869, 31681]);
}
