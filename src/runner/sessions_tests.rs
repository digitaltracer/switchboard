use std::sync::Mutex as StdMutex;

use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::UnixListener;

use super::*;

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
    /// Answers that replace the scripted ones, by method.
    answers: Arc<StdMutex<HashMap<String, Value>>>,
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
        let answers: Arc<StdMutex<HashMap<String, Value>>> = Arc::default();
        let screens = Arc::new(StdMutex::new(std::collections::VecDeque::from(screens)));
        let (log, set) = (calls.clone(), answers.clone());
        tokio::spawn(async move {
            loop {
                let (stream, _) = listener.accept().await.unwrap();
                let (read, mut write) = stream.into_split();
                let mut line = String::new();
                BufReader::new(read).read_line(&mut line).await.unwrap();
                let req: Value = serde_json::from_str(&line).unwrap();
                log.lock().unwrap().push(req.clone());
                let answer = set
                    .lock()
                    .unwrap()
                    .get(req["method"].as_str().unwrap())
                    .cloned();
                let result = answer.unwrap_or_else(|| match req["method"].as_str().unwrap() {
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
                    "ping" => json!({"type": "pong", "version": "0.9.1", "protocol": 22}),
                    _ => json!({"type": "ok"}),
                });
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
        Self {
            socket,
            calls,
            answers,
        }
    }

    fn answer(&self, method: &str, result: Value) {
        self.answers
            .lock()
            .unwrap()
            .insert(method.to_owned(), result);
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
        claude_mode: Default::default(),
        claude_projects: std::path::PathBuf::from("/nonexistent/claude/projects"),
        codex_sessions: std::path::PathBuf::from("/nonexistent/codex/sessions"),
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
        backend: None,
        space: None,
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
    assert_eq!(out["codex_version"], "0.163.0");
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
async fn unreadable_codex_version_is_refused_before_anything_is_created() {
    let dir = tempfile::tempdir().unwrap();
    let herdr = FakeHerdr::start(dir.path(), json!({"type": "ok"}), vec![]);
    let r = rig_with(
        tempfile::tempdir().unwrap(),
        &herdr.socket,
        "something else",
    );
    let e = r.sessions.start(start_req(Agent::Codex)).await.unwrap_err();
    assert_eq!(e.code, ErrorCode::UnsupportedAgentVersion);
    assert!(!r.dir.path().join("state").exists());
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
        [json!("--permission-mode"), json!("auto")],
        "auto unless the machine's config says otherwise"
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
        claude_mode: Default::default(),
        claude_projects: std::path::PathBuf::from("/nonexistent/claude/projects"),
        codex_sessions: std::path::PathBuf::from("/nonexistent/codex/sessions"),
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

#[tokio::test]
async fn a_stopped_tasks_worktree_goes_only_if_registered_clean_and_merged() {
    let dir = tempfile::tempdir().unwrap();
    let repo = dir.path().join("repo");
    let git = |args: &[&str], cwd: &Path| {
        let ok = std::process::Command::new("git")
            .args(["-c", "user.name=t", "-c", "user.email=t@t", "-C"])
            .arg(cwd)
            .args(args)
            .status()
            .unwrap()
            .success();
        assert!(ok, "git {args:?}");
    };
    std::fs::create_dir(&repo).unwrap();
    git(&["init", "-q", "-b", "main"], &repo);
    git(&["commit", "-q", "--allow-empty", "-m", "init"], &repo);
    let wt = dir.path().join("wt");
    git(
        &["worktree", "add", "-q", "-b", "sb/s1", wt.to_str().unwrap()],
        &repo,
    );
    let stray = dir.path().join("stray");
    std::fs::create_dir(&stray).unwrap();

    let (removed, why) = remove_stopped_worktree(&repo, &stray, "sb/s1").await;
    assert!(!removed && stray.exists(), "not a worktree: {why:?}");
    let (removed, why) = remove_stopped_worktree(&repo, &repo, "main").await;
    assert!(!removed && repo.exists(), "the main checkout: {why:?}");

    std::fs::write(wt.join("f.txt"), "x").unwrap();
    assert_eq!(
        remove_stopped_worktree(&repo, &wt, "sb/s1").await,
        (false, Some("dirty".into()))
    );
    git(&["add", "f.txt"], &wt);
    git(&["commit", "-q", "-m", "work"], &wt);
    assert_eq!(
        remove_stopped_worktree(&repo, &wt, "sb/s1").await,
        (false, Some("unmerged".into()))
    );
    git(&["merge", "-q", "sb/s1"], &repo);
    assert_eq!(
        remove_stopped_worktree(&repo, &wt, "sb/s1").await,
        (true, None)
    );
    assert!(!wt.exists());
    let branches = std::process::Command::new("git")
        .args(["-C", repo.to_str().unwrap(), "branch", "--list", "sb/s1"])
        .output()
        .unwrap();
    assert!(!branches.stdout.is_empty(), "the branch stays");
    assert_eq!(
        remove_stopped_worktree(&repo, &wt, "sb/s1").await,
        (false, None),
        "already gone"
    );
}

// Each task in a Herdr session of its own.

use super::super::herdrs::{Herdrs, Starter};

/// The Herdr sessions a multi-mode runner started, by name.
type Started = Arc<StdMutex<Vec<(String, FakeHerdr)>>>;

/// A runner in multi mode whose new Herdr sessions are scripted Herdrs; returns the sessions it started.
fn multi_rig(dir: tempfile::TempDir, screens: Vec<String>) -> (Rig, PathBuf, Started) {
    let config_dir = dir.path().join("herdr");
    let started: Started = Arc::default();
    let log = started.clone();
    let starter = Starter::Func(Arc::new(move |name: String, socket: PathBuf| {
        let (log, screens) = (log.clone(), screens.clone());
        Box::pin(async move {
            let folder = socket.parent().unwrap().to_owned();
            std::fs::create_dir_all(&folder).unwrap();
            let fake = FakeHerdr::start(
                &folder,
                json!({"type": "agent_started", "agent": agent_info("idle"), "argv": []}),
                screens,
            );
            log.lock().unwrap().push((name, fake));
            Ok(())
        })
    }));
    // The legacy server doesn't exist: nothing may go there.
    let herdrs = Herdrs::multi(config_dir.clone(), dir.path().join("nowhere.sock"), starter);
    let (tx, events) = mpsc::channel(64);
    let hooks = HookHub::new(tx.clone(), Duration::from_secs(5));
    let cfg = SessionsConfig {
        state_dir: dir.path().join("state"),
        claude_config: dir.path().join("claude.json"),
        claude_mode: Default::default(),
        claude_projects: dir.path().join("claude-projects"),
        codex_sessions: std::path::PathBuf::from("/nonexistent/codex/sessions"),
        exe: PathBuf::from("/opt/sb/switchboard"),
        hook_addr: "127.0.0.1:9".parse().unwrap(),
        codex_version_cmd: vec!["echo".into(), "codex-cli 0.163.0".into()],
        liveness_timeout: Duration::from_secs(1),
        startup_timeout: Duration::from_secs(2),
    };
    let rig = Rig {
        sessions: Sessions::with_herdrs(herdrs, hooks, tx, cfg),
        dir,
        events,
    };
    (rig, config_dir, started)
}

#[tokio::test]
async fn every_new_task_starts_in_a_new_herdr_session_of_its_own() {
    let dir = tempfile::tempdir().unwrap();
    let screens = vec![fixture("folder-access"), fixture("ready")];
    let (r, config_dir, started) = multi_rig(dir, screens);
    let first = r.sessions.start(start_req(Agent::Codex)).await.unwrap();
    assert_eq!(first["herdr_session"], "p-s1");
    let mut second_req = start_req(Agent::Codex);
    second_req.session_id = "s2".into();
    second_req.branch = "sb/s2".into();
    let second = r.sessions.start(second_req).await.unwrap();
    assert_eq!(second["herdr_session"], "p-s2");

    let started = started.lock().unwrap();
    let names: Vec<&str> = started.iter().map(|(n, _)| n.as_str()).collect();
    assert_eq!(names, ["p-s1", "p-s2"], "one new Herdr session per task");
    for (name, fake) in started.iter() {
        assert_eq!(
            fake.socket,
            config_dir.join("sessions").join(name).join("herdr.sock")
        );
        let m = fake.methods();
        assert!(
            m.contains(&"worktree.create".to_owned()) && m.contains(&"agent.prompt".to_owned()),
            "{name}: {m:?}"
        );
    }
    assert_eq!(
        r.sessions.get("s1").unwrap().herdr_session.as_deref(),
        Some("p-s1")
    );
    let saved: RunnerSession = serde_json::from_slice(
        &std::fs::read(r.dir.path().join("state/sessions/s2/meta.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(
        saved.herdr_session.as_deref(),
        Some("p-s2"),
        "it survives a runner restart"
    );
}

#[tokio::test]
async fn delete_stops_a_running_task_then_deletes_its_own_herdr_session() {
    let dir = tempfile::tempdir().unwrap();
    let screens = vec![fixture("folder-access"), fixture("ready")];
    let (r, config_dir, started) = multi_rig(dir, screens);
    r.sessions.start(start_req(Agent::Codex)).await.unwrap();
    let folder = config_dir.join("sessions/p-s1");
    assert!(folder.exists());
    let delete = |herdr_session: Option<&str>| Op::Delete {
        session_id: "s1".into(),
        herdr_session: herdr_session.map(str::to_owned),
        repo_path: Some("/srv/repo".into()),
        worktree_path: None,
        branch: Some("sb/s1".into()),
    };

    let v = r.sessions.handle(delete(None)).await.unwrap();
    assert_eq!(v["herdr_session_deleted"], true);
    assert_eq!(
        v["worktree_removed"], false,
        "a worktree it can't check is kept"
    );
    assert!(v["kept_because"].is_string());
    assert!(!folder.exists(), "its Herdr session is gone");
    assert!(
        r.sessions.get("s1").is_none(),
        "and the runner forgot the task"
    );
    let methods = started.lock().unwrap()[0].1.methods();
    assert!(
        !methods.contains(&"worktree.remove".to_owned()),
        "{methods:?}"
    );

    // A retry after a lost answer: the task is gone here, so the hub's record says what to delete.
    let again = r.sessions.handle(delete(Some("p-s1"))).await.unwrap();
    assert_eq!(again["herdr_session_deleted"], true);
}

/// A Herdr session with one named agent, as a user's own session looks.
const CLAUDE_SESSION: &str = "77748a76-125f-4e27-9534-dfbb00b872e9";

fn named_herdr(folder: &Path, agent: &'static str, screen: &'static str) {
    std::fs::create_dir_all(folder).unwrap();
    let listener = UnixListener::bind(folder.join("herdr.sock")).unwrap();
    tokio::spawn(async move {
        loop {
            let (stream, _) = listener.accept().await.unwrap();
            let (read, mut write) = stream.into_split();
            let mut line = String::new();
            BufReader::new(read).read_line(&mut line).await.unwrap();
            let req: Value = serde_json::from_str(&line).unwrap();
            let result = match req["method"].as_str().unwrap() {
                "ping" => json!({"type": "pong", "version": "0.9.1", "protocol": 22}),
                "session.snapshot" => {
                    let mut a = agent_info("idle");
                    a["name"] = json!(agent);
                    a["agent"] = json!("claude");
                    a["agent_session"] =
                        json!({"agent": "claude", "kind": "id", "value": CLAUDE_SESSION});
                    json!({"type": "session_snapshot", "snapshot": {"version": "0.9.1", "protocol": 22,
                           "workspaces": [], "tabs": [], "panes": [], "layouts": [], "agents": [a]}})
                }
                "agent.read" => {
                    json!({"type": "pane_read", "read": {"pane_id": "w2:p1", "text": screen, "revision": 1, "truncated": false}})
                }
                _ => json!({"type": "ok"}),
            };
            let resp = json!({"id": req["id"], "result": result});
            write
                .write_all(format!("{resp}\n").as_bytes())
                .await
                .unwrap();
        }
    });
}

#[tokio::test]
async fn agents_in_every_running_herdr_session_are_listed_and_reached_by_session() {
    let dir = tempfile::tempdir().unwrap();
    let (r, config_dir, _) = multi_rig(dir, vec![]);
    named_herdr(&config_dir.join("sessions/mono1"), "claude", "mono1 screen");
    named_herdr(&config_dir.join("sessions/pca-udp"), "claude", "pca screen");
    // A stopped session: a folder with nothing answering.
    std::fs::create_dir_all(config_dir.join("sessions/old-one")).unwrap();

    let list: Vec<crate::proto::AgentSummary> =
        serde_json::from_value(r.sessions.handle(Op::Agents).await.unwrap()).unwrap();
    let names: Vec<(&str, Option<&str>)> = list
        .iter()
        .map(|a| (a.name.as_str(), a.herdr_session.as_deref()))
        .collect();
    assert_eq!(
        names,
        [
            ("mono1/claude", Some("mono1")),
            ("pca-udp/claude", Some("pca-udp"))
        ]
    );

    let read = r
        .sessions
        .handle(Op::ReadAgent {
            agent: "pca-udp/claude".into(),
            lines: 0,
            visible: true,
            ansi: false,
        })
        .await
        .unwrap();
    assert_eq!(read["text"], "pca screen");
    let read = r
        .sessions
        .handle(Op::ReadAgent {
            agent: "mono1/claude".into(),
            lines: 0,
            visible: true,
            ansi: false,
        })
        .await
        .unwrap();
    assert_eq!(read["text"], "mono1 screen");
}

fn codex_herdr(dir: &Path) -> FakeHerdr {
    FakeHerdr::start(
        dir,
        json!({"type": "agent_started", "agent": agent_info("idle"), "argv": []}),
        vec![fixture("folder-access"), fixture("ready")],
    )
}

#[tokio::test]
async fn a_first_prompt_that_fails_undoes_the_start() {
    let dir = tempfile::tempdir().unwrap();
    let herdr = codex_herdr(dir.path());
    herdr.answer(
        "agent.prompt",
        json!({"error": {"code": "pane_gone", "message": "the pane went away"}}),
    );
    let mut r = rig_with(dir, &herdr.socket, "codex-cli 0.163.0");
    let e = r.sessions.start(start_req(Agent::Codex)).await.unwrap_err();
    assert_eq!(e.code, ErrorCode::Herdr);
    assert!(
        r.sessions.get("s1").is_none(),
        "the hub fails it, so the runner doesn't keep it"
    );
    assert!(herdr.methods().contains(&"workspace.close".to_owned()));
    assert!(!r.dir.path().join("state/sessions/s1").exists());
    assert!(audit_kinds(&mut r.events).contains(&"start_failed".to_owned()));
}

#[tokio::test]
async fn a_start_cut_short_by_a_runner_restart_is_undone() {
    let dir = tempfile::tempdir().unwrap();
    let herdr = codex_herdr(dir.path());
    let r = rig_with(dir, &herdr.socket, "codex-cli 0.163.0");
    r.sessions.start(start_req(Agent::Codex)).await.unwrap();
    assert!(r.sessions.get("s1").unwrap().launched);
    assert_eq!(r.sessions.undo_interrupted_starts().await, 0);

    // As a runner that died between creating the worktree and launching the agent left it.
    let meta = r.dir.path().join("state/sessions/s1/meta.json");
    let mut saved: Value = serde_json::from_slice(&std::fs::read(&meta).unwrap()).unwrap();
    saved["launched"] = json!(false);
    std::fs::write(&meta, saved.to_string()).unwrap();
    r.sessions.load().unwrap();
    assert_eq!(r.sessions.undo_interrupted_starts().await, 1);
    assert!(r.sessions.get("s1").is_none());
    assert!(herdr.methods().contains(&"workspace.close".to_owned()));

    // Tasks saved before the field existed were complete starts.
    saved.as_object_mut().unwrap().remove("launched");
    let old: RunnerSession = serde_json::from_value(saved).unwrap();
    assert!(old.launched);
}

#[tokio::test]
async fn a_task_whose_herdr_server_is_gone_is_reported_exited() {
    let dir = tempfile::tempdir().unwrap();
    let (mut r, config_dir, _) = multi_rig(dir, vec![fixture("folder-access"), fixture("ready")]);
    r.sessions.start(start_req(Agent::Codex)).await.unwrap();
    while r.events.try_recv().is_ok() {}
    // A reboot or `herdr server stop`: nothing listens any more.
    std::fs::remove_file(config_dir.join("sessions/p-s1/herdr.sock")).unwrap();

    tokio::time::pause();
    tokio::spawn(super::super::watch::watch(r.sessions.clone()));
    let exited = tokio::time::timeout(Duration::from_secs(300), async {
        loop {
            if let Some(Event::SessionExited { session_id }) = r.events.recv().await {
                return session_id;
            }
        }
    })
    .await
    .expect("reported once the server has been gone a while");
    assert_eq!(exited, "s1");
}

#[tokio::test]
async fn changes_lists_edits_since_the_start_commit_and_new_files() {
    let dir = tempfile::tempdir().unwrap();
    let wt = dir.path();
    let run = |args: &[&str]| {
        let out = std::process::Command::new("git")
            .arg("-C")
            .arg(wt)
            .args([
                "-c",
                "user.name=t",
                "-c",
                "user.email=t@t",
                "-c",
                "commit.gpgsign=false",
            ])
            .args(args)
            .output()
            .unwrap();
        assert!(out.status.success(), "{args:?}: {out:?}");
    };
    run(&["init", "-q"]);
    std::fs::write(wt.join("a.rs"), "one\ntwo\n").unwrap();
    run(&["add", "a.rs"]);
    run(&["commit", "-qm", "start"]);
    let base = head_commit(wt).await.unwrap();
    std::fs::write(wt.join("a.rs"), "one\n2\nthree\n").unwrap();
    run(&["commit", "-qam", "agent work"]);
    std::fs::write(wt.join("a.rs"), "one\n2\nthree\nfour\n").unwrap();
    std::fs::write(wt.join("new.rs"), "x\n").unwrap();

    let v = changes(wt, &base).await.unwrap();
    assert_eq!(
        v,
        json!({"files": [
            {"path": "a.rs", "added": 3, "deleted": 1},
            {"path": "new.rs", "added": null, "deleted": null},
        ], "truncated": false})
    );
    assert!(
        changes(wt, "0000000000000000000000000000000000000000")
            .await
            .is_err()
    );
}

#[tokio::test]
async fn an_agents_conversation_is_read_from_its_claude_log() {
    let dir = tempfile::tempdir().unwrap();
    let (r, config_dir, _) = multi_rig(dir, vec![]);
    named_herdr(&config_dir.join("sessions/mono1"), "claude", "mono1 screen");
    let ask = |after| Op::Transcript {
        agent: "mono1/claude".into(),
        after,
        before: None,
        wait_ms: None,
    };
    let e = r.sessions.handle(ask(None)).await.unwrap_err();
    assert_eq!(e.code, ErrorCode::NotFound, "no log yet: {e:?}");

    let folder = r.dir.path().join("claude-projects/-Users-me-mono1");
    std::fs::create_dir_all(&folder).unwrap();
    let line = json!({"type": "user", "uuid": "u1", "message": {"content": "fix the flaky test"}});
    std::fs::write(
        folder.join(format!("{CLAUDE_SESSION}.jsonl")),
        format!("{line}\n"),
    )
    .unwrap();
    let page: crate::proto::TranscriptPage =
        serde_json::from_value(r.sessions.handle(ask(None)).await.unwrap()).unwrap();
    assert_eq!(page.items[0].text.as_deref(), Some("fix the flaky test"));
    let next: crate::proto::TranscriptPage =
        serde_json::from_value(r.sessions.handle(ask(Some(page.end))).await.unwrap()).unwrap();
    assert!(next.items.is_empty() && next.start == page.end);
}

#[tokio::test]
async fn activity_gives_each_agent_its_last_line() {
    let dir = tempfile::tempdir().unwrap();
    let (r, config_dir, _) = multi_rig(dir, vec![]);
    named_herdr(&config_dir.join("sessions/mono1"), "claude", "mono1 screen");
    named_herdr(&config_dir.join("sessions/pca-udp"), "claude", "pca screen");
    let activity =
        |v: Value| serde_json::from_value::<Vec<crate::proto::AgentActivity>>(v).unwrap();
    let none = activity(r.sessions.handle(Op::Activity).await.unwrap());
    assert_eq!(none.len(), 2);
    assert!(
        none.iter().all(|a| a.line.is_none()),
        "no logs yet: {none:?}"
    );

    let folder = r.dir.path().join("claude-projects/-Users-me-mono1");
    std::fs::create_dir_all(&folder).unwrap();
    let said = json!({"type": "assistant", "uuid": "a1", "timestamp": "2026-11-07T09:00:00Z",
                      "message": {"content": [{"type": "text", "text": "Tests pass.\nAll of them."}]}});
    std::fs::write(
        folder.join(format!("{CLAUDE_SESSION}.jsonl")),
        format!("{said}\n"),
    )
    .unwrap();
    let list = activity(r.sessions.handle(Op::Activity).await.unwrap());
    let names: Vec<&str> = list.iter().map(|a| a.name.as_str()).collect();
    assert_eq!(names, ["mono1/claude", "pca-udp/claude"]);
    // Both fake servers report the same session, so both find the log.
    assert_eq!(list[0].line.as_deref(), Some("Tests pass."));
    assert_eq!(list[0].at.as_deref(), Some("2026-11-07T09:00:00Z"));
    assert_eq!(list[0].session_id, None);
}

#[tokio::test]
async fn a_transcript_read_waits_for_the_log_to_grow_and_a_ninth_wait_answers_at_once() {
    let _turn = crate::runner::logwatch::TIMED.lock().await;
    let dir = tempfile::tempdir().unwrap();
    let (r, config_dir, _) = multi_rig(dir, vec![]);
    named_herdr(&config_dir.join("sessions/mono1"), "claude", "mono1 screen");
    let folder = r.dir.path().join("claude-projects/-Users-me-mono1");
    std::fs::create_dir_all(&folder).unwrap();
    let log = folder.join(format!("{CLAUDE_SESSION}.jsonl"));
    let said = |text: &str| {
        let line = json!({"type": "user", "uuid": text, "message": {"content": text}});
        format!("{line}\n")
    };
    std::fs::write(&log, said("first")).unwrap();
    let end = std::fs::metadata(&log).unwrap().len();
    let ask = move || Op::Transcript {
        agent: "mono1/claude".into(),
        after: Some(end),
        before: None,
        wait_ms: Some(20_000),
    };
    let page = |v: Value| serde_json::from_value::<crate::proto::TranscriptPage>(v).unwrap();

    let waits: Vec<_> = (0..crate::runner::logwatch::MAX_WAITS)
        .map(|_| {
            let (s, op) = (r.sessions.clone(), ask());
            tokio::spawn(async move { s.handle(op).await })
        })
        .collect();
    while r.sessions.transcript_waits.available_permits() > 0 {
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    let start = Instant::now();
    let ninth = page(r.sessions.handle(ask()).await.unwrap());
    assert!(
        start.elapsed() < Duration::from_secs(1),
        "{:?}",
        start.elapsed()
    );
    assert!(ninth.items.is_empty() && ninth.end == end);

    tokio::time::sleep(Duration::from_millis(500)).await;
    // Held open while the waits answer, as Codex keeps its log.
    let mut f = std::fs::OpenOptions::new().append(true).open(&log).unwrap();
    std::io::Write::write_all(&mut f, said("second").as_bytes()).unwrap();
    let start = Instant::now();
    for w in waits {
        let p = page(w.await.unwrap().unwrap());
        assert_eq!(p.items[0].text.as_deref(), Some("second"));
    }
    assert!(
        start.elapsed() < Duration::from_secs(1),
        "{:?}",
        start.elapsed()
    );
    assert_eq!(
        r.sessions.transcript_waits.available_permits(),
        crate::runner::logwatch::MAX_WAITS
    );
}

#[tokio::test]
async fn only_a_real_status_change_wakes_transcript_waits() {
    let dir = tempfile::tempdir().unwrap();
    let (r, _, _) = multi_rig(dir, vec![]);
    let mut woken = r.sessions.status_changes.subscribe();
    let mut forward = crate::runner::StatusForwarder::default();
    // Snapshots every few seconds report the same status again.
    for status in ["idle", "idle", "idle", "working", "working", "idle"] {
        forward.forward(&r.sessions, "mono1/claude".into(), status.into());
    }
    let mut n = 0;
    while let Ok(agent) = woken.try_recv() {
        assert_eq!(agent, "mono1/claude");
        n += 1;
    }
    assert_eq!(n, 3);
}

fn dialog_screen(name: &str, selected: u32) -> String {
    let text = std::fs::read_to_string(format!(
        "{}/tests/fixtures/dialogs/codex-0.165.0/{name}.txt",
        env!("CARGO_MANIFEST_DIR")
    ))
    .unwrap();
    text.replace("› 1. Yes", "  1. Yes")
        .replace(&format!("  {selected}. "), &format!("› {selected}. "))
}

fn blocked_codex() -> Value {
    let mut a = agent_info("blocked");
    a["agent"] = json!("codex");
    a["name"] = json!("cx");
    a["state_change_seq"] = json!(9);
    json!({"type": "agent_info", "agent": a})
}

#[tokio::test]
async fn an_outside_codex_dialog_is_listed_and_answered_by_arrows_then_enter() {
    let dir = tempfile::tempdir().unwrap();
    let herdr = FakeHerdr::start(
        dir.path(),
        json!({}),
        vec![
            dialog_screen("permission-command", 1),
            dialog_screen("permission-command", 1),
            dialog_screen("permission-command", 3),
            std::fs::read_to_string(format!(
                "{}/tests/fixtures/dialogs/codex-0.165.0/after-deny-command.txt",
                env!("CARGO_MANIFEST_DIR")
            ))
            .unwrap(),
        ],
    );
    herdr.answer("agent.get", blocked_codex());
    let r = rig_with(dir, &herdr.socket, "codex-cli 0.165.0");
    let v = r
        .sessions
        .handle(Op::Dialog { agent: "cx".into() })
        .await
        .unwrap();
    let d: crate::proto::DialogView = serde_json::from_value(v["dialog"].clone()).unwrap();
    assert!(d.answerable, "{d:?}");
    let no = &d.options[2];
    assert!(no.is_deny && no.offered);
    r.sessions
        .handle(Op::Answer {
            agent: "cx".into(),
            occurrence: d.occurrence.clone(),
            option: no.id.clone(),
            label_sha256: no.label_sha256.clone(),
            deny_only: true,
        })
        .await
        .unwrap();
    let keys: Vec<Value> = herdr
        .calls
        .lock()
        .unwrap()
        .iter()
        .filter(|c| c["method"] == "agent.send_keys")
        .map(|c| c["params"]["keys"].clone())
        .collect();
    assert_eq!(keys, [json!(["down", "down"]), json!(["enter"])]);
}

#[tokio::test]
async fn a_claude_session_switchboard_started_never_gets_dialog_keys() {
    let dir = tempfile::tempdir().unwrap();
    let screen = std::fs::read_to_string(format!(
        "{}/tests/fixtures/dialogs/claude-2.1.312/permission-bash.txt",
        env!("CARGO_MANIFEST_DIR")
    ))
    .unwrap();
    let herdr = FakeHerdr::start(dir.path(), json!({}), vec![screen]);
    let mut a = agent_info("blocked");
    a["agent"] = json!("claude");
    herdr.answer("agent.get", json!({"type": "agent_info", "agent": a}));
    let r = rig_with(dir, &herdr.socket, "codex-cli 0.165.0");
    std::fs::create_dir_all(r.sessions.dir("s1")).unwrap();
    r.sessions
        .save(&RunnerSession {
            session_id: "s1".into(),
            agent: Agent::Claude,
            project_id: "p".into(),
            repo_path: "/srv/repo".into(),
            branch: "sb/s1".into(),
            deny_patterns: vec![],
            agent_name: "sb-s1".into(),
            workspace_id: Some("w2".into()),
            pane_id: Some("w2:p1".into()),
            worktree_path: None,
            codex_version: None,
            start_commit: None,
            herdr_session: None,
            launched: true,
            core_id: None,
            terminal_identity: None,
        })
        .unwrap();
    let v = r
        .sessions
        .handle(Op::Dialog {
            agent: "sb-s1".into(),
        })
        .await
        .unwrap();
    let d: crate::proto::DialogView = serde_json::from_value(v["dialog"].clone()).unwrap();
    assert_eq!((d.answerable, d.reason.as_deref()), (false, Some("hooked")));
    let yes = &d.options[0];
    let e = r
        .sessions
        .handle(Op::Answer {
            agent: "sb-s1".into(),
            occurrence: d.occurrence.clone(),
            option: yes.id.clone(),
            label_sha256: yes.label_sha256.clone(),
            deny_only: false,
        })
        .await
        .unwrap_err();
    assert_eq!(e.code, ErrorCode::DialogChanged);
    assert!(!herdr.methods().contains(&"agent.send_keys".to_string()));
}

#[tokio::test]
async fn an_agent_blocked_through_the_hold_is_reported_waiting_once() {
    let dir = tempfile::tempdir().unwrap();
    let herdr = FakeHerdr::start(
        dir.path(),
        json!({}),
        vec![dialog_screen("permission-command", 1)],
    );
    herdr.answer("agent.get", blocked_codex());
    let mut r = rig_with(dir, &herdr.socket, "codex-cli 0.165.0");
    r.sessions.check_waiting("cx").await.unwrap();
    r.sessions.check_waiting("cx").await.unwrap();
    let mut waiting = Vec::new();
    while let Ok(ev) = r.events.try_recv() {
        if let Event::Waiting {
            agent_name,
            terminal_id,
            session_id,
            ..
        } = ev
        {
            waiting.push((agent_name, terminal_id, session_id));
        }
    }
    assert_eq!(waiting, [("cx".to_string(), "t".to_string(), None)]);
}

/// A Herdr session whose one named agent's pane runs `foreground`.
fn herdr_running(folder: &Path, agent: &'static str, foreground: &'static str) {
    std::fs::create_dir_all(folder).unwrap();
    let listener = UnixListener::bind(folder.join("herdr.sock")).unwrap();
    tokio::spawn(async move {
        loop {
            let (stream, _) = listener.accept().await.unwrap();
            let (read, mut write) = stream.into_split();
            let mut line = String::new();
            BufReader::new(read).read_line(&mut line).await.unwrap();
            let req: Value = serde_json::from_str(&line).unwrap();
            let result = match req["method"].as_str().unwrap() {
                "ping" => json!({"type": "pong", "version": "0.9.1", "protocol": 22}),
                "session.snapshot" => {
                    let mut a = agent_info("working");
                    a["name"] = json!(agent);
                    a["agent"] = json!("claude");
                    json!({"type": "session_snapshot", "snapshot": {"version": "0.9.1", "protocol": 22,
                           "workspaces": [], "tabs": [], "panes": [], "layouts": [], "agents": [a]}})
                }
                "pane.process_info" => json!({"type": "pane_process_info", "process_info": {
                    "pane_id": "w2:p1", "foreground_processes": [{"pid": 7, "name": foreground}]}}),
                _ => json!({"type": "ok"}),
            };
            let resp = json!({"id": req["id"], "result": result});
            write
                .write_all(format!("{resp}\n").as_bytes())
                .await
                .unwrap();
        }
    });
}

#[tokio::test]
async fn core_agents_are_listed_beside_herdr_ones_and_attached_panes_once() {
    let dir = tempfile::tempdir().unwrap();
    let (r, config_dir, _) = multi_rig(dir, vec![]);
    herdr_running(&config_dir.join("sessions/mono1"), "claude", "2.1.322");
    // A TUI pane showing a core terminal: Herdr's Claude hook may report into it.
    herdr_running(&config_dir.join("sessions/viewer"), "claude", "hc");
    let fake = crate::hc::fake::FakeCore::start();
    fake.set_doc(json!({ "spaces": [ { "id": "sp1", "name": "Switchboard",
        "tabs": [ { "id": "tb1", "name": "fix login" } ] } ] }));
    fake.add(json!({ "argv": ["/x/claude"], "reported": true,
                     "status": { "state": "blocked", "app": "claude-code", "kind": "permission" },
                     "labels": { "herdr.space": "sp1", "herdr.tab": "tb1" } }));
    fake.add(json!({ "argv": ["/bin/zsh"] }));
    let core = core_watch::Core::new(&fake.socket);
    r.sessions.set_core(core.clone());

    let names = |list: Vec<crate::proto::AgentSummary>| -> Vec<(String, String, Option<String>)> {
        list.into_iter()
            .map(|a| (a.name, a.agent_status, a.space))
            .collect()
    };
    let agents = || async {
        serde_json::from_value::<Vec<crate::proto::AgentSummary>>(
            r.sessions.handle(Op::Agents).await.unwrap(),
        )
        .unwrap()
    };
    // The core isn't up yet: no dial has answered.
    assert_eq!(
        names(agents().await),
        [
            ("mono1/claude".into(), "working".into(), None),
            ("viewer/claude".into(), "working".into(), None)
        ]
    );
    core.up("core-a");
    let list = agents().await;
    assert_eq!(list[1].herdr_session.as_deref(), Some("hc"));
    assert_eq!(list[1].pane_id, "t1");
    assert_eq!(
        names(list),
        [
            ("mono1/claude".into(), "working".into(), None),
            (
                "hc/fix login".into(),
                "blocked".into(),
                Some("Switchboard".into())
            ),
        ]
    );
}

/// A runner with no Herdr session running and `fake` as its core, already up.
fn core_rig(dir: tempfile::TempDir) -> (Rig, crate::hc::fake::FakeCore) {
    let (r, _, _) = multi_rig(dir, vec![]);
    let fake = crate::hc::fake::FakeCore::start();
    let core = core_watch::Core::new(&fake.socket);
    core.up("core-a");
    r.sessions.set_core(core);
    (r, fake)
}

fn claude_terminal(name: &str, state: &str) -> Value {
    json!({ "argv": ["/x/claude"], "name": name, "reported": true, "cwd": "/w",
            "status": { "state": state, "app": "claude-code" } })
}

#[tokio::test]
async fn core_agents_are_read_and_prompted_by_name_and_never_forced() {
    let dir = tempfile::tempdir().unwrap();
    let (r, fake) = core_rig(dir);
    let t = fake.add(claude_terminal("worker", "idle"));
    let shell = fake.add(json!({ "argv": ["/bin/zsh"] }));
    fake.set_screen(&t, "the screen");
    let read = |visible: bool, lines: u32| Op::ReadAgent {
        agent: "hc/worker".into(),
        lines,
        visible,
        ansi: true,
    };
    let v = r.sessions.handle(read(true, 0)).await.unwrap();
    assert_eq!(
        (v["text"].as_str(), v["ansi"].as_str()),
        (Some("the screen"), Some("the screen"))
    );
    r.sessions.handle(read(false, 30)).await.unwrap();
    let reads: Vec<Value> = fake
        .requests()
        .into_iter()
        .filter(|q| q["method"] == "terminal.read")
        .map(|q| q["params"].clone())
        .collect();
    assert_eq!(reads[0], json!({ "terminal": t, "lines": 0 }));
    assert!(reads.contains(&json!({ "terminal": t, "lines": 30 })));

    let prompt = |agent: &str| Op::PromptAgent {
        agent: agent.into(),
        text: "fix it".into(),
    };
    let sent = r.sessions.handle(prompt("hc/worker")).await.unwrap();
    assert_eq!(sent["outcome"], "sent");
    let sends: Vec<Value> = fake
        .requests()
        .into_iter()
        .filter(|q| q["method"] == "terminal.send")
        .map(|q| q["params"].clone())
        .collect();
    assert_eq!(sends, [json!({ "terminal": t, "text": "fix it" })]);

    fake.patch(
        &t,
        json!({ "status": { "state": "blocked", "app": "claude-code" } }),
    );
    let blocked = r.sessions.handle(prompt("hc/worker")).await.unwrap();
    assert_eq!(
        (blocked["outcome"].as_str(), blocked["screen"].as_str()),
        (Some("blocked"), Some("the screen"))
    );

    fake.patch(
        &t,
        json!({ "status": { "state": "idle", "app": "claude-code" } }),
    );
    fake.script("terminal.send", crate::hc::fake::Script::Drop);
    let lost = r.sessions.handle(prompt("hc/worker")).await.unwrap_err();
    assert_eq!(lost.code, ErrorCode::OutcomeUnknown);

    // A plain shell is never typed into, by address or otherwise.
    let shell_prompt = r
        .sessions
        .handle(prompt(&format!("hc/{shell}")))
        .await
        .unwrap();
    assert_eq!(shell_prompt["outcome"], "not_found");
    assert!(
        !fake
            .requests()
            .iter()
            .any(|q| q["method"] == "terminal.send" && q["params"]["terminal"] == shell)
    );
}

#[tokio::test]
async fn a_core_claudes_conversation_is_found_through_its_process_record() {
    let dir = tempfile::tempdir().unwrap();
    let (r, fake) = core_rig(dir);
    let t = fake.add(claude_terminal("worker", "idle"));
    let fg = fake.terminal(&t).unwrap()["foreground"]["pid"]
        .as_u64()
        .unwrap();
    let records = r.dir.path().join("sessions");
    std::fs::create_dir_all(&records).unwrap();
    std::fs::write(
        records.join(format!("{fg}.json")),
        json!({ "pid": fg, "sessionId": CLAUDE_SESSION }).to_string(),
    )
    .unwrap();
    let folder = r.dir.path().join("claude-projects/-w");
    std::fs::create_dir_all(&folder).unwrap();
    let line = json!({"type": "user", "uuid": "u1", "message": {"content": "on core"}});
    std::fs::write(
        folder.join(format!("{CLAUDE_SESSION}.jsonl")),
        format!("{line}\n"),
    )
    .unwrap();
    let page: crate::proto::TranscriptPage = serde_json::from_value(
        r.sessions
            .handle(Op::Transcript {
                agent: "hc/worker".into(),
                after: None,
                before: None,
                wait_ms: None,
            })
            .await
            .unwrap(),
    )
    .unwrap();
    assert_eq!(page.items[0].text.as_deref(), Some("on core"));
}

fn codex_terminal(kind: &str) -> Value {
    json!({ "argv": ["/x/codex"], "name": "cx", "reported": true, "turn": 3, "status_ms": 77,
            "status": { "state": "blocked", "app": "codex", "kind": kind } })
}

#[tokio::test]
async fn a_core_codex_dialog_is_answered_by_arrows_then_enter_on_its_terminal() {
    let dir = tempfile::tempdir().unwrap();
    let (r, fake) = core_rig(dir);
    let t = fake.add(codex_terminal("permission"));
    fake.queue_screens(
        &t,
        vec![
            dialog_screen("permission-command", 1),
            dialog_screen("permission-command", 1),
            dialog_screen("permission-command", 3),
            std::fs::read_to_string(format!(
                "{}/tests/fixtures/dialogs/codex-0.165.0/after-deny-command.txt",
                env!("CARGO_MANIFEST_DIR")
            ))
            .unwrap(),
        ],
    );
    let v = r
        .sessions
        .handle(Op::Dialog {
            agent: "hc/cx".into(),
        })
        .await
        .unwrap();
    let d: crate::proto::DialogView = serde_json::from_value(v["dialog"].clone()).unwrap();
    assert!(d.answerable, "{d:?}");
    let no = &d.options[2];
    r.sessions
        .handle(Op::Answer {
            agent: "hc/cx".into(),
            occurrence: d.occurrence.clone(),
            option: no.id.clone(),
            label_sha256: no.label_sha256.clone(),
            deny_only: true,
        })
        .await
        .unwrap();
    let keys: Vec<Value> = fake
        .requests()
        .into_iter()
        .filter(|q| q["method"] == "terminal.keys")
        .map(|q| q["params"].clone())
        .collect();
    assert_eq!(
        keys,
        [
            json!({ "terminal": t, "keys": ["down", "down"] }),
            json!({ "terminal": t, "keys": ["enter"] })
        ]
    );
}

#[tokio::test]
async fn a_dialog_the_core_calls_something_else_is_shown_read_only() {
    let dir = tempfile::tempdir().unwrap();
    let (r, fake) = core_rig(dir);
    let t = fake.add(codex_terminal("question"));
    fake.set_screen(&t, &dialog_screen("permission-command", 1));
    let v = r
        .sessions
        .handle(Op::Dialog {
            agent: "hc/cx".into(),
        })
        .await
        .unwrap();
    let d: crate::proto::DialogView = serde_json::from_value(v["dialog"].clone()).unwrap();
    assert_eq!(
        (d.answerable, d.reason.as_deref()),
        (false, Some("read_only"))
    );
    assert!(!fake.methods().contains(&"terminal.keys".to_owned()));
}

#[tokio::test]
async fn a_new_process_in_a_core_terminal_is_a_new_occurrence() {
    let dir = tempfile::tempdir().unwrap();
    let (r, fake) = core_rig(dir);
    let t = fake.add(codex_terminal("permission"));
    fake.set_screen(&t, &dialog_screen("permission-command", 1));
    let occurrence = || async {
        r.sessions
            .handle(Op::Dialog {
                agent: "hc/cx".into(),
            })
            .await
            .unwrap()["occurrence"]
            .as_str()
            .unwrap()
            .to_owned()
    };
    let first = occurrence().await;
    assert_eq!(first, occurrence().await);
    fake.patch(
        &t,
        json!({ "foreground": { "pid": 999, "name": "codex", "argv": ["codex"], "cwd": "/w" } }),
    );
    let other_process = occurrence().await;
    fake.patch(&t, json!({ "turn": 4 }));
    let next_turn = occurrence().await;
    assert!(first != other_process && other_process != next_turn);
}

/// A runner whose new sessions go to the fake core, with a real git repo to start them in.
fn core_start_rig(mode: crate::config::NewSessions) -> (Rig, crate::hc::fake::FakeCore, PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    let repo = dir.path().join("repo");
    std::fs::create_dir_all(&repo).unwrap();
    let sh = |args: &[&str]| {
        assert!(
            std::process::Command::new("git")
                .arg("-C")
                .arg(&repo)
                .args(args)
                .status()
                .unwrap()
                .success()
        );
    };
    sh(&["init", "-q", "-b", "main"]);
    std::fs::write(repo.join("a.txt"), "a\n").unwrap();
    sh(&["add", "."]);
    sh(&[
        "-c",
        "user.email=t@t",
        "-c",
        "user.name=t",
        "commit",
        "-qm",
        "init",
    ]);
    let root = dir.path().to_owned();
    let (r, fake) = core_rig(dir);
    r.sessions.set_core_starts(CoreStarts {
        new_sessions: mode,
        worktrees: root.join("worktrees"),
        home: root.clone(),
        bins: Some(PathBuf::from("/opt/agents")),
    });
    (r, fake, repo)
}

fn core_req(agent: Agent, repo: &Path) -> StartRequest {
    StartRequest {
        repo_path: repo.to_owned(),
        ..start_req(agent)
    }
}

/// Stands in for the agent reporting idle once its terminal exists.
fn report_idle(fake: &crate::hc::fake::FakeCore, app: &'static str) {
    let handle = fake.handle();
    tokio::spawn(async move {
        loop {
            if handle.patch(
                "t1",
                json!({ "reported": true,
                "status": { "state": "idle", "app": app }, "status_ms": 5 }),
            ) {
                return;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    });
}

fn requests_of(fake: &crate::hc::fake::FakeCore, method: &str) -> Vec<Value> {
    fake.requests()
        .into_iter()
        .filter(|q| q["method"] == method)
        .map(|q| q["params"].clone())
        .collect()
}

#[tokio::test]
async fn a_codex_session_starts_on_core_in_a_git_worktree_the_app_adopts() {
    let (r, fake, repo) = core_start_rig(crate::config::NewSessions::Hc);
    fake.queue_screens("t1", vec![fixture("folder-access"), fixture("ready")]);
    report_idle(&fake, "codex");
    let started = r
        .sessions
        .start(core_req(Agent::Codex, &repo))
        .await
        .unwrap();
    assert_eq!(started["herdr_session"], "hc");
    assert_eq!(started["pane_id"], "t1");
    let space = started["workspace_id"].as_str().unwrap().to_owned();
    assert!(space.starts_with("sp") && space.len() == 34, "{space}");

    let run = &requests_of(&fake, "terminal.run")[0];
    let argv: Vec<&str> = run["argv"]
        .as_array()
        .unwrap()
        .iter()
        .map(|a| a.as_str().unwrap())
        .collect();
    assert_eq!(&argv[..2], ["/bin/sh", "-c"]);
    assert!(
        argv[2].contains("HERDR_*|CLAUDE_CODE*|CLAUDECODE") && argv[2].ends_with("exec \"$@\"")
    );
    assert_eq!(
        &argv[3..6],
        ["sb-launch", "/opt/agents/codex", "--no-daemon"]
    );
    let wt = r.dir.path().join("worktrees/repo/sb-s1");
    assert_eq!(run["cwd"], wt.to_string_lossy().as_ref());
    assert_eq!(run["name"], "sb-s1");
    assert_eq!(
        (run["keep"].as_bool(), run["no_parent"].as_bool()),
        (Some(true), Some(true))
    );
    let labels = &run["labels"];
    assert_eq!(labels["herdr.space"], space.as_str());
    assert!(labels["herdr.tab"].as_str().unwrap().starts_with("tb"));
    assert_eq!(
        (
            labels["herdr.seen"].as_str(),
            labels["switchboard.session"].as_str()
        ),
        (Some("0"), Some("s1"))
    );
    assert!(wt.join("a.txt").exists(), "a git worktree");

    assert_eq!(
        requests_of(&fake, "terminal.keys"),
        [json!({ "terminal": "t1", "keys": ["enter"] })]
    );
    assert_eq!(
        requests_of(&fake, "terminal.send"),
        [json!({ "terminal": "t1", "text": "do the task" })]
    );
    let s = r.sessions.get("s1").unwrap();
    assert_eq!(
        (
            s.core_id.as_deref(),
            s.terminal_identity.as_deref(),
            s.launched
        ),
        (Some("core-a"), Some("core-a:t1:1001:1000001"), true)
    );
    assert!(s.start_commit.is_some());
    let owned = r.sessions.owned().await;
    assert_eq!(
        (
            owned[0].workspace_id.as_str(),
            owned[0].agent_status.as_str()
        ),
        (space.as_str(), "idle")
    );

    // Stop: the terminal ends, and the clean, merged worktree goes.
    let v = r
        .sessions
        .handle(Op::Stop {
            session_id: "s1".into(),
            remove_worktree_if_clean: true,
        })
        .await
        .unwrap();
    assert_eq!(v["worktree_removed"], true, "{v}");
    assert!(!wt.exists());
    assert_eq!(
        requests_of(&fake, "terminal.kill"),
        [json!({ "terminal": "t1" })]
    );
    assert_eq!(
        requests_of(&fake, "terminal.rm"),
        [json!({ "terminal": "t1" })]
    );

    // Delete then has no server of its own to delete.
    let d = r
        .sessions
        .handle(Op::Delete {
            session_id: "s1".into(),
            herdr_session: Some("hc".into()),
            repo_path: Some(repo.to_string_lossy().into_owned()),
            worktree_path: Some(wt.to_string_lossy().into_owned()),
            branch: Some("sb/s1".into()),
        })
        .await
        .unwrap();
    assert_eq!(d["herdr_session_deleted"], false);
}

#[tokio::test]
async fn a_claude_session_on_core_waits_for_its_hook_then_idle() {
    let (r, fake, repo) = core_start_rig(crate::config::NewSessions::Auto);
    let sessions = r.sessions.clone();
    let start = tokio::spawn(async move { sessions.start(core_req(Agent::Claude, &repo)).await });
    while r.sessions.get("s1").and_then(|s| s.pane_id).is_none() {
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert!(
        requests_of(&fake, "terminal.send").is_empty(),
        "nothing before the hook"
    );
    r.sessions.hooks.test_mark_live("s1");
    tokio::time::sleep(Duration::from_millis(600)).await;
    assert!(
        requests_of(&fake, "terminal.send").is_empty(),
        "nothing before idle"
    );
    report_idle(&fake, "claude-code");
    start.await.unwrap().unwrap();
    let run = &requests_of(&fake, "terminal.run")[0];
    assert_eq!(run["argv"][4], "/opt/agents/claude");
    assert_eq!(requests_of(&fake, "terminal.send").len(), 1);
}

#[tokio::test]
async fn hc_with_no_core_answering_fails_and_auto_falls_back_to_herdr() {
    let (r, fake, repo) = core_start_rig(crate::config::NewSessions::Hc);
    drop(fake);
    let e = r
        .sessions
        .start(core_req(Agent::Codex, &repo))
        .await
        .unwrap_err();
    assert!(e.message.contains("new_sessions is \"hc\""), "{e:?}");
    assert!(r.sessions.get("s1").is_none() && !r.dir.path().join("state/sessions/s1").exists());

    // With `auto`, the same start goes to a Herdr session of its own.
    let dir = tempfile::tempdir().unwrap();
    let (r, _, _) = multi_rig(dir, vec![fixture("folder-access"), fixture("ready")]);
    let core = core_watch::Core::new("/nonexistent/core.sock");
    r.sessions.set_core(core);
    r.sessions.set_core_starts(CoreStarts {
        new_sessions: crate::config::NewSessions::Auto,
        worktrees: "/nonexistent".into(),
        home: "/nonexistent".into(),
        bins: None,
    });
    let started = r.sessions.start(start_req(Agent::Codex)).await.unwrap();
    assert_eq!(started["herdr_session"], "p-s1");
}

#[tokio::test]
async fn a_core_start_that_never_gets_ready_is_undone() {
    let (r, fake, repo) = core_start_rig(crate::config::NewSessions::Hc);
    fake.set_screen("t1", "  something we have never seen\n");
    let e = r
        .sessions
        .start(core_req(Agent::Codex, &repo))
        .await
        .unwrap_err();
    assert_eq!(e.code, ErrorCode::StartupFailed, "{e:?}");
    assert!(
        requests_of(&fake, "terminal.send").is_empty()
            && requests_of(&fake, "terminal.keys").is_empty()
    );
    assert_eq!(
        requests_of(&fake, "terminal.kill"),
        [json!({ "terminal": "t1" })]
    );
    assert!(r.sessions.get("s1").is_none());
}

#[tokio::test]
async fn an_interrupted_core_start_is_undone_by_its_label() {
    let (r, fake, repo) = core_start_rig(crate::config::NewSessions::Hc);
    fake.queue_screens("t1", vec![fixture("folder-access"), fixture("ready")]);
    report_idle(&fake, "codex");
    r.sessions
        .start(core_req(Agent::Codex, &repo))
        .await
        .unwrap();
    // As a runner that died before saving the terminal left it.
    let meta = r.dir.path().join("state/sessions/s1/meta.json");
    let mut saved: Value = serde_json::from_slice(&std::fs::read(&meta).unwrap()).unwrap();
    saved["launched"] = json!(false);
    saved["pane_id"] = Value::Null;
    std::fs::write(&meta, saved.to_string()).unwrap();
    r.sessions.load().unwrap();
    assert_eq!(r.sessions.undo_interrupted_starts().await, 1);
    assert_eq!(
        requests_of(&fake, "terminal.kill"),
        [json!({ "terminal": "t1" })]
    );
    // A terminal without the session's label is never touched.
    let other = fake.add(json!({ "argv": ["claude"], "labels": { "switchboard.session": "s9" } }));
    assert!(
        !requests_of(&fake, "terminal.kill")
            .iter()
            .any(|k| k["terminal"] == other)
    );
}

/// The next exit, or `None` at the next idle status.
async fn exit_or_idle(rx: &mut mpsc::Receiver<Event>) -> Option<String> {
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            match rx.recv().await {
                Some(Event::SessionExited { session_id }) => return Some(session_id),
                Some(Event::AgentStatus { agent_status, .. }) if agent_status == "idle" => {
                    return None;
                }
                Some(_) => {}
                None => panic!("events closed"),
            }
        }
    })
    .await
    .unwrap()
}

#[tokio::test]
async fn a_core_task_whose_terminal_is_gone_or_replaced_exits() {
    let (mut r, fake, repo) = core_start_rig(crate::config::NewSessions::Hc);
    fake.queue_screens("t1", vec![fixture("folder-access"), fixture("ready")]);
    report_idle(&fake, "codex");
    r.sessions
        .start(core_req(Agent::Codex, &repo))
        .await
        .unwrap();
    let core = r.sessions.core().unwrap();
    tokio::spawn(core_watch::follow(core.clone(), || {}, || {}));
    tokio::spawn(super::super::watch::watch_core(
        r.sessions.clone(),
        core.clone(),
    ));
    assert_eq!(exit_or_idle(&mut r.events).await, None, "running first");
    // The program is lost and the kept terminal gets another process in the same address.
    fake.patch("t1", json!({ "pid": 4242 }));
    fake.drop_subscribers();
    assert_eq!(exit_or_idle(&mut r.events).await.as_deref(), Some("s1"));
}

#[tokio::test(start_paused = true)]
async fn core_tasks_exit_once_the_core_has_been_gone_a_while() {
    let (mut r, _, _) = core_start_rig(crate::config::NewSessions::Hc);
    let core = core_watch::Core::new("/nonexistent/core.sock");
    std::fs::create_dir_all(r.sessions.dir("s1")).unwrap();
    r.sessions
        .save(&RunnerSession {
            session_id: "s1".into(),
            agent: Agent::Codex,
            project_id: "p".into(),
            repo_path: "/srv/repo".into(),
            branch: "sb/s1".into(),
            deny_patterns: vec![],
            agent_name: "sb-s1".into(),
            workspace_id: Some("sp1".into()),
            pane_id: Some("t1".into()),
            worktree_path: None,
            codex_version: None,
            start_commit: None,
            herdr_session: Some("hc".into()),
            launched: true,
            core_id: Some("core-a".into()),
            terminal_identity: Some("core-a:t1:1:1".into()),
        })
        .unwrap();
    tokio::spawn(super::super::watch::watch_core(r.sessions.clone(), core));
    let exited = tokio::time::timeout(Duration::from_secs(300), async {
        loop {
            if let Some(Event::SessionExited { session_id }) = r.events.recv().await {
                return session_id;
            }
        }
    })
    .await
    .unwrap();
    assert_eq!(exited, "s1");
}

#[tokio::test]
async fn the_app_adopted_tab_and_space_are_named_after_the_session_and_project() {
    let (r, fake, repo) = core_start_rig(crate::config::NewSessions::Hc);
    fake.queue_screens("t1", vec![fixture("folder-access"), fixture("ready")]);
    report_idle(&fake, "codex");
    r.sessions
        .start(core_req(Agent::Codex, &repo))
        .await
        .unwrap();
    let labels = requests_of(&fake, "terminal.run")[0]["labels"].clone();
    // The app adopts the ids as a new space and tab, with fields of its own.
    fake.set_doc(json!({ "spaces": [
        { "id": "spOther", "name": "mine", "tabs": [] },
        { "id": labels["herdr.space"], "color": "blue",
          "tabs": [ { "id": labels["herdr.tab"], "root": { "content": { "terminal": "t1" } } } ] },
    ], "app": { "kept": true } }));
    for _ in 0..60 {
        if fake.doc().0["spaces"][1]["tabs"][0]["name"].is_string() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    let (doc, _) = fake.doc();
    assert_eq!(doc["spaces"][1]["tabs"][0]["name"], "p-s1");
    assert_eq!(doc["spaces"][1]["name"], "p");
    assert_eq!(doc["spaces"][0]["name"], "mine");
    assert_eq!(
        (
            doc["spaces"][1]["color"].as_str(),
            doc["app"]["kept"].as_bool()
        ),
        (Some("blue"), Some(true))
    );
}

#[tokio::test]
async fn a_core_task_exits_when_another_core_answers() {
    let (mut r, fake, repo) = core_start_rig(crate::config::NewSessions::Hc);
    fake.queue_screens("t1", vec![fixture("folder-access"), fixture("ready")]);
    report_idle(&fake, "codex");
    r.sessions
        .start(core_req(Agent::Codex, &repo))
        .await
        .unwrap();
    let core = r.sessions.core().unwrap();
    tokio::spawn(core_watch::follow(core.clone(), || {}, || {}));
    tokio::spawn(super::super::watch::watch_core(
        r.sessions.clone(),
        core.clone(),
    ));
    assert_eq!(exit_or_idle(&mut r.events).await, None);
    // Its state was wiped: same addresses, another core.
    fake.set_core_id("core-b");
    fake.drop_subscribers();
    assert_eq!(exit_or_idle(&mut r.events).await.as_deref(), Some("s1"));
    // And its terminals are never ended from the new core.
    r.sessions
        .handle(Op::Stop {
            session_id: "s1".into(),
            remove_worktree_if_clean: false,
        })
        .await
        .unwrap();
    assert!(requests_of(&fake, "terminal.kill").is_empty());
}

#[tokio::test]
async fn a_failed_core_start_stops_no_herdr_session() {
    use std::os::unix::fs::PermissionsExt;
    let dir = tempfile::tempdir().unwrap();
    let bin = dir.path().join("herdr");
    let calls = dir.path().join("herdr-calls");
    std::fs::write(
        &bin,
        format!("#!/bin/sh\necho \"$@\" >> '{}'\n", calls.display()),
    )
    .unwrap();
    std::fs::set_permissions(&bin, std::fs::Permissions::from_mode(0o755)).unwrap();
    let starter = Starter::Herdr {
        bin,
        xdg: None,
        log_dir: dir.path().to_owned(),
    };
    let herdrs = Herdrs::multi(
        dir.path().join("herdr"),
        dir.path().join("nowhere.sock"),
        starter,
    );
    let (tx, _events) = mpsc::channel(64);
    let hooks = HookHub::new(tx.clone(), Duration::from_secs(5));
    let cfg = SessionsConfig {
        state_dir: dir.path().join("state"),
        claude_config: dir.path().join("claude.json"),
        claude_mode: Default::default(),
        claude_projects: dir.path().join("claude-projects"),
        codex_sessions: PathBuf::from("/nonexistent/codex/sessions"),
        exe: PathBuf::from("/opt/sb/switchboard"),
        hook_addr: "127.0.0.1:9".parse().unwrap(),
        codex_version_cmd: vec!["echo".into(), "codex-cli 0.163.0".into()],
        liveness_timeout: Duration::from_secs(1),
        startup_timeout: Duration::from_secs(2),
    };
    let sessions = Sessions::with_herdrs(herdrs, hooks, tx, cfg);
    let fake = crate::hc::fake::FakeCore::start();
    let core = core_watch::Core::new(&fake.socket);
    core.up("core-a");
    sessions.set_core(core);
    sessions.set_core_starts(CoreStarts {
        new_sessions: crate::config::NewSessions::Hc,
        worktrees: dir.path().join("worktrees"),
        home: dir.path().to_owned(),
        bins: Some(PathBuf::from("/opt/agents")),
    });
    // Not a git repo: the worktree fails after the session is recorded on core.
    let e = sessions
        .start(core_req(Agent::Codex, &dir.path().join("no-repo")))
        .await
        .unwrap_err();
    assert_eq!(e.code, ErrorCode::Internal, "{e:?}");
    assert!(sessions.get("s1").is_none());
    assert!(!calls.exists(), "herdr was run");
}

/// A Claude on the fake core, blocked at a permission dialog.
fn blocked_core_claude(fake: &crate::hc::fake::FakeCore, name: &str) -> String {
    let t = fake.add(
        json!({ "argv": ["/x/claude"], "name": name, "reported": true,
        "status": { "state": "blocked", "app": "claude-code", "kind": "permission" } }),
    );
    fake.set_screen(
        &t,
        &std::fs::read_to_string(format!(
            "{}/tests/fixtures/dialogs/core/claude-2.1.322/permission-bash.txt",
            env!("CARGO_MANIFEST_DIR")
        ))
        .unwrap(),
    );
    t
}

/// Session `s1`, a Claude Switchboard started at `pane` on core `core_id`.
fn save_core_claude_session(r: &Rig, pane: &str, core_id: &str, identity: &str) {
    std::fs::create_dir_all(r.sessions.dir("s1")).unwrap();
    r.sessions
        .save(&RunnerSession {
            session_id: "s1".into(),
            agent: Agent::Claude,
            project_id: "p".into(),
            repo_path: "/srv/repo".into(),
            branch: "sb/s1".into(),
            deny_patterns: vec![],
            agent_name: "sb-s1".into(),
            workspace_id: Some("sp1".into()),
            pane_id: Some(pane.into()),
            worktree_path: None,
            codex_version: None,
            start_commit: None,
            herdr_session: Some("hc".into()),
            launched: true,
            core_id: Some(core_id.into()),
            terminal_identity: Some(identity.into()),
        })
        .unwrap();
}

#[tokio::test]
async fn an_owned_core_claude_asked_for_by_address_never_gets_dialog_keys() {
    let dir = tempfile::tempdir().unwrap();
    let (r, fake) = core_rig(dir);
    let t = blocked_core_claude(&fake, "sb-s1");
    let rec = fake.terminal(&t).unwrap();
    let identity = format!("core-a:{t}:{}:{}", rec["pid"], rec["created_ms"]);
    save_core_claude_session(&r, &t, "core-a", &identity);
    let agent = format!("hc/{t}");
    let v = r
        .sessions
        .handle(Op::Dialog {
            agent: agent.clone(),
        })
        .await
        .unwrap();
    let d: crate::proto::DialogView = serde_json::from_value(v["dialog"].clone()).unwrap();
    assert_eq!((d.answerable, d.reason.as_deref()), (false, Some("hooked")));
    let yes = &d.options[0];
    let e = r
        .sessions
        .handle(Op::Answer {
            agent,
            occurrence: d.occurrence.clone(),
            option: yes.id.clone(),
            label_sha256: yes.label_sha256.clone(),
            deny_only: false,
        })
        .await
        .unwrap_err();
    assert_eq!(e.code, ErrorCode::DialogChanged);
    assert!(!fake.methods().contains(&"terminal.keys".to_owned()));
}

#[tokio::test]
async fn a_core_stop_whose_terminal_wont_end_keeps_the_worktree() {
    let (r, fake, repo) = core_start_rig(crate::config::NewSessions::Hc);
    fake.queue_screens("t1", vec![fixture("folder-access"), fixture("ready")]);
    report_idle(&fake, "codex");
    let started = r
        .sessions
        .start(core_req(Agent::Codex, &repo))
        .await
        .unwrap();
    let wt = PathBuf::from(started["worktree_path"].as_str().unwrap());
    fake.script("terminal.kill", crate::hc::fake::Script::Error("internal"));
    let v = r
        .sessions
        .handle(Op::Stop {
            session_id: "s1".into(),
            remove_worktree_if_clean: true,
        })
        .await
        .unwrap();
    assert_eq!(v["worktree_removed"], false, "{v}");
    assert_eq!(v["kept_because"], "the agent's terminal couldn't be ended");
    assert!(wt.exists(), "clean and merged, but kept");
    assert!(r.sessions.get("s1").is_none(), "the stop completes");
}

#[tokio::test]
async fn a_session_from_an_earlier_core_doesnt_own_the_new_cores_terminal_at_its_address() {
    let dir = tempfile::tempdir().unwrap();
    let (r, fake) = core_rig(dir);
    save_core_claude_session(&r, "t1", "core-a", "core-a:t1:1001:1000001");
    // A replacement core numbers its terminals from t1 again.
    fake.set_core_id("core-b");
    r.sessions.core().unwrap().up("core-b");
    let t = blocked_core_claude(&fake, "other");
    assert_eq!(t, "t1");
    let v = r
        .sessions
        .handle(Op::Dialog {
            agent: "hc/t1".into(),
        })
        .await
        .unwrap();
    let d: crate::proto::DialogView = serde_json::from_value(v["dialog"].clone()).unwrap();
    assert!(d.answerable, "not the stale session's: {d:?}");
}

#[tokio::test]
async fn a_chosen_herdr_app_that_isnt_answering_fails_without_falling_back() {
    let (r, fake, repo) = core_start_rig(crate::config::NewSessions::Auto);
    drop(fake);
    let req = StartRequest {
        backend: Some(crate::proto::StartBackend::Hc),
        ..core_req(Agent::Codex, &repo)
    };
    let e = r.sessions.start(req).await.unwrap_err();
    assert_eq!(e.code, ErrorCode::BackendUnavailable, "{e:?}");
    assert!(!e.message.contains("herdr-core"), "{e:?}");
    assert!(r.sessions.get("s1").is_none() && !r.dir.path().join("state/sessions/s1").exists());
}

#[tokio::test]
async fn a_chosen_herdr_terminal_overrides_the_runners_setting() {
    let dir = tempfile::tempdir().unwrap();
    let (r, _, _) = multi_rig(dir, vec![fixture("folder-access"), fixture("ready")]);
    r.sessions.set_core_starts(CoreStarts {
        new_sessions: crate::config::NewSessions::Hc,
        worktrees: "/nonexistent".into(),
        home: "/nonexistent".into(),
        bins: None,
    });
    let req = StartRequest {
        backend: Some(crate::proto::StartBackend::Herdr),
        ..start_req(Agent::Codex)
    };
    let started = r.sessions.start(req).await.unwrap();
    assert_eq!(started["herdr_session"], "p-s1");
}

#[tokio::test]
async fn start_options_follow_the_setting_and_whether_the_core_answers() {
    use crate::proto::StartBackend::{Hc, Herdr};
    let (r, fake, _) = core_start_rig(crate::config::NewSessions::Auto);
    let o = r.sessions.start_options().await;
    assert_eq!(o.default, Hc);
    assert!(o.backends.iter().all(|b| b.available), "{o:?}");
    drop(fake);
    let o = r.sessions.start_options().await;
    assert_eq!(o.default, Herdr);
    let hc = o.backends.iter().find(|b| b.backend == Hc).unwrap();
    assert!(!hc.available);
    assert_eq!(hc.reason.as_deref(), Some("not_answering"));

    let dir = tempfile::tempdir().unwrap();
    let (r, _, _) = multi_rig(dir, vec![]);
    r.sessions.set_core_starts(CoreStarts {
        new_sessions: crate::config::NewSessions::Hc,
        worktrees: "/nonexistent".into(),
        home: "/nonexistent".into(),
        bins: None,
    });
    let o = r.sessions.start_options().await;
    assert_eq!(o.default, Hc, "the setting's choice, though it would fail");
    let hc = o.backends.iter().find(|b| b.backend == Hc).unwrap();
    assert_eq!(hc.reason.as_deref(), Some("not_followed"));
}

// Renames, and a start into a space that exists.

fn doc_with(spaces: Value) -> Value {
    json!({ "spaces": spaces, "app_owned": { "keep": true } })
}

fn tab_of(id: &str, name: Value, terminals: &[&str]) -> Value {
    let panes: Vec<Value> = terminals
        .iter()
        .map(|t| json!({ "content": { "terminal": t } }))
        .collect();
    json!({ "id": id, "name": name, "root": { "children": panes } })
}

fn tabbed(tab: &str) -> Value {
    json!({ "argv": ["/x/claude"], "reported": true, "cwd": "/w",
            "status": { "state": "idle", "app": "claude-code" },
            "labels": { "herdr.space": "sp1", "herdr.tab": tab } })
}

fn rename_agent(agent: &str, name: &str) -> Op {
    Op::RenameAgent {
        agent: agent.into(),
        name: name.into(),
    }
}

#[tokio::test]
async fn a_herdr_app_agent_is_renamed_by_its_tab_and_only_when_it_owns_one() {
    let dir = tempfile::tempdir().unwrap();
    let (r, fake) = core_rig(dir);
    let (t1, t2, t3) = (
        fake.add(tabbed("tb1")),
        fake.add(tabbed("tb2")),
        fake.add(tabbed("tb2")),
    );
    let mut named = tabbed("tb4");
    named["name"] = json!("review");
    fake.add(named);
    let shell = fake.add(json!({ "argv": ["/bin/zsh"], "labels": { "herdr.tab": "tb5" } }));
    fake.set_doc(doc_with(json!([
        { "id": "sp1", "name": "repo", "color": "blue", "tabs": [
            tab_of("tb1", Value::Null, &[&t1]), tab_of("tb2", json!("pair"), &[&t2, &t3]),
            tab_of("tb4", json!("x"), &["t4"]), tab_of("tb5", json!("sh"), &[&shell]) ] },
    ])));
    let listed: Vec<crate::proto::AgentSummary> =
        serde_json::from_value(r.sessions.handle(Op::Agents).await.unwrap()).unwrap();
    let can: Vec<(&str, bool, Option<&str>)> = listed
        .iter()
        .map(|a| (a.name.as_str(), a.can_rename, a.space_id.as_deref()))
        .collect();
    assert_eq!(
        can,
        [
            ("hc/t1", true, Some("sp1")),
            ("hc/pair-t2", false, Some("sp1")),
            ("hc/pair-t3", false, Some("sp1")),
            ("hc/review", false, Some("sp1")),
        ]
    );

    let v = r
        .sessions
        .handle(rename_agent("hc/t1", "  fix login  "))
        .await
        .unwrap();
    assert_eq!(v["name"], "hc/fix login");
    let (doc, _) = fake.doc();
    assert_eq!(doc["spaces"][0]["tabs"][0]["name"], "fix login");
    assert_eq!(
        doc["spaces"][0]["color"], "blue",
        "the rest is kept as read"
    );
    assert_eq!(doc["app_owned"], json!({ "keep": true }));
    // The next listing has it, by the new name, without waiting for the core's event.
    r.sessions
        .handle(Op::ReadAgent {
            agent: "hc/fix login".into(),
            lines: 0,
            visible: true,
            ansi: false,
        })
        .await
        .unwrap();

    let writes = || requests_of(&fake, "doc.set").len();
    let before = writes();
    for (agent, name, code) in [
        ("hc/pair-t2", "solo", ErrorCode::BadRequest),
        ("hc/review", "other", ErrorCode::BadRequest),
        // A shell is no agent, whatever tab it sits in.
        ("hc/sh", "other", ErrorCode::NotFound),
        ("hc/t9", "other", ErrorCode::NotFound),
        ("vps/t1", "other", ErrorCode::NotFound),
        // Names another agent shows, alone or told apart by its address.
        ("hc/fix login", "review", ErrorCode::NameTaken),
        ("hc/fix login", "pair", ErrorCode::NameTaken),
        ("hc/fix login", "a/b", ErrorCode::BadRequest),
        ("hc/fix login", " ", ErrorCode::BadRequest),
    ] {
        let e = r
            .sessions
            .handle(rename_agent(agent, name))
            .await
            .unwrap_err();
        assert_eq!(e.code, code, "{agent} -> {name:?}: {e:?}");
    }
    assert_eq!(writes(), before, "a refusal writes nothing");
    let sent: Vec<String> = fake.methods();
    for typed in ["terminal.send", "terminal.keys", "terminal.label"] {
        assert!(
            !sent.contains(&typed.to_owned()),
            "a rename never does {typed}"
        );
    }
}

#[tokio::test]
async fn a_layout_someone_keeps_writing_is_retried_then_reported() {
    use crate::hc::fake::Script;
    let dir = tempfile::tempdir().unwrap();
    let (r, fake) = core_rig(dir);
    let t = fake.add(tabbed("tb1"));
    let e = r
        .sessions
        .handle(rename_agent("hc/t1", "x"))
        .await
        .unwrap_err();
    assert_eq!(e.code, ErrorCode::NotFound, "the app never ran here: {e:?}");

    fake.set_doc(doc_with(json!([
        { "id": "sp1", "name": "repo", "tabs": [ tab_of("tb1", Value::Null, &[&t]) ] },
    ])));
    for _ in 0..2 {
        fake.script("doc.set", Script::Error("conflict"));
    }
    r.sessions
        .handle(rename_agent("hc/t1", "third time"))
        .await
        .unwrap();
    assert_eq!(fake.doc().0["spaces"][0]["tabs"][0]["name"], "third time");

    for _ in 0..5 {
        fake.script("doc.set", Script::Error("conflict"));
    }
    let e = r
        .sessions
        .handle(rename_agent("hc/third time", "never"))
        .await
        .unwrap_err();
    assert_eq!(e.code, ErrorCode::Busy, "{e:?}");
    assert_eq!(fake.doc().0["spaces"][0]["tabs"][0]["name"], "third time");
}

#[tokio::test]
async fn a_herdr_app_space_is_renamed_by_id_and_never_another_machines() {
    let dir = tempfile::tempdir().unwrap();
    let (r, fake) = core_rig(dir);
    fake.add(tabbed("tb1"));
    let space = |id: &str, name: &str| Op::RenameSpace {
        herdr_session: Some("hc".into()),
        space_id: id.into(),
        name: name.into(),
    };
    let e = r.sessions.handle(space("sp1", "x")).await.unwrap_err();
    assert_eq!(e.code, ErrorCode::NotFound, "the app never ran here: {e:?}");

    fake.set_doc(doc_with(json!([
        { "id": "sp1", "name": "repo", "folder": "/r", "tabs": [] },
        { "id": "sp2", "name": "billing", "tabs": [] },
        { "id": "sp3", "name": "far", "machine": "vps", "tabs": [] },
    ])));
    let v = r.sessions.handle(space("sp1", " kraft ")).await.unwrap();
    assert_eq!(v["name"], "kraft");
    let (doc, _) = fake.doc();
    assert_eq!(doc["spaces"][0]["name"], "kraft");
    assert_eq!(doc["spaces"][0]["folder"], "/r");

    for (id, name, code) in [
        ("sp1", "billing", ErrorCode::NameTaken),
        ("sp3", "near", ErrorCode::NotFound),
        ("vps/sp1", "near", ErrorCode::NotFound),
        ("sp9", "near", ErrorCode::NotFound),
        ("sp2", "two\nlines", ErrorCode::BadRequest),
    ] {
        let e = r.sessions.handle(space(id, name)).await.unwrap_err();
        assert_eq!(e.code, code, "{id} -> {name:?}: {e:?}");
    }
    assert_eq!(fake.doc().0, doc, "a refusal writes nothing");
    // A Herdr session that isn't running is not looked for on disk.
    let e = r
        .sessions
        .handle(Op::RenameSpace {
            herdr_session: Some("../elsewhere".into()),
            space_id: "w1".into(),
            name: "x".into(),
        })
        .await
        .unwrap_err();
    assert_eq!(e.code, ErrorCode::NotFound);
}

#[tokio::test]
async fn a_herdr_terminal_agent_and_workspace_are_renamed_through_herdr() {
    let dir = tempfile::tempdir().unwrap();
    let (r, config_dir, _) = multi_rig(dir, vec![]);
    let folder = config_dir.join("sessions/main");
    std::fs::create_dir_all(&folder).unwrap();
    let herdr = FakeHerdr::start(&folder, json!({"type": "ok"}), vec![]);
    let mut agent = agent_info("idle");
    agent["name"] = json!("old");
    agent["agent"] = json!("claude");
    herdr.answer(
        "session.snapshot",
        json!({"type": "session_snapshot", "snapshot": {"version": "0.9.1", "protocol": 22,
            "workspaces": [
                {"workspace_id": "w2", "label": "kraft", "agent_status": "idle"},
                {"workspace_id": "w3", "label": "atlas", "agent_status": "idle"}],
            "tabs": [], "panes": [], "layouts": [], "agents": [agent.clone()]}}),
    );
    herdr.answer("agent.get", json!({"type": "agent_info", "agent": agent}));

    let listed: Vec<crate::proto::AgentSummary> =
        serde_json::from_value(r.sessions.handle(Op::Agents).await.unwrap()).unwrap();
    assert_eq!(listed[0].name, "main/old");
    assert_eq!(listed[0].space_id.as_deref(), Some("w2"));
    assert!(listed[0].can_rename);

    let v = r
        .sessions
        .handle(rename_agent("main/old", "reviewer"))
        .await
        .unwrap();
    assert_eq!(v["name"], "main/reviewer");
    // By pane, so an agent that goes by its pane id is renamed the same way.
    assert_eq!(
        herdr.call("agent.rename").unwrap()["params"],
        json!({"target": "w2:p1", "name": "reviewer"})
    );
    let e = r
        .sessions
        .handle(rename_agent("main/old", "My Agent"))
        .await
        .unwrap_err();
    assert_eq!(e.code, ErrorCode::BadRequest, "{e:?}");
    herdr.answer(
        "agent.rename",
        json!({"error": {"code": "agent_name_taken", "message": "agent name tester is already used"}}),
    );
    let e = r
        .sessions
        .handle(rename_agent("main/old", "tester"))
        .await
        .unwrap_err();
    assert_eq!(e.code, ErrorCode::NameTaken, "{e:?}");

    let space = |id: &str, name: &str| Op::RenameSpace {
        herdr_session: Some("main".into()),
        space_id: id.into(),
        name: name.into(),
    };
    let v = r.sessions.handle(space("w2", "billing")).await.unwrap();
    assert_eq!(v["name"], "billing");
    assert_eq!(
        herdr.call("workspace.rename").unwrap()["params"],
        json!({"workspace_id": "w2", "label": "billing"})
    );
    // Herdr would accept a label twice; the phone would then show two spaces as one.
    let e = r.sessions.handle(space("w2", "atlas")).await.unwrap_err();
    assert_eq!(e.code, ErrorCode::NameTaken);
    let e = r.sessions.handle(space("w9", "x")).await.unwrap_err();
    assert_eq!(e.code, ErrorCode::NotFound);
    let renames = herdr
        .methods()
        .iter()
        .filter(|m| *m == "workspace.rename")
        .count();
    assert_eq!(renames, 1);
    assert!(!herdr.methods().contains(&"agent.send_keys".to_owned()));
}

#[tokio::test]
async fn a_session_started_into_a_space_gets_a_tab_there_named_after_it() {
    let (r, fake, repo) = core_start_rig(crate::config::NewSessions::Herdr);
    // The app's side: the space exists, and it puts the new terminal in a tab of its own making.
    fake.set_doc(doc_with(json!([
        { "id": "sp1", "name": "billing", "tabs": [
            tab_of("tb1", json!("other"), &["t7"]), tab_of("tbnew", Value::Null, &["t1"]) ] },
    ])));
    fake.queue_screens("t1", vec![fixture("folder-access"), fixture("ready")]);
    report_idle(&fake, "codex");
    let req = StartRequest {
        backend: Some(crate::proto::StartBackend::Hc),
        space: Some("sp1".into()),
        ..core_req(Agent::Codex, &repo)
    };
    let started = r.sessions.start(req).await.unwrap();
    assert_eq!(started["workspace_id"], "sp1");
    let run = &requests_of(&fake, "terminal.run")[0];
    assert_eq!(run["labels"]["herdr.space"], "sp1");
    assert!(
        run["labels"].get("herdr.tab").is_none(),
        "the app makes the tab in a space that exists: {run}"
    );
    assert_eq!(run["labels"]["switchboard.session"], "s1");

    let named = || fake.doc().0["spaces"][0]["tabs"][1]["name"] == "p-s1";
    for _ in 0..100 {
        if named() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    let (doc, _) = fake.doc();
    assert_eq!(doc["spaces"][0]["tabs"][1]["name"], "p-s1");
    assert_eq!(doc["spaces"][0]["tabs"][0]["name"], "other");
    assert_eq!(
        doc["spaces"][0]["name"], "billing",
        "the space keeps its name"
    );

    // The hub's later name for the session reaches that same tab, and no other.
    let v = r
        .sessions
        .handle(Op::RenameSessionTab {
            session_id: "s1".into(),
            name: "login fix".into(),
        })
        .await
        .unwrap();
    assert_eq!(v["renamed"], true);
    let (doc, _) = fake.doc();
    assert_eq!(doc["spaces"][0]["tabs"][1]["name"], "login fix");
    assert_eq!(doc["spaces"][0]["tabs"][0]["name"], "other");
    // Its own agent keeps the name the runner reaches it by.
    let e = r
        .sessions
        .handle(rename_agent("hc/sb-s1", "x"))
        .await
        .unwrap_err();
    assert_eq!(e.code, ErrorCode::BadRequest, "{e:?}");
}

#[tokio::test]
async fn a_start_into_a_space_that_is_gone_or_elsewhere_creates_nothing() {
    let (r, fake, repo) = core_start_rig(crate::config::NewSessions::Auto);
    fake.set_doc(doc_with(json!([
        { "id": "sp1", "name": "billing", "tabs": [] },
        { "id": "sp3", "name": "far", "machine": "vps", "tabs": [] },
    ])));
    let into = |space: &str, backend| StartRequest {
        backend,
        space: Some(space.into()),
        ..core_req(Agent::Claude, &repo)
    };
    let hc = Some(crate::proto::StartBackend::Hc);
    for (req, code) in [
        (into("sp9", hc), ErrorCode::NotFound),
        (into("sp3", hc), ErrorCode::NotFound),
        (into("vps/sp1", hc), ErrorCode::NotFound),
        // A Herdr terminal has no such space.
        (
            into("sp1", Some(crate::proto::StartBackend::Herdr)),
            ErrorCode::BadRequest,
        ),
    ] {
        let e = r.sessions.start(req).await.unwrap_err();
        assert_eq!(e.code, code, "{e:?}");
    }
    assert!(requests_of(&fake, "terminal.run").is_empty());
    assert!(r.sessions.get("s1").is_none(), "no session record");
    assert!(!r.dir.path().join("worktrees").exists(), "no worktree");
    assert!(!r.dir.path().join("state/s1").exists(), "no secret");
}

#[tokio::test]
async fn a_herdr_terminal_session_has_no_tab_to_rename() {
    let dir = tempfile::tempdir().unwrap();
    let screens = vec![fixture("folder-access"), fixture("ready")];
    let (r, _, _) = multi_rig(dir, screens);
    r.sessions.start(start_req(Agent::Codex)).await.unwrap();
    let rename = |id: &str| Op::RenameSessionTab {
        session_id: id.into(),
        name: "login fix".into(),
    };
    let v = r.sessions.handle(rename("s1")).await.unwrap();
    assert_eq!(v["renamed"], false);
    let e = r.sessions.handle(rename("nope")).await.unwrap_err();
    assert_eq!(e.code, ErrorCode::NotFound);
}
