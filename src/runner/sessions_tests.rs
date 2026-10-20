use std::sync::Mutex as StdMutex;

use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::UnixListener;

use super::*;
use crate::agents::codex::SUPPORTED_VERSIONS;

fn fixture(name: &str) -> String {
    std::fs::read_to_string(format!(
        "{}/tests/fixtures/codex-0.163.0/{name}.txt",
        env!("CARGO_MANIFEST_DIR")
    ))
    .unwrap()
}

/// A scripted Herdr: answers by method, plays `screens` for `agent.read`, records every call.
struct FakeHerdr {
    socket: PathBuf,
    calls: Arc<StdMutex<Vec<Value>>>,
}

fn agent_info(status: &str) -> Value {
    json!({"terminal_id": "t", "agent_status": status, "workspace_id": "w2", "tab_id": "w2:t1",
           "pane_id": "w2:p1", "focused": false, "revision": 1})
}

impl FakeHerdr {
    fn start(dir: &Path, start_result: Value, screens: Vec<String>) -> Self {
        Self::start_with(dir, start_result, screens, true)
    }

    fn start_with(
        dir: &Path,
        start_result: Value,
        screens: Vec<String>,
        launch_done: bool,
    ) -> Self {
        let socket = dir.join("herdr.sock");
        let listener = UnixListener::bind(&socket).unwrap();
        let calls = Arc::new(StdMutex::new(Vec::new()));
        let screens = Arc::new(StdMutex::new(std::collections::VecDeque::from(screens)));
        let log = calls.clone();
        tokio::spawn(async move {
            loop {
                let (stream, _) = listener.accept().await.unwrap();
                let (read, mut write) = stream.into_split();
                let mut line = String::new();
                BufReader::new(read).read_line(&mut line).await.unwrap();
                let req: Value = serde_json::from_str(&line).unwrap();
                log.lock().unwrap().push(req.clone());
                let result = match req["method"].as_str().unwrap() {
                    "worktree.create" => json!({"type": "worktree_created",
                        "workspace": {"workspace_id": "w2", "label": "sb-s1", "agent_status": "unknown"},
                        "tab": {"tab_id": "w2:t1", "workspace_id": "w2"},
                        "root_pane": {"pane_id": "w2:p1", "workspace_id": "w2", "tab_id": "w2:t1",
                                      "agent_status": "unknown", "revision": 0},
                        "worktree": {"path": "/wt/sb-s1", "branch": "sb/s1", "label": "repo",
                                     "is_linked_worktree": true}}),
                    "agent.start" => start_result.clone(),
                    "agent.read" => {
                        let mut q = screens.lock().unwrap();
                        let text = if q.len() > 1 {
                            q.pop_front().unwrap()
                        } else {
                            q.front().cloned().unwrap_or_default()
                        };
                        json!({"type": "pane_read", "read": {"pane_id": "w2:p1", "text": text, "revision": 1, "truncated": false}})
                    }
                    "agent.prompt" => {
                        json!({"type": "agent_prompted", "agent": agent_info("working")})
                    }
                    "session.snapshot" => {
                        let mut agent = agent_info("idle");
                        agent["interactive_ready"] = json!(launch_done);
                        agent["launch_pending"] = json!(!launch_done);
                        json!({"type": "session_snapshot", "snapshot": {"version": "0.9.1", "protocol": 22,
                               "workspaces": [], "tabs": [], "panes": [], "layouts": [], "agents": [agent]}})
                    }
                    _ => json!({"type": "ok"}),
                };
                let resp = if result.get("error").is_some() {
                    json!({"id": req["id"], "error": result["error"]})
                } else {
                    json!({"id": req["id"], "result": result})
                };
                write
                    .write_all(format!("{resp}\n").as_bytes())
                    .await
                    .unwrap();
            }
        });
        Self { socket, calls }
    }

    fn methods(&self) -> Vec<String> {
        self.calls
            .lock()
            .unwrap()
            .iter()
            .map(|c| c["method"].as_str().unwrap().to_owned())
            .collect()
    }

    fn call(&self, method: &str) -> Option<Value> {
        self.calls
            .lock()
            .unwrap()
            .iter()
            .find(|c| c["method"] == method)
            .cloned()
    }
}

struct Rig {
    dir: tempfile::TempDir,
    sessions: Arc<Sessions>,
    events: mpsc::Receiver<Event>,
}

fn rig_with(dir: tempfile::TempDir, socket: &Path, codex_version_output: &str) -> Rig {
    let (tx, events) = mpsc::channel(64);
    let hooks = HookHub::new(tx.clone(), Duration::from_secs(5));
    let cfg = SessionsConfig {
        state_dir: dir.path().join("state"),
        claude_config: dir.path().join("claude.json"),
        exe: PathBuf::from("/opt/sb/switchboard"),
        hook_addr: "127.0.0.1:9".parse().unwrap(),
        codex_version_cmd: vec!["echo".into(), codex_version_output.into()],
        liveness_timeout: Duration::from_secs(1),
        startup_timeout: Duration::from_secs(2),
    };
    let herdr = Client::new(socket).with_timeout(Duration::from_secs(5));
    Rig {
        sessions: Sessions::new(herdr, hooks, tx, cfg),
        dir,
        events,
    }
}

fn start_req(agent: Agent) -> StartRequest {
    StartRequest {
        session_id: "s1".into(),
        project_id: "p".into(),
        repo_path: "/srv/repo".into(),
        branch: "sb/s1".into(),
        agent,
        prompt: "do the task".into(),
        deny_patterns: vec![],
    }
}

fn audit_kinds(events: &mut mpsc::Receiver<Event>) -> Vec<String> {
    let mut kinds = Vec::new();
    while let Ok(ev) = events.try_recv() {
        if let Event::Audit { kind, .. } = ev {
            kinds.push(kind);
        }
    }
    kinds
}

#[test]
fn ids_and_names() {
    assert!(check_session_id("a1b2-c3").is_ok());
    for bad in ["", "../x", "S1", "a b", &"x".repeat(41)] {
        assert!(check_session_id(bad).is_err(), "{bad:?}");
    }
    assert_eq!(
        agent_name("0123456789abcdef0123456789abcdef"),
        "sb-0123456789abcdef0123456789abc"
    );
    crate::herdr::check_agent_name(&agent_name("0123456789abcdef0123456789abcdef")).unwrap();
}

#[tokio::test]
async fn codex_start_clears_startup_screen_before_sending_the_prompt() {
    let dir = tempfile::tempdir().unwrap();
    let herdr = FakeHerdr::start(
        dir.path(),
        json!({"type": "agent_started", "agent": agent_info("idle"), "argv": []}),
        vec![fixture("folder-access"), fixture("ready")],
    );
    let mut r = rig_with(dir, &herdr.socket, "codex-cli 0.163.0");
    let out = r.sessions.start(start_req(Agent::Codex)).await.unwrap();
    assert_eq!(out["codex_version"], SUPPORTED_VERSIONS[0]);
    assert_eq!(
        herdr.methods(),
        [
            "worktree.create",
            "agent.start",
            "agent.read",
            "agent.send_keys",
            "agent.read",
            "agent.read",
            "session.snapshot",
            "agent.prompt"
        ]
    );
    assert_eq!(
        herdr.call("agent.send_keys").unwrap()["params"]["keys"],
        json!(["enter"])
    );
    let args = herdr.call("agent.start").unwrap()["params"]["args"].clone();
    assert!(
        args.as_array()
            .unwrap()
            .contains(&json!("--dangerously-bypass-hook-trust"))
    );
    assert!(
        args[5]
            .as_str()
            .unwrap()
            .contains(r#"{"/srv/repo"={trust_level="untrusted"}}"#)
    );
    assert!(args[8].as_str().unwrap().contains("--secret-file"));
    assert!(audit_kinds(&mut r.events).contains(&"startup_cleared".to_string()));
    let secret = r.dir.path().join("state/sessions/s1/secret.json");
    assert_eq!(SecretFile::read(&secret).unwrap().session_id, "s1");
}

#[tokio::test]
async fn codex_unrecognised_startup_never_prompts_and_closes_workspace() {
    let dir = tempfile::tempdir().unwrap();
    let herdr = FakeHerdr::start(
        dir.path(),
        json!({"type": "agent_started", "agent": agent_info("idle"), "argv": []}),
        vec!["  Some new screen we have not seen\n  press any key\n".into()],
    );
    let mut r = rig_with(dir, &herdr.socket, "codex-cli 0.163.0");
    let e = r.sessions.start(start_req(Agent::Codex)).await.unwrap_err();
    assert_eq!(e.code, ErrorCode::StartupFailed);
    let methods = herdr.methods();
    assert!(
        !methods.contains(&"agent.prompt".to_string()),
        "{methods:?}"
    );
    assert!(
        !methods.contains(&"agent.send_keys".to_string()),
        "{methods:?}"
    );
    assert_eq!(methods.last().unwrap(), "workspace.close");
    let kinds = audit_kinds(&mut r.events);
    assert!(
        kinds.contains(&"startup_failed".to_string())
            && kinds.contains(&"start_failed".to_string())
    );
    assert!(r.sessions.get("s1").is_none());
    assert!(
        !r.dir.path().join("state/sessions/s1").exists(),
        "secret removed"
    );
}

#[tokio::test]
async fn codex_launch_herdr_never_finishes_fails_without_prompting() {
    let dir = tempfile::tempdir().unwrap();
    let herdr = FakeHerdr::start_with(
        dir.path(),
        json!({"error": {"code": "agent_not_ready", "message": "blocked during startup"}}),
        vec![fixture("folder-access"), fixture("ready")],
        false,
    );
    let r = rig_with(dir, &herdr.socket, "codex-cli 0.163.0");
    let e = r.sessions.start(start_req(Agent::Codex)).await.unwrap_err();
    assert_eq!(e.code, ErrorCode::StartupFailed);
    assert!(!herdr.methods().contains(&"agent.prompt".to_string()));
    assert_eq!(herdr.methods().last().unwrap(), "workspace.close");
}

#[tokio::test]
async fn unknown_codex_version_is_refused_before_anything_is_created() {
    let dir = tempfile::tempdir().unwrap();
    let herdr = FakeHerdr::start(dir.path(), json!({"type": "ok"}), vec![]);
    for output in ["codex-cli 0.164.0", "something else"] {
        let r = rig_with(tempfile::tempdir().unwrap(), &herdr.socket, output);
        let e = r.sessions.start(start_req(Agent::Codex)).await.unwrap_err();
        assert_eq!(e.code, ErrorCode::UnsupportedAgentVersion, "{output}");
        assert!(!r.dir.path().join("state").exists());
    }
    assert!(herdr.methods().is_empty(), "Herdr was never called");
}

#[tokio::test]
async fn claude_without_live_hook_fails_hooks_untrusted_and_never_prompts() {
    let dir = tempfile::tempdir().unwrap();
    let herdr = FakeHerdr::start(
        dir.path(),
        json!({"type": "agent_started", "agent": agent_info("idle"), "argv": []}),
        vec![],
    );
    let r = rig_with(dir, &herdr.socket, "unused");
    let e = r
        .sessions
        .start(start_req(Agent::Claude))
        .await
        .unwrap_err();
    assert_eq!(e.code, ErrorCode::HooksUntrusted);
    assert!(!herdr.methods().contains(&"agent.prompt".to_string()));
    assert_eq!(herdr.methods().last().unwrap(), "workspace.close");
    assert!(!herdr.methods().contains(&"agent.send_keys".to_string()));
    // The repo root was pre-trusted and the settings named the fail-closed hook.
    let cfg: Value =
        serde_json::from_str(&std::fs::read_to_string(r.dir.path().join("claude.json")).unwrap())
            .unwrap();
    assert_eq!(cfg["projects"]["/srv/repo"]["hasTrustDialogAccepted"], true);
    let args = herdr.call("agent.start").unwrap()["params"]["args"].clone();
    assert_eq!(
        args.as_array().unwrap()[2..],
        [json!("--permission-mode"), json!("acceptEdits")]
    );
}

#[tokio::test]
async fn claude_with_live_hook_gets_the_prompt() {
    let dir = tempfile::tempdir().unwrap();
    let herdr = FakeHerdr::start(
        dir.path(),
        json!({"type": "agent_started", "agent": agent_info("idle"), "argv": []}),
        vec![],
    );
    let r = rig_with(dir, &herdr.socket, "unused");
    let sessions = r.sessions.clone();
    // Liveness comes first, then Herdr's readiness, then the prompt.
    let start = tokio::spawn(async move { sessions.start(start_req(Agent::Claude)).await });
    // Stand in for the SessionStart hook reaching the runner.
    while r.sessions.get("s1").and_then(|s| s.pane_id).is_none() {
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    tokio::time::sleep(Duration::from_millis(500)).await;
    r.sessions.hooks.test_mark_live("s1");
    start.await.unwrap().unwrap();
    assert_eq!(herdr.methods().last().unwrap(), "agent.prompt");
    assert_eq!(
        herdr.call("agent.prompt").unwrap()["params"]["text"],
        "do the task"
    );
}

#[tokio::test]
async fn start_is_idempotent_by_session_id() {
    let dir = tempfile::tempdir().unwrap();
    let herdr = FakeHerdr::start(
        dir.path(),
        json!({"type": "agent_started", "agent": agent_info("idle"), "argv": []}),
        vec![fixture("folder-access"), fixture("ready")],
    );
    let r = rig_with(dir, &herdr.socket, "codex-cli 0.163.0");
    let first = r.sessions.start(start_req(Agent::Codex)).await.unwrap();
    let n = herdr.methods().len();
    let again = r.sessions.start(start_req(Agent::Codex)).await.unwrap();
    assert_eq!(first, again);
    assert_eq!(herdr.methods().len(), n, "no second worktree or agent");
}

#[tokio::test]
async fn sessions_reload_after_restart_with_new_hook_address() {
    let dir = tempfile::tempdir().unwrap();
    let herdr = FakeHerdr::start(
        dir.path(),
        json!({"type": "agent_started", "agent": agent_info("idle"), "argv": []}),
        vec![fixture("folder-access"), fixture("ready")],
    );
    let state = dir.path().join("state");
    let r = rig_with(dir, &herdr.socket, "codex-cli 0.163.0");
    r.sessions.start(start_req(Agent::Codex)).await.unwrap();

    let (tx, _rx) = mpsc::channel(8);
    let hooks = HookHub::new(tx.clone(), Duration::from_secs(5));
    let cfg = SessionsConfig {
        state_dir: state.clone(),
        claude_config: r.dir.path().join("claude.json"),
        exe: PathBuf::from("/opt/sb/switchboard"),
        hook_addr: "127.0.0.1:4242".parse().unwrap(),
        codex_version_cmd: vec![],
        liveness_timeout: Duration::from_secs(1),
        startup_timeout: Duration::from_secs(2),
    };
    let reloaded = Sessions::new(Client::new(&herdr.socket), hooks.clone(), tx, cfg);
    assert_eq!(reloaded.load().unwrap(), 1);
    assert_eq!(
        reloaded.get("s1").unwrap().pane_id.as_deref(),
        Some("w2:p1")
    );
    assert!(hooks.state("s1").is_some());
    let secret = SecretFile::read(&state.join("sessions/s1/secret.json")).unwrap();
    assert_eq!(secret.addr.port(), 4242);
}

#[tokio::test]
async fn clean_and_merged_check() {
    let dir = tempfile::tempdir().unwrap();
    let repo = dir.path().join("repo");
    let run = |args: &[&str], cwd: &Path| {
        let ok = std::process::Command::new("git")
            .arg("-C")
            .arg(cwd)
            .args(args)
            .status()
            .unwrap()
            .success();
        assert!(ok, "git {args:?}");
    };
    std::fs::create_dir(&repo).unwrap();
    run(&["init", "-q", "-b", "main"], &repo);
    run(
        &[
            "-c",
            "user.name=t",
            "-c",
            "user.email=t@t",
            "commit",
            "-q",
            "--allow-empty",
            "-m",
            "init",
        ],
        &repo,
    );
    let wt = dir.path().join("wt");
    run(
        &["worktree", "add", "-q", "-b", "sb/s1", wt.to_str().unwrap()],
        &repo,
    );
    assert_eq!(worktree_clean_and_merged(&wt, &repo, "sb/s1").await, Ok(()));
    let head = std::process::Command::new("git")
        .args(["-C", repo.to_str().unwrap(), "rev-parse", "HEAD"])
        .output()
        .unwrap();
    let head = String::from_utf8(head.stdout).unwrap().trim().to_owned();
    assert_eq!(head_commit(&wt).await.as_deref(), Some(head.as_str()));
    assert_eq!(head_commit(dir.path()).await, None);

    std::fs::write(wt.join("f.txt"), "x").unwrap();
    assert_eq!(
        worktree_clean_and_merged(&wt, &repo, "sb/s1").await,
        Err("dirty".into())
    );
    run(&["add", "f.txt"], &wt);
    run(
        &[
            "-c",
            "user.name=t",
            "-c",
            "user.email=t@t",
            "commit",
            "-q",
            "-m",
            "work",
        ],
        &wt,
    );
    assert_eq!(
        worktree_clean_and_merged(&wt, &repo, "sb/s1").await,
        Err("unmerged".into())
    );
}
