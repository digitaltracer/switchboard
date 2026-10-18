use std::sync::{Arc, Mutex};

use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::UnixListener;

use super::*;

fn agent(name: Option<&str>, pane: &str, status: &str) -> Value {
    json!({"terminal_id": "t", "agent_status": status, "workspace_id": "w1", "tab_id": "w1:t1",
           "pane_id": pane, "focused": false, "revision": 0, "name": name})
}

fn snapshot(agents: &[Value]) -> Value {
    json!({"type": "session_snapshot", "snapshot": {"version": "0.9.1", "protocol": 22,
           "workspaces": [], "tabs": [], "panes": [], "layouts": [], "agents": agents}})
}

/// A Herdr whose agent list the test changes while the watcher runs. Subscriptions stay open.
fn fake_herdr(dir: &std::path::Path, agents: Arc<Mutex<Vec<Value>>>) -> std::path::PathBuf {
    let socket = dir.join("herdr.sock");
    let listener = UnixListener::bind(&socket).unwrap();
    tokio::spawn(async move {
        loop {
            let (stream, _) = listener.accept().await.unwrap();
            let agents = agents.clone();
            tokio::spawn(async move {
                let (read, mut write) = stream.into_split();
                let mut line = String::new();
                BufReader::new(read).read_line(&mut line).await.unwrap();
                let req: Value = serde_json::from_str(&line).unwrap();
                let result = match req["method"].as_str().unwrap() {
                    "events.subscribe" => json!({"type": "subscription_started"}),
                    "session.snapshot" => snapshot(&agents.lock().unwrap()),
                    m => panic!("unexpected {m}"),
                };
                let resp = json!({"id": req["id"], "result": result});
                write
                    .write_all(format!("{resp}\n").as_bytes())
                    .await
                    .unwrap();
                if req["method"] == "events.subscribe" {
                    std::future::pending::<()>().await;
                }
            });
        }
    });
    socket
}

#[test]
fn every_named_agent_is_followed() {
    let snap: SessionSnapshot = serde_json::from_value(
        snapshot(&[
            agent(Some("sb-3f9a"), "w1:p1", "blocked"),
            agent(None, "w1:p2", "idle"),
            agent(Some("switchboard-impl"), "w1:p3", "working"),
        ])["snapshot"]
            .clone(),
    )
    .unwrap();
    assert_eq!(
        named_panes(&snap),
        BTreeMap::from([
            ("w1:p1".to_string(), "sb-3f9a".to_string()),
            ("w1:p3".to_string(), "switchboard-impl".to_string()),
        ])
    );
    assert_eq!(
        statuses(&snap),
        [
            ("sb-3f9a".to_string(), AgentStatus::Blocked),
            ("switchboard-impl".to_string(), AgentStatus::Working),
        ]
    );
}

#[tokio::test]
async fn a_new_switchboard_agent_is_picked_up_without_restart() {
    let dir = tempfile::tempdir().unwrap();
    let agents = Arc::new(Mutex::new(vec![agent(Some("reviewer"), "w1:p1", "idle")]));
    let socket = fake_herdr(dir.path(), agents.clone());
    let (tx, mut rx) = mpsc::channel(64);
    tokio::spawn(watch_every(
        Client::new(&socket),
        tx,
        Duration::from_millis(100),
    ));

    let first = tokio::time::timeout(Duration::from_secs(5), rx.recv())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(first, ("reviewer".to_string(), AgentStatus::Idle));

    // A session starts after the watcher is already running.
    agents
        .lock()
        .unwrap()
        .push(agent(Some("sb-7c21"), "w2:p1", "blocked"));
    let seen = tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let (name, status) = rx.recv().await.unwrap();
            if name == "sb-7c21" {
                return status;
            }
        }
    })
    .await
    .expect("new agent reported");
    assert_eq!(seen, AgentStatus::Blocked);
}
