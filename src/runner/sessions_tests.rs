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
