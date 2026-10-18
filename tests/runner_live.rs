//! End-to-end runner checks with real Claude Code and Codex inside a named Herdr test session.
//! Skipped unless SWITCHBOARD_LIVE_HERDR_SOCKET names a test session's socket; set
//! SWITCHBOARD_LIVE_AGENTS to `codex`, `claude` or `codex,claude`.
//! The Claude run pre-trusts a scratch repo in ~/.claude.json and removes that key afterwards.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;
use std::time::Duration;

use futures_util::FutureExt;
use serde_json::Value;
use switchboard::cli::Agent;
use switchboard::herdr::Client;
use switchboard::proto::Event;
use switchboard::runner::hooks::HookHub;
use switchboard::runner::sessions::{Sessions, SessionsConfig, StartRequest};
use switchboard::runner::watch::watch;
use tokio::net::TcpListener;
use tokio::sync::mpsc;
use tokio::time::Instant;

const TURN: Duration = Duration::from_secs(180);

struct Live {
    sessions: Arc<Sessions>,
    events: mpsc::Receiver<Event>,
    _state: tempfile::TempDir,
}

async fn live(claude_config: PathBuf) -> Option<Live> {
    let socket = std::env::var_os("SWITCHBOARD_LIVE_HERDR_SOCKET")?;
    let (tx, events) = mpsc::channel(256);
    let hooks = HookHub::new(tx.clone(), Duration::from_secs(120));
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let hook_addr = listener.local_addr().unwrap();
    tokio::spawn(hooks.clone().serve(listener));
    let state = tempfile::tempdir().unwrap();
    let sessions = Sessions::new(
        Client::new(socket).with_timeout(Duration::from_secs(30)),
        hooks,
        tx,
        SessionsConfig {
            state_dir: state.path().to_owned(),
            claude_config,
            exe: PathBuf::from(env!("CARGO_BIN_EXE_switchboard")),
            hook_addr,
            codex_version_cmd: vec!["codex".into(), "--version".into()],
            liveness_timeout: Duration::from_secs(30),
            startup_timeout: Duration::from_secs(30),
        },
    );
    tokio::spawn(watch(sessions.clone()));
    Some(Live {
        sessions,
        events,
        _state: state,
    })
}

fn enabled(agent: &str) -> bool {
    std::env::var("SWITCHBOARD_LIVE_AGENTS").is_ok_and(|v| v.split(',').any(|a| a.trim() == agent))
}

impl Live {
    /// Waits for the first event matching `pick`, logging every event seen.
    async fn wait_for<T>(&mut self, what: &str, mut pick: impl FnMut(&Event) -> Option<T>) -> T {
        let deadline = Instant::now() + TURN;
        loop {
            let left = deadline.saturating_duration_since(Instant::now());
            let ev = tokio::time::timeout(left, self.events.recv())
                .await
                .unwrap_or_else(|_| panic!("timed out waiting for {what}"))
                .expect("event channel open");
            eprintln!("event: {}", serde_json::to_string(&ev).unwrap());
            if let Some(t) = pick(&ev) {
                return t;
            }
        }
    }

    async fn approval(&mut self) -> (String, String) {
        self.wait_for("approval", |e| match e {
            Event::ApprovalRequested {
                approval_id,
                tool_name,
                ..
            } => Some((approval_id.clone(), tool_name.clone())),
            _ => None,
        })
        .await
    }

    async fn audit(&mut self, kind: &str) -> Value {
        self.wait_for(kind, |e| match e {
            Event::Audit {
                kind: k, detail, ..
            } if k == kind => Some(detail.clone()),
            _ => None,
        })
        .await
    }

    async fn prompt(&self, session_id: &str, text: &str) {
        let op = switchboard::proto::Op::Prompt {
            session_id: session_id.into(),
            text: text.into(),
        };
        self.sessions.handle(op).await.unwrap();
    }

    async fn screen(&self, session_id: &str) -> String {
        let op = switchboard::proto::Op::Read {
            session_id: session_id.into(),
            lines: 40,
        };
        let v = self.sessions.handle(op).await.unwrap();
        v["text"].as_str().unwrap_or_default().to_owned()
    }
}

fn scratch_repo() -> (tempfile::TempDir, PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    let repo = dir.path().canonicalize().unwrap();
    git(&repo, &["init", "-q", "-b", "main"]);
    std::fs::write(repo.join("README.md"), "hi\n").unwrap();
    git(&repo, &["add", "README.md"]);
    git(
        &repo,
        &[
            "-c",
            "user.name=sb",
            "-c",
            "user.email=sb@test",
            "commit",
            "-q",
            "-m",
            "init",
        ],
    );
    (dir, repo)
}

fn git(dir: &Path, args: &[&str]) {
    assert!(
        Command::new("git")
            .arg("-C")
            .arg(dir)
            .args(args)
            .status()
            .unwrap()
            .success(),
        "git {args:?}"
    );
}

/// Kills the session's hook helper with SIGKILL while it waits for a decision. Anchored to the
/// binary path, because the agent's own command line also contains the hook command.
fn kill_helper(secret_path: &Path) {
    let pattern = format!(
        "^{} hook --agent [a-z]+ --secret-file {}",
        env!("CARGO_BIN_EXE_switchboard"),
        secret_path.display()
    );
    let out = Command::new("pkill")
        .args(["-9", "-f", &pattern])
        .status()
        .unwrap();
    assert!(out.success(), "a helper process was running");
}

async fn wait_until(what: &str, mut cond: impl FnMut() -> bool) {
    let deadline = Instant::now() + TURN;
    while !cond() {
        assert!(Instant::now() < deadline, "timed out waiting for {what}");
        tokio::time::sleep(Duration::from_millis(500)).await;
    }
}

async fn stop_and_clean(l: &Live, session_id: &str, repo: &Path) {
    let worktree = l.sessions.get(session_id).and_then(|s| s.worktree_path);
    let op = switchboard::proto::Op::Stop {
        session_id: session_id.into(),
        remove_worktree_if_clean: false,
    };
    let _ = l.sessions.handle(op).await;
    if let Some(wt) = worktree {
        let _ = Command::new("git")
            .arg("-C")
            .arg(repo)
            .args(["worktree", "remove", "--force", &wt])
            .status();
        if let Some(parent) = Path::new(&wt).parent() {
            let _ = std::fs::remove_dir(parent);
        }
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn codex_restricted_start_decision_and_keystroke_fallback() {
    if !enabled("codex") {
        eprintln!("skipped: codex not in SWITCHBOARD_LIVE_AGENTS");
        return;
    }
    let Some(mut l) = live(PathBuf::from("/nonexistent/claude.json")).await else {
        eprintln!("skipped: SWITCHBOARD_LIVE_HERDR_SOCKET not set");
        return;
    };
    let (_repo_dir, repo) = scratch_repo();
    let id = format!("live-codex-{}", std::process::id());
    let probe = PathBuf::from(std::env::var("HOME").unwrap())
        .join(format!(".cache/switchboard-live-{}", std::process::id()));
    std::fs::create_dir_all(&probe).unwrap();
    let ask = |name: &str| {
        format!(
            "Run exactly this shell command: touch {}/{name} . It writes outside the workspace, so request \
             escalation if the sandbox blocks it. Run nothing else. If it is denied, reply DENIED and stop.",
            probe.display()
        )
    };

    let body = std::panic::AssertUnwindSafe(codex_body(&mut l, &id, &repo, &probe, &ask))
        .catch_unwind()
        .await;
    stop_and_clean(&l, &id, &repo).await;
    let _ = std::fs::remove_dir_all(&probe);
    if let Err(panic) = body {
        std::panic::resume_unwind(panic);
    }
}

async fn codex_body(
    l: &mut Live,
    id: &str,
    repo: &Path,
    probe: &Path,
    ask: &dyn Fn(&str) -> String,
) {
    let id = id.to_owned();
    let repo = repo.to_owned();
    let started = l
        .sessions
        .start(StartRequest {
            session_id: id.clone(),
            project_id: "live".into(),
            repo_path: repo.clone(),
            branch: format!("sb/{id}"),
            agent: Agent::Codex,
            prompt: ask("decided.txt"),
            deny_patterns: vec![],
        })
        .await
        .unwrap();
    eprintln!("started: {started}");
    l.audit("startup_cleared").await;

    // 1. The hook reaches the runner and a deny decision is honoured.
    let (approval, tool) = l.approval().await;
    assert_eq!(tool, "Bash");
    l.sessions
        .handle(switchboard::proto::Op::Decide {
            session_id: id.clone(),
            approval_id: approval,
            allow: false,
        })
        .await
        .unwrap();
    tokio::time::sleep(Duration::from_secs(8)).await;
    assert!(
        !probe.join("decided.txt").exists(),
        "denied command must not run"
    );
    eprintln!("screen after decision:\n{}", l.screen(&id).await);

    // 2. The helper is killed mid-approval: Codex falls through to its dialog and the fallback denies it.
    l.prompt(&id, &ask("fallback.txt")).await;
    let (_approval, _) = l.approval().await;
    let secret = l
        ._state
        .path()
        .join("sessions")
        .join(&id)
        .join("secret.json");
    kill_helper(&secret);
    let closed = l
        .wait_for("approval closed", |e| match e {
            Event::ApprovalClosed { reason, .. } => Some(reason.clone()),
            _ => None,
        })
        .await;
    assert_eq!(closed, "hook_helper_gone");
    let detail = l.audit("fallback_deny").await;
    let excerpt = detail["screen_excerpt"].as_str().unwrap();
    assert!(
        excerpt.contains("3. No, and tell Codex what to do differently (esc)"),
        "{excerpt}"
    );
    tokio::time::sleep(Duration::from_secs(5)).await;
    assert!(
        !probe.join("fallback.txt").exists(),
        "fallback-denied command must not run"
    );
    eprintln!("screen after fallback:\n{}", l.screen(&id).await);
}

#[tokio::test(flavor = "multi_thread")]
async fn claude_liveness_allow_and_fail_closed_on_killed_helper() {
    if !enabled("claude") {
        eprintln!("skipped: claude not in SWITCHBOARD_LIVE_AGENTS");
        return;
    }
    let claude_json = PathBuf::from(std::env::var("HOME").unwrap()).join(".claude.json");
    let Some(mut l) = live(claude_json.clone()).await else {
        eprintln!("skipped: SWITCHBOARD_LIVE_HERDR_SOCKET not set");
        return;
    };
    let (_repo_dir, repo) = scratch_repo();
    let id = format!("live-claude-{}", std::process::id());
    let ask = |name: &str| {
        format!(
            "Use your Bash tool to run exactly this command: touch {name} . Run nothing else. If it is denied, reply DENIED and stop."
        )
    };

    let result = l
        .sessions
        .start(StartRequest {
            session_id: id.clone(),
            project_id: "live".into(),
            repo_path: repo.clone(),
            branch: format!("sb/{id}"),
            agent: Agent::Claude,
            prompt: ask("allowed.txt"),
            deny_patterns: vec![],
        })
        .await;
    let outcome = std::panic::AssertUnwindSafe(async {
        let started = result.unwrap();
        eprintln!("started: {started}");
        let worktree = PathBuf::from(started["worktree_path"].as_str().unwrap());

        // 1. Liveness was proven before the prompt; an allow decision takes effect.
        let (approval, tool) = l.approval().await;
        assert_eq!(tool, "Bash");
        l.sessions
            .handle(switchboard::proto::Op::Decide {
                session_id: id.clone(),
                approval_id: approval,
                allow: true,
            })
            .await
            .unwrap();
        wait_until("allowed.txt", || worktree.join("allowed.txt").exists()).await;

        // 2. Killed helper: onFailure=block denies, and the runner never sends keys to Claude.
        tokio::time::sleep(Duration::from_secs(5)).await;
        l.prompt(&id, &ask("killed.txt")).await;
        let _ = l.approval().await;
        kill_helper(
            &l._state
                .path()
                .join("sessions")
                .join(&id)
                .join("secret.json"),
        );
        tokio::time::sleep(Duration::from_secs(15)).await;
        let screen = l.screen(&id).await;
        eprintln!("screen after killed helper:\n{screen}");
        assert!(
            !worktree.join("killed.txt").exists(),
            "tool must not run after a killed helper"
        );
        assert!(
            screen.contains("Denied by PermissionRequest hook") || screen.contains("DENIED"),
            "{screen}"
        );
        while let Ok(ev) = l.events.try_recv() {
            assert!(
                !matches!(&ev, Event::Audit { kind, .. } if kind == "fallback_deny"),
                "no keys for Claude"
            );
        }
    })
    .catch_unwind()
    .await;
    stop_and_clean(&l, &id, &repo).await;
    remove_trust_key(&claude_json, &repo);
    if let Err(panic) = outcome {
        std::panic::resume_unwind(panic);
    }
}

/// Removes only the scratch repo's key that the pre-trust step added.
fn remove_trust_key(claude_json: &Path, repo: &Path) {
    let text = std::fs::read_to_string(claude_json).unwrap();
    let mut doc: Value = serde_json::from_str(&text).unwrap();
    if let Some(p) = doc.get_mut("projects").and_then(Value::as_object_mut) {
        p.remove(&repo.to_string_lossy().into_owned());
    }
    std::fs::write(claude_json, serde_json::to_vec_pretty(&doc).unwrap()).unwrap();
}
