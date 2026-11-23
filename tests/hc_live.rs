//! End-to-end runner checks with real Claude Code and Codex in a throwaway herdr-core. Skipped
//! unless SWITCHBOARD_LIVE_HC=1; set SWITCHBOARD_LIVE_AGENTS to `claude`, `codex` or both.
//! Each test starts its own core under a unique HC_SESSION, with every HC_ variable of the
//! caller's own core removed, then stops it and deletes its state folder. Worktrees go to a temp
//! folder. The Claude runs pre-trust a scratch repo in ~/.claude.json and remove that key after.
//! The rename test needs no agent and no account: SWITCHBOARD_LIVE_HC=1 alone runs it.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::Duration;

use futures_util::FutureExt;
use serde_json::{Value, json};
use switchboard::cli::Agent;
use switchboard::config::NewSessions;
use switchboard::hc::types::RunParams;
use switchboard::herdr::types::AgentStatus as Status;
use switchboard::proto::{AgentSummary, DialogView, Event, Op};
use switchboard::runner::core_watch::{self, Core};
use switchboard::runner::herdrs::Herdrs;
use switchboard::runner::hooks::HookHub;
use switchboard::runner::sessions::{CoreStarts, Sessions, SessionsConfig, StartRequest};
use tokio::net::TcpListener;
use tokio::sync::mpsc;
use tokio::time::Instant;

const TURN: Duration = Duration::from_secs(180);

/// Held for the whole of every test that edits ~/.claude.json, so they run one at a time.
static CLAUDE_JSON: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

fn enabled(agent: &str) -> bool {
    std::env::var("SWITCHBOARD_LIVE_HC").is_ok_and(|v| v == "1")
        && std::env::var("SWITCHBOARD_LIVE_AGENTS")
            .unwrap_or_default()
            .split(',')
            .any(|a| a.trim() == agent)
}

fn home() -> PathBuf {
    PathBuf::from(std::env::var("HOME").unwrap())
}

/// `hc` for the throwaway core only: none of the caller's core, Herdr or Claude variables.
fn hc(session: &str) -> Command {
    let mut cmd = Command::new("hc");
    for (k, _) in std::env::vars_os() {
        let k = k.to_string_lossy();
        if k.starts_with("HC_")
            || k.starts_with("HERDR_")
            || k.starts_with("CLAUDE_CODE")
            || k == "CLAUDECODE"
        {
            cmd.env_remove(&*k);
        }
    }
    cmd.env("HC_SESSION", session);
    cmd
}

struct Throwaway {
    session: String,
    socket: PathBuf,
}

impl Throwaway {
    async fn start() -> Self {
        static NEXT: AtomicU32 = AtomicU32::new(0);
        let n = NEXT.fetch_add(1, Ordering::Relaxed);
        let session = format!("sbt-{:04x}-{n}", std::process::id() & 0xffff);
        let socket = switchboard::hc::find_socket(
            |k| (k == "HC_SESSION").then(|| session.clone()),
            &home(),
            unsafe { libc::getuid() },
            Path::is_dir,
        );
        let out = hc(&session).args(["server", "status"]).output().unwrap();
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        let client = switchboard::hc::Client::new(&socket);
        let deadline = Instant::now() + Duration::from_secs(20);
        while client.hello().await.is_err() {
            assert!(
                Instant::now() < deadline,
                "the throwaway core never answered"
            );
            tokio::time::sleep(Duration::from_millis(200)).await;
        }
        Self { session, socket }
    }
}

impl Drop for Throwaway {
    fn drop(&mut self) {
        let _ = hc(&self.session).args(["server", "stop"]).output();
        // Its programs may still be writing as they exit.
        std::thread::sleep(Duration::from_secs(3));
        if let Some(dir) = self.socket.parent()
            && dir.file_name().is_some_and(|n| n == self.session.as_str())
        {
            let _ = std::fs::remove_dir_all(dir);
        }
    }
}

struct Live {
    sessions: Arc<Sessions>,
    events: mpsc::Receiver<Event>,
    core: Arc<Core>,
    _state: tempfile::TempDir,
    core_process: Throwaway,
}

async fn live(claude_config: PathBuf) -> Live {
    let core_process = Throwaway::start().await;
    let (tx, events) = mpsc::channel(256);
    let hooks = HookHub::new(tx.clone(), Duration::from_secs(120));
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let hook_addr = listener.local_addr().unwrap();
    tokio::spawn(hooks.clone().serve(listener));
    let state = tempfile::tempdir().unwrap();
    let sessions = Sessions::with_herdrs(
        Herdrs::single(state.path().join("no-herdr.sock")),
        hooks,
        tx,
        SessionsConfig {
            state_dir: state.path().join("state"),
            claude_config,
            claude_mode: switchboard::agents::claude::PermissionMode::AcceptEdits,
            claude_projects: home().join(".claude/projects"),
            codex_sessions: home().join(".codex/sessions"),
            exe: PathBuf::from(env!("CARGO_BIN_EXE_switchboard")),
            hook_addr,
            codex_version_cmd: vec!["codex".into(), "--version".into()],
            liveness_timeout: Duration::from_secs(30),
            startup_timeout: Duration::from_secs(30),
        },
    );
    let core = Core::new(&core_process.socket);
    sessions.set_core(core.clone());
    sessions.set_core_starts(CoreStarts {
        new_sessions: NewSessions::Hc,
        worktrees: state.path().join("worktrees"),
        home: home(),
        bins: None,
    });
    tokio::spawn(core_watch::follow(core.clone(), || {}, || {}));
    tokio::spawn(switchboard::runner::watch::watch_core(
        sessions.clone(),
        core.clone(),
    ));
    Live {
        sessions,
        events,
        core,
        _state: state,
        core_process,
    }
}

fn scratch_repo() -> (tempfile::TempDir, PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    let repo = std::fs::canonicalize(dir.path()).unwrap().join("repo");
    std::fs::create_dir_all(&repo).unwrap();
    let git = |args: &[&str]| {
        assert!(
            Command::new("git")
                .arg("-C")
                .arg(&repo)
                .args(args)
                .status()
                .unwrap()
                .success()
        )
    };
    git(&["init", "-q", "-b", "main"]);
    std::fs::write(repo.join("README.md"), "scratch\n").unwrap();
    git(&["add", "."]);
    git(&[
        "-c",
        "user.email=t@t",
        "-c",
        "user.name=t",
        "commit",
        "-qm",
        "init",
    ]);
    (dir, repo)
}

fn remove_trust_key(claude_json: &Path, repo: &Path) {
    let text = std::fs::read_to_string(claude_json).unwrap();
    let mut doc: Value = serde_json::from_str(&text).unwrap();
    if let Some(p) = doc.get_mut("projects").and_then(Value::as_object_mut) {
        p.retain(|k, _| !k.starts_with(&*repo.to_string_lossy()));
    }
    std::fs::write(claude_json, serde_json::to_vec_pretty(&doc).unwrap()).unwrap();
}

async fn wait_until(what: &str, f: impl Fn() -> bool) {
    let deadline = Instant::now() + TURN;
    while !f() {
        assert!(Instant::now() < deadline, "timed out waiting for {what}");
        tokio::time::sleep(Duration::from_millis(500)).await;
    }
}

impl Live {
    async fn approval(&mut self) -> (String, String) {
        let deadline = Instant::now() + TURN;
        loop {
            let left = deadline.saturating_duration_since(Instant::now());
            match tokio::time::timeout(left, self.events.recv()).await {
                Ok(Some(Event::ApprovalRequested {
                    approval_id,
                    tool_name,
                    ..
                })) => return (approval_id, tool_name),
                Ok(Some(_)) => {}
                _ => panic!("no approval request"),
            }
        }
    }

    async fn agents(&self) -> Vec<AgentSummary> {
        serde_json::from_value(self.sessions.handle(Op::Agents).await.unwrap()).unwrap()
    }

    async fn dialog(&self, agent: &str) -> Option<DialogView> {
        let v = self
            .sessions
            .handle(Op::Dialog {
                agent: agent.into(),
            })
            .await
            .ok()?;
        serde_json::from_value(v["dialog"].clone()).ok()
    }

    async fn screen(&self, agent: &str) -> String {
        self.sessions
            .handle(Op::ReadAgent {
                agent: agent.into(),
                lines: 0,
                visible: true,
                ansi: false,
            })
            .await
            .map(|v| v["text"].as_str().unwrap_or_default().to_owned())
            .unwrap_or_default()
    }

    /// Answers each dialog `agent` shows in turn, by the offered option with that label.
    async fn pick_each(&self, agent: &str, labels: &[&str]) {
        for label in labels {
            let deadline = Instant::now() + TURN;
            let (d, option) = loop {
                if let Some(d) = self.dialog(agent).await
                    && d.answerable
                    && let Some(o) = d.options.iter().find(|o| o.offered && o.label == *label)
                {
                    let o = o.clone();
                    break (d, o);
                }
                assert!(
                    Instant::now() < deadline,
                    "no {label}: {}",
                    self.screen(agent).await
                );
                tokio::time::sleep(Duration::from_secs(1)).await;
            };
            eprintln!("picking {label} on: {}", d.prompt.replace('\n', " / "));
            self.sessions
                .handle(Op::Answer {
                    agent: agent.into(),
                    occurrence: d.occurrence,
                    option: option.id,
                    label_sha256: option.label_sha256,
                    deny_only: false,
                })
                .await
                .unwrap();
        }
    }

    /// Shows the previous question of the group `agent` is at, once it may go back.
    async fn go_back(&self, agent: &str) {
        let deadline = Instant::now() + TURN;
        let d = loop {
            if let Some(d) = self.dialog(agent).await
                && d.back
            {
                break d;
            }
            assert!(
                Instant::now() < deadline,
                "no way back: {}",
                self.screen(agent).await
            );
            tokio::time::sleep(Duration::from_secs(1)).await;
        };
        eprintln!("going back from: {}", d.prompt.replace('\n', " / "));
        self.sessions
            .handle(Op::Back {
                agent: agent.into(),
                occurrence: d.occurrence,
            })
            .await
            .unwrap();
    }

    async fn wait_for_screen(&self, agent: &str, texts: &[&str]) {
        let deadline = Instant::now() + TURN;
        loop {
            let screen = self.screen(agent).await;
            if texts.iter().all(|t| screen.contains(t)) {
                return;
            }
            assert!(
                Instant::now() < deadline,
                "never showed {texts:?}: {screen}"
            );
            tokio::time::sleep(Duration::from_secs(1)).await;
        }
    }
}

const GROUP: &str = "three single-select questions in one call: 'Pick a size' (header 'Size') \
    options Small, Large; 'Pick a drink' (header 'Drink') options Tea, Coffee; 'Pick a colour' \
    (header 'Colour') options Red, Green, Blue. Do nothing else before it. Then reply with my \
    answers only.";

#[tokio::test(flavor = "multi_thread")]
async fn a_claude_session_starts_runs_and_stops_on_core() {
    if !enabled("claude") {
        eprintln!("skipped: SWITCHBOARD_LIVE_HC=1 with claude in SWITCHBOARD_LIVE_AGENTS");
        return;
    }
    let _claude_json = CLAUDE_JSON.lock().await;
    let claude_json = home().join(".claude.json");
    let mut l = live(claude_json.clone()).await;
    let (_repo_dir, repo) = scratch_repo();
    let id = format!("live-hc-{}", std::process::id());
    let outcome = std::panic::AssertUnwindSafe(async {
        let started = l
            .sessions
            .start(StartRequest {
                session_id: id.clone(),
                project_id: "live".into(),
                repo_path: repo.clone(),
                branch: format!("sb/{id}"),
                agent: Agent::Claude,
                prompt:
                    "Use your Bash tool to run exactly: touch made-on-core.txt . Then reply DONE."
                        .into(),
                deny_patterns: vec![],
                backend: None,
                space: None,
            })
            .await
            .unwrap();
        eprintln!("started: {started}");
        assert_eq!(started["herdr_session"], "hc");
        let worktree = PathBuf::from(started["worktree_path"].as_str().unwrap());

        // Listed beside other agents, with its session.
        let agents = l.agents().await;
        let mine = agents
            .iter()
            .find(|a| a.session_id.as_deref() == Some(id.as_str()))
            .expect("listed");
        assert_eq!(mine.herdr_session.as_deref(), Some("hc"));

        // The hook asks; an allow takes effect: a prompt and its result.
        let (approval, tool) = l.approval().await;
        assert_eq!(tool, "Bash");
        l.sessions
            .handle(Op::Decide {
                session_id: id.clone(),
                approval_id: approval,
                allow: true,
                choices: vec![],
            })
            .await
            .unwrap();
        wait_until("the allowed command", || {
            worktree.join("made-on-core.txt").exists()
        })
        .await;

        // Stop from the hub: the terminal ends.
        let address = started["pane_id"].as_str().unwrap().to_owned();
        let v = l
            .sessions
            .handle(Op::Stop {
                session_id: id.clone(),
                remove_worktree_if_clean: true,
            })
            .await
            .unwrap();
        eprintln!("stopped: {v}");
        assert_eq!(v["worktree_removed"], false, "it has a new file: kept");
        let gone = l.core.client().get(&address).await;
        assert!(gone.is_err(), "terminal removed: {gone:?}");
    })
    .catch_unwind()
    .await;
    // Claude writes its config as it exits, so it goes first.
    drop(l.core_process);
    remove_trust_key(&claude_json, &repo);
    if let Err(panic) = outcome {
        std::panic::resume_unwind(panic);
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn an_outside_claude_on_core_is_prompted_answered_denied_and_seen_to_end() {
    if !enabled("claude") {
        eprintln!("skipped: SWITCHBOARD_LIVE_HC=1 with claude in SWITCHBOARD_LIVE_AGENTS");
        return;
    }
    let _claude_json = CLAUDE_JSON.lock().await;
    let claude_json = home().join(".claude.json");
    let l = live(PathBuf::from("/nonexistent/claude.json")).await;
    let (_repo_dir, repo) = scratch_repo();
    // Trusted so Claude starts straight to its prompt.
    let mut doc: Value =
        serde_json::from_str(&std::fs::read_to_string(&claude_json).unwrap()).unwrap();
    doc["projects"][repo.to_string_lossy().as_ref()] = json!({ "hasTrustDialogAccepted": true });
    std::fs::write(&claude_json, serde_json::to_vec_pretty(&doc).unwrap()).unwrap();
    let outcome = std::panic::AssertUnwindSafe(async {
        let claude = switchboard::runner::find_bin("claude", &home()).unwrap();
        let t = l
            .core
            .client()
            .run(&RunParams {
                argv: vec![
                    claude.to_string_lossy().into_owned(),
                    "--permission-mode".into(),
                    "default".into(),
                ],
                cwd: repo.to_string_lossy().into_owned(),
                name: "outside".into(),
                no_parent: true,
                ..Default::default()
            })
            .await
            .unwrap();
        let agent = "hc/outside";
        wait_until("listed idle", || {
            l.core.facts().is_some_and(|f| {
                f.iter()
                    .any(|f| f.target == t.address && f.status == Status::Idle)
            })
        })
        .await;
        let names: Vec<String> = l.agents().await.into_iter().map(|a| a.name).collect();
        assert!(names.contains(&agent.to_owned()), "{names:?}");

        for (file, choose_yes) in [("yes.txt", true), ("no.txt", false)] {
            let sent = l
                .sessions
                .handle(Op::PromptAgent {
                    agent: agent.into(),
                    text: format!(
                        "Use your Bash tool to run exactly: touch {file} . Nothing else."
                    ),
                })
                .await
                .unwrap();
            assert_eq!(sent["outcome"], "sent", "{sent}");
            let deadline = Instant::now() + TURN;
            let d = loop {
                if let Some(d) = l.dialog(agent).await
                    && d.answerable
                {
                    break d;
                }
                assert!(Instant::now() < deadline, "{}", l.screen(agent).await);
                tokio::time::sleep(Duration::from_secs(1)).await;
            };
            let option = if choose_yes {
                &d.options[0]
            } else {
                d.options.iter().find(|o| o.is_deny).unwrap()
            };
            l.sessions
                .handle(Op::Answer {
                    agent: agent.into(),
                    occurrence: d.occurrence.clone(),
                    option: option.id.clone(),
                    label_sha256: option.label_sha256.clone(),
                    deny_only: !choose_yes,
                })
                .await
                .unwrap();
            if choose_yes {
                wait_until("the chosen command", || repo.join(file).exists()).await;
            } else {
                tokio::time::sleep(Duration::from_secs(5)).await;
                assert!(!repo.join(file).exists(), "denied");
            }
            // A turn the app hasn't looked at ends `done`.
            wait_until("the turn's end", || {
                l.core.facts().is_some_and(|f| {
                    f.iter().any(|f| {
                        f.target == t.address && matches!(f.status, Status::Idle | Status::Done)
                    })
                })
            })
            .await;
        }

        // A kill is seen: it leaves the listing.
        l.core.client().kill(&t.address).await.unwrap();
        wait_until("gone from the listing", || {
            l.core
                .facts()
                .is_some_and(|f| !f.iter().any(|f| f.target == t.address))
        })
        .await;
    })
    .catch_unwind()
    .await;
    // Claude writes its config as it exits, so it goes first.
    drop(l.core_process);
    remove_trust_key(&claude_json, &repo);
    if let Err(panic) = outcome {
        std::panic::resume_unwind(panic);
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn an_outside_claude_on_core_has_a_group_of_questions_answered_one_by_one() {
    if !enabled("claude") {
        eprintln!("skipped: SWITCHBOARD_LIVE_HC=1 with claude in SWITCHBOARD_LIVE_AGENTS");
        return;
    }
    let _claude_json = CLAUDE_JSON.lock().await;
    let claude_json = home().join(".claude.json");
    let l = live(PathBuf::from("/nonexistent/claude.json")).await;
    let (_repo_dir, repo) = scratch_repo();
    let mut doc: Value =
        serde_json::from_str(&std::fs::read_to_string(&claude_json).unwrap()).unwrap();
    doc["projects"][repo.to_string_lossy().as_ref()] = json!({ "hasTrustDialogAccepted": true });
    std::fs::write(&claude_json, serde_json::to_vec_pretty(&doc).unwrap()).unwrap();
    let outcome = std::panic::AssertUnwindSafe(async {
        let claude = switchboard::runner::find_bin("claude", &home()).unwrap();
        let t = l
            .core
            .client()
            .run(&RunParams {
                argv: vec![
                    claude.to_string_lossy().into_owned(),
                    "--permission-mode".into(),
                    "default".into(),
                ],
                cwd: repo.to_string_lossy().into_owned(),
                name: "asker".into(),
                no_parent: true,
                ..Default::default()
            })
            .await
            .unwrap();
        let agent = "hc/asker";
        wait_until("listed idle", || {
            l.core.facts().is_some_and(|f| {
                f.iter()
                    .any(|f| f.target == t.address && f.status == Status::Idle)
            })
        })
        .await;
        let sent = l
            .sessions
            .handle(Op::PromptAgent {
                agent: agent.into(),
                text: format!("Use the AskUserQuestion tool once with {GROUP}"),
            })
            .await
            .unwrap();
        assert_eq!(sent["outcome"], "sent", "{sent}");
        l.pick_each(agent, &["Large", "Tea", "Blue"]).await;
        // From the review, back to the last question to change its answer.
        l.go_back(agent).await;
        l.pick_each(agent, &["Green", "Submit answers"]).await;
        l.wait_for_screen(
            agent,
            &[
                "Pick a size → Large",
                "Pick a drink → Tea",
                "Pick a colour → Green",
            ],
        )
        .await;
        l.core.client().kill(&t.address).await.unwrap();
    })
    .catch_unwind()
    .await;
    drop(l.core_process);
    remove_trust_key(&claude_json, &repo);
    if let Err(panic) = outcome {
        std::panic::resume_unwind(panic);
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn an_outside_codex_on_core_has_a_group_of_questions_answered_one_by_one() {
    if !enabled("codex") {
        eprintln!("skipped: SWITCHBOARD_LIVE_HC=1 with codex in SWITCHBOARD_LIVE_AGENTS");
        return;
    }
    let l = live(PathBuf::from("/nonexistent/claude.json")).await;
    let (_repo_dir, repo) = scratch_repo();
    let outcome = std::panic::AssertUnwindSafe(async {
        let codex = switchboard::runner::find_bin("codex", &home()).unwrap();
        let t = l
            .core
            .client()
            .run(&RunParams {
                argv: vec![
                    codex.to_string_lossy().into_owned(),
                    "--no-daemon".into(),
                    "-s".into(),
                    "read-only".into(),
                    "-a".into(),
                    "on-request".into(),
                    // Trusted for this run only, so nothing is saved to Codex's config.
                    "-c".into(),
                    format!(
                        "projects={{{:?}={{trust_level=\"trusted\"}}}}",
                        repo.to_string_lossy()
                    ),
                ],
                cwd: repo.to_string_lossy().into_owned(),
                name: "asker".into(),
                no_parent: true,
                ..Default::default()
            })
            .await
            .unwrap();
        let agent = "hc/asker";
        wait_until("listed idle", || {
            l.core.facts().is_some_and(|f| {
                f.iter()
                    .any(|f| f.target == t.address && f.status == Status::Idle)
            })
        })
        .await;
        // Codex only has its question dialog in Plan mode.
        for text in [
            "/plan".to_owned(),
            format!("Use the request_user_input tool once with {GROUP}"),
        ] {
            // The core refuses a send while Codex's animated start screen is still settling.
            let deadline = Instant::now() + TURN;
            let sent = loop {
                let op = Op::PromptAgent {
                    agent: agent.into(),
                    text: text.clone(),
                };
                match l.sessions.handle(op).await {
                    Ok(v) => break v,
                    Err(e) if e.message.contains("still starting") => {
                        assert!(Instant::now() < deadline, "{e:?}");
                        tokio::time::sleep(Duration::from_secs(2)).await;
                    }
                    Err(e) => panic!("{e:?}"),
                }
            };
            assert_eq!(sent["outcome"], "sent", "{sent}");
            tokio::time::sleep(Duration::from_secs(3)).await;
        }
        l.pick_each(agent, &["Large", "Tea"]).await;
        // From the last question, back to the second to change its answer.
        l.go_back(agent).await;
        l.pick_each(agent, &["Coffee", "Blue"]).await;
        l.wait_for_screen(
            agent,
            &[
                "Questions 3/3 answered",
                "answer: Large",
                "answer: Coffee",
                "answer: Blue",
            ],
        )
        .await;
        l.core.client().kill(&t.address).await.unwrap();
    })
    .catch_unwind()
    .await;
    drop(l.core_process);
    if let Err(panic) = outcome {
        std::panic::resume_unwind(panic);
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn a_codex_session_starts_on_core_and_its_denied_command_never_runs() {
    if !enabled("codex") {
        eprintln!("skipped: SWITCHBOARD_LIVE_HC=1 with codex in SWITCHBOARD_LIVE_AGENTS");
        return;
    }
    let mut l = live(PathBuf::from("/nonexistent/claude.json")).await;
    let (_repo_dir, repo) = scratch_repo();
    let id = format!("live-hc-codex-{}", std::process::id());
    let outcome = std::panic::AssertUnwindSafe(async {
        let started = l
            .sessions
            .start(StartRequest {
                session_id: id.clone(),
                project_id: "live".into(),
                repo_path: repo.clone(),
                branch: format!("sb/{id}"),
                agent: Agent::Codex,
                prompt: "Run exactly this shell command: touch ~/.cache/sb-hc-live.txt . If the sandbox blocks it, request escalated permission for exactly that command. Do nothing else."
                    .into(),
                deny_patterns: vec![],
                backend: None,
                space: None,
            })
            .await
            .unwrap();
        eprintln!("started: {started}");
        let (approval, _) = l.approval().await;
        l.sessions
            .handle(Op::Decide {
                session_id: id.clone(),
                approval_id: approval,
                allow: false,
                choices: vec![],
            })
            .await
            .unwrap();
        tokio::time::sleep(Duration::from_secs(10)).await;
        assert!(!home().join(".cache/sb-hc-live.txt").exists());
        l.sessions
            .handle(Op::Stop {
                session_id: id.clone(),
                remove_worktree_if_clean: true,
            })
            .await
            .unwrap();
    })
    .catch_unwind()
    .await;
    let _ = std::fs::remove_file(home().join(".cache/sb-hc-live.txt"));
    drop(l.core_process);
    if let Err(panic) = outcome {
        std::panic::resume_unwind(panic);
    }
}

/// The renames and the start label against a real core's `doc` and `terminal.run`. A program
/// called `claude` that only sleeps stands in for an agent, so no CLI or account is used. It
/// can't show the app adopting a terminal: a throwaway core has no window.
#[tokio::test]
async fn renames_are_layout_writes_a_real_core_takes() {
    if !std::env::var("SWITCHBOARD_LIVE_HC").is_ok_and(|v| v == "1") {
        eprintln!("skipped: SWITCHBOARD_LIVE_HC=1");
        return;
    }
    use std::os::unix::fs::PermissionsExt;
    let scratch = tempfile::tempdir().unwrap();
    let bin = scratch.path().join("claude");
    std::fs::write(&bin, "#!/bin/sh\nexec sleep 600\n").unwrap();
    std::fs::set_permissions(&bin, std::fs::Permissions::from_mode(0o755)).unwrap();
    let l = live(scratch.path().join("claude.json")).await;
    let client = l.core.client();
    let run = |labels: Value| {
        let (client, bin, cwd) = (client.clone(), bin.clone(), scratch.path().to_owned());
        async move {
            client
                .call(
                    "terminal.run",
                    json!({ "argv": [bin], "cwd": cwd, "labels": labels, "no_parent": true }),
                )
                .await
                .unwrap()
        }
    };
    let t = run(json!({ "herdr.space": "sp1", "herdr.tab": "tb1" })).await;
    let address = t["address"].as_str().unwrap().to_owned();
    assert!(
        t["name"].is_null(),
        "unnamed, as the app's own terminals are"
    );

    // The app's side: a layout holding that terminal in a tab of its own.
    let layout = "herdr-app";
    let doc = json!({ "spaces": [
        { "id": "sp1", "name": "repo", "tabs": [
            { "id": "tb1", "name": null, "root": { "content": { "terminal": address } } } ] },
        { "id": "sp2", "name": "billing", "tabs": [] },
    ], "app_owned": { "keep": true } });
    let v0 = client.doc_get(layout).await.unwrap().version;
    client.doc_set(layout, &doc, v0).await.unwrap();
    // A stale write is refused the way the retry expects.
    let stale = client.doc_set(layout, &doc, v0).await.unwrap_err();
    assert_eq!(stale.kind(), Some("conflict"), "{stale:?}");
    wait_until("listed in its space, renamable", || {
        l.core.facts().is_some_and(|f| {
            f.iter().any(|f| {
                f.target == address && f.space_id.as_deref() == Some("sp1") && f.can_rename
            })
        })
    })
    .await;

    let v = l
        .sessions
        .handle(Op::RenameAgent {
            agent: format!("hc/{address}"),
            name: "fix login".into(),
        })
        .await
        .unwrap();
    assert_eq!(v["name"], "hc/fix login");
    let space = |name: &str| Op::RenameSpace {
        herdr_session: Some("hc".into()),
        space_id: "sp1".into(),
        name: name.into(),
    };
    let taken = l.sessions.handle(space("billing")).await.unwrap_err();
    assert_eq!(
        taken.code,
        switchboard::proto::ErrorCode::NameTaken,
        "{taken:?}"
    );
    l.sessions.handle(space("kraft")).await.unwrap();
    let now = client.doc_get(layout).await.unwrap().value;
    assert_eq!(now["spaces"][0]["tabs"][0]["name"], "fix login");
    assert_eq!(now["spaces"][0]["name"], "kraft");
    assert_eq!(now["spaces"][1]["name"], "billing");
    assert_eq!(
        now["app_owned"],
        json!({ "keep": true }),
        "the rest is kept"
    );
    let listed = l.agents().await;
    let me = listed.iter().find(|a| a.pane_id == address).unwrap();
    assert_eq!(
        (me.name.as_str(), me.space.as_deref()),
        ("hc/fix login", Some("kraft"))
    );

    // What a start into an existing space sends: the space's label and no tab. The core takes it.
    let second = run(json!({ "herdr.space": "sp1", "herdr.seen": "0" })).await;
    assert_eq!(second["labels"]["herdr.space"], "sp1");
    assert!(second["labels"].get("herdr.tab").is_none());
    drop(l.core_process);
}
