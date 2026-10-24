use std::io::Write;
use std::net::SocketAddr;
use std::path::PathBuf;

use serde_json::json;

use super::*;
use crate::hook::ask_runner;
use crate::hook::wire::SecretFile;

struct Rig {
    hub: Arc<HookHub>,
    events: mpsc::Receiver<Event>,
    addr: SocketAddr,
    dir: tempfile::TempDir,
}

async fn rig(deadline: Duration) -> Rig {
    let (tx, events) = mpsc::channel(64);
    let hub = HookHub::new(tx, deadline);
    hub.register("s1", "good-token", Agent::Codex, vec!["git push".into()]);
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(hub.clone().serve(listener));
    Rig {
        hub,
        events,
        addr,
        dir: tempfile::tempdir().unwrap(),
    }
}

impl Rig {
    fn secret(&self, token: &str) -> PathBuf {
        let path = self.dir.path().join(format!("secret-{token}"));
        SecretFile {
            addr: self.addr,
            session_id: "s1".into(),
            token: token.into(),
        }
        .write(&path)
        .unwrap();
        path
    }

    /// Runs the real helper client on a blocking thread.
    fn ask(
        &self,
        token: &str,
        agent: Agent,
        event: &str,
        payload: Value,
    ) -> tokio::task::JoinHandle<Result<HookReply, String>> {
        let path = self.secret(token);
        let event = event.to_owned();
        tokio::task::spawn_blocking(move || {
            ask_runner(&path, agent, &event, &payload, Duration::from_secs(10))
        })
    }

    async fn next_event(&mut self) -> Event {
        tokio::time::timeout(Duration::from_secs(5), self.events.recv())
            .await
            .unwrap()
            .unwrap()
    }
}

fn bash(cmd: &str) -> Value {
    json!({"hook_event_name": "PermissionRequest", "tool_name": "Bash", "tool_input": {"command": cmd}})
}

#[tokio::test]
async fn bad_token_or_wrong_agent_is_denied() {
    let r = rig(Duration::from_secs(5)).await;
    let reply = r
        .ask("wrong", Agent::Codex, "PermissionRequest", bash("ls"))
        .await
        .unwrap()
        .unwrap();
    assert!(matches!(reply, HookReply::Deny { ref message } if message.contains("unauthorized")));
    let reply = r
        .ask("good-token", Agent::Claude, "PermissionRequest", bash("ls"))
        .await
        .unwrap()
        .unwrap();
    assert!(matches!(reply, HookReply::Deny { .. }));
    assert_eq!(r.hub.state("s1").unwrap().pending, 0);
}

#[tokio::test]
async fn session_start_proves_liveness() {
    let r = rig(Duration::from_secs(5)).await;
    assert!(!r.hub.state("s1").unwrap().live);
    let reply = r
        .ask("good-token", Agent::Codex, "SessionStart", json!({}))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(reply, HookReply::Ack);
    assert!(r.hub.state("s1").unwrap().live);
}

#[tokio::test]
async fn approval_reaches_hub_and_decision_returns_once() {
    let mut r = rig(Duration::from_secs(5)).await;
    let call = r.ask(
        "good-token",
        Agent::Codex,
        "PermissionRequest",
        bash("touch x"),
    );
    let Event::ApprovalRequested {
        approval_id,
        tool_name,
        tool_input,
        deadline_ms,
        ..
    } = r.next_event().await
    else {
        panic!("expected approval request");
    };
    assert_eq!((tool_name.as_str(), deadline_ms), ("Bash", 5000));
    assert_eq!(tool_input, json!({"command": "touch x"}));
    let state = r.hub.state("s1").unwrap();
    assert_eq!((state.pending, state.open_calls), (1, 1));

    r.hub.decide("s1", &approval_id, true).unwrap();
    assert_eq!(
        call.await.unwrap().unwrap(),
        HookReply::Allow {
            updated_input: None
        }
    );
    assert_eq!(
        r.hub.decide("s1", &approval_id, false),
        Err(DecideError::NotPending)
    );
    assert_eq!(
        r.hub.decide("nope", &approval_id, false),
        Err(DecideError::UnknownSession)
    );
    tokio::time::sleep(Duration::from_millis(50)).await;
    let state = r.hub.state("s1").unwrap();
    assert_eq!((state.pending, state.open_calls), (0, 0));
    assert!(state.last_answered.is_some());
}

#[tokio::test]
async fn deny_pattern_blocks_before_any_approval() {
    let mut r = rig(Duration::from_secs(5)).await;
    let reply = r
        .ask(
            "good-token",
            Agent::Codex,
            "PermissionRequest",
            bash("git push origin main"),
        )
        .await
        .unwrap()
        .unwrap();
    assert!(matches!(reply, HookReply::Deny { ref message } if message.contains("deny pattern")));
    let Event::Audit { kind, .. } = r.next_event().await else {
        panic!()
    };
    assert_eq!(kind, "deny_pattern");
    assert!(r.events.try_recv().is_err(), "no approval was created");
}

#[tokio::test]
async fn before_a_tool_only_deny_patterns_answer() {
    let mut r = rig(Duration::from_secs(5)).await;
    let before = |cmd: &str| json!({"hook_event_name": "PreToolUse", "tool_name": "Bash", "tool_input": {"command": cmd}});
    let reply = r
        .ask(
            "good-token",
            Agent::Codex,
            "PreToolUse",
            before("git push origin main"),
        )
        .await
        .unwrap()
        .unwrap();
    assert!(matches!(reply, HookReply::Deny { ref message } if message.contains("deny pattern")));
    let Event::Audit { kind, .. } = r.next_event().await else {
        panic!()
    };
    assert_eq!(kind, "deny_pattern");

    let reply = r
        .ask("good-token", Agent::Codex, "PreToolUse", before("ls"))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        reply,
        HookReply::Ack,
        "no decision: the agent's own mode decides"
    );
    assert!(r.events.try_recv().is_err(), "no approval was created");
    assert_eq!(r.hub.state("s1").unwrap().pending, 0);
}

#[tokio::test]
async fn unanswered_approval_expires_to_deny() {
    let mut r = rig(Duration::from_millis(300)).await;
    let call = r.ask(
        "good-token",
        Agent::Codex,
        "PermissionRequest",
        bash("touch x"),
    );
    let Event::ApprovalRequested { approval_id, .. } = r.next_event().await else {
        panic!()
    };
    let reply = call.await.unwrap().unwrap();
    assert!(matches!(reply, HookReply::Deny { ref message } if message.contains("expired")));
    let Event::ApprovalClosed { reason, .. } = r.next_event().await else {
        panic!()
    };
    assert_eq!(reason, "expired");
    assert_eq!(
        r.hub.decide("s1", &approval_id, true),
        Err(DecideError::NotPending)
    );
}

#[tokio::test]
async fn helper_that_goes_away_is_recorded_as_a_failure() {
    let mut r = rig(Duration::from_secs(5)).await;
    let mut stream = std::net::TcpStream::connect(r.addr).unwrap();
    let req = json!({"session_id": "s1", "token": "good-token", "agent": "codex",
                     "event": "PermissionRequest", "payload": bash("touch x")});
    writeln!(stream, "{req}").unwrap();
    let Event::ApprovalRequested { approval_id, .. } = r.next_event().await else {
        panic!()
    };
    drop(stream);
    let Event::ApprovalClosed { reason, .. } = r.next_event().await else {
        panic!()
    };
    assert_eq!(reason, "hook_helper_gone");
    let state = r.hub.state("s1").unwrap();
    assert_eq!((state.pending, state.open_calls), (0, 0));
    assert!(state.last_failure.is_some());
    assert_eq!(
        r.hub.decide("s1", &approval_id, true),
        Err(DecideError::NotPending)
    );
}

#[tokio::test]
async fn stopping_a_session_denies_waiting_helpers() {
    let mut r = rig(Duration::from_secs(5)).await;
    let call = r.ask(
        "good-token",
        Agent::Codex,
        "PermissionRequest",
        bash("touch x"),
    );
    let _ = r.next_event().await;
    r.hub.unregister("s1");
    let reply = call.await.unwrap().unwrap();
    assert!(
        matches!(reply, HookReply::Deny { ref message } if message.contains("session stopped"))
    );
}

#[test]
fn deny_patterns_match_name_command_or_input() {
    let p = vec![
        "rm -rf".to_string(),
        "WebFetch".to_string(),
        "/etc/passwd".to_string(),
    ];
    assert_eq!(
        matching_deny_pattern(&p, "Bash", &json!({"command": "rm -rf /"})).as_deref(),
        Some("rm -rf")
    );
    assert_eq!(
        matching_deny_pattern(&p, "WebFetch", &json!({})).as_deref(),
        Some("WebFetch")
    );
    assert_eq!(
        matching_deny_pattern(&p, "Read", &json!({"file_path": "/etc/passwd"})).as_deref(),
        Some("/etc/passwd")
    );
    assert_eq!(
        matching_deny_pattern(&p, "Bash", &json!({"command": "ls"})),
        None
    );
}
