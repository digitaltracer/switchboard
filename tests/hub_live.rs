//! The hub end to end: real runner, a named Herdr test session and real Claude Code, all driven
//! through the session service over a real WebSocket. Skipped unless SWITCHBOARD_LIVE_HERDR_SOCKET
//! names a test session's socket and SWITCHBOARD_LIVE_AGENTS includes `claude`.
//! Pre-trusts a scratch repo in ~/.claude.json and removes that key afterwards.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;
use std::time::Duration;

use futures_util::FutureExt;
use serde_json::Value;
use switchboard::cli::Agent;
use switchboard::config::HubConfig;
use switchboard::herdr::Client;
use switchboard::hub::runners::Readiness;
use switchboard::hub::service::{Caller, Clock, SessionService, SessionView, StartRequest};
use switchboard::hub::{Hub, serve};
use switchboard::proto::auth::hash_token;
use switchboard::runner::hooks::HookHub;
use switchboard::runner::link::{self, LinkConfig};
use switchboard::runner::sessions::{Sessions, SessionsConfig};
use switchboard::runner::watch::watch;
use switchboard::store::{Decision, Delivery, Lifecycle, Store};
use tokio::net::TcpListener;
use tokio::sync::mpsc;
use tokio::time::Instant;
use url::Url;

const TURN: Duration = Duration::from_secs(180);
const TOKEN: &str = "live-mac-token";

fn enabled() -> bool {
    std::env::var("SWITCHBOARD_LIVE_AGENTS")
        .is_ok_and(|v| v.split(',').any(|a| a.trim() == "claude"))
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
    let ok = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .status()
        .unwrap()
        .success();
    assert!(ok, "git {args:?}");
}

async fn hub(db: &Path, repo: &Path) -> Hub {
    let cfg = HubConfig::parse(&format!(
        r#"
        public_url = "https://hub.example.com"
        listen = "127.0.0.1:0"
        database = "{}"
        [auth]
        passphrase_hash = "$argon2id$v=19$m=19456,t=2,p=1$c2FsdA$aGFzaA"
        [[machines]]
        id = "mac"
        token_hash = "{}"
        [[projects]]
        id = "live"
        description = "scratch repo for the live hub test"
        repos = {{ mac = "{}" }}
        "#,
        db.display(),
        hash_token(TOKEN),
        repo.display()
    ))
    .unwrap();
    let svc = SessionService::new(cfg, Store::open(db).await.unwrap(), Clock::System);
    svc.recover().await.unwrap();
    serve(svc, TcpListener::bind("127.0.0.1:0").await.unwrap()).unwrap()
}

async fn runner(socket: &str, hub_addr: std::net::SocketAddr, state: &Path, claude_json: &Path) {
    let (tx, events) = mpsc::channel(256);
    let hooks = HookHub::new(tx.clone(), Duration::from_secs(120));
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let hook_addr = listener.local_addr().unwrap();
    tokio::spawn(hooks.clone().serve(listener));
    let sessions = Sessions::new(
        Client::new(socket).with_timeout(Duration::from_secs(30)),
        hooks,
        tx,
        SessionsConfig {
            state_dir: state.to_owned(),
            claude_config: claude_json.to_owned(),
            // Live runs exercise the approval path, which auto mode rarely takes.
            claude_mode: switchboard::agents::claude::PermissionMode::AcceptEdits,
            exe: PathBuf::from(env!("CARGO_BIN_EXE_switchboard")),
            hook_addr,
            codex_version_cmd: vec!["codex".into(), "--version".into()],
            liveness_timeout: Duration::from_secs(30),
            startup_timeout: Duration::from_secs(30),
        },
    );
    tokio::spawn(watch(sessions.clone()));
    let cfg = LinkConfig {
        url: Url::parse(&format!("ws://{hub_addr}/runner")).unwrap(),
        token: TOKEN.into(),
        machine_id: "mac".into(),
        herdr_version: None,
    };
    tokio::spawn(link::run(cfg, sessions, events));
}

/// Long-polls the session like a bot would, until `pick` accepts a view.
async fn poll_until<T>(
    svc: &Arc<SessionService>,
    id: &str,
    what: &str,
    seen: &mut Vec<String>,
    mut pick: impl FnMut(&SessionView) -> Option<T>,
) -> T {
    let deadline = Instant::now() + TURN;
    let mut revision = None;
    loop {
        assert!(Instant::now() < deadline, "timed out waiting for {what}");
        let v = svc
            .get(id, revision, Duration::from_secs(25))
            .await
            .unwrap();
        revision = Some(v.revision);
        if seen.last() != Some(&v.agent_status) {
            eprintln!(
                "rev {} lifecycle {:?} status {}",
                v.revision, v.lifecycle, v.agent_status
            );
            seen.push(v.agent_status.clone());
        }
        if let Some(t) = pick(&v) {
            return t;
        }
    }
}

fn pending(v: &SessionView) -> Option<String> {
    v.pending_approvals.first().map(|a| {
        eprintln!("pending approval {} {} {}", a.id, a.tool_name, a.tool_input);
        a.id.clone()
    })
}

#[tokio::test(flavor = "multi_thread")]
async fn claude_through_the_hub_allow_deny_prompt_and_stop() {
    if !enabled() {
        eprintln!("skipped: claude not in SWITCHBOARD_LIVE_AGENTS");
        return;
    }
    let Ok(socket) = std::env::var("SWITCHBOARD_LIVE_HERDR_SOCKET") else {
        eprintln!("skipped: SWITCHBOARD_LIVE_HERDR_SOCKET not set");
        return;
    };
    let claude_json = PathBuf::from(std::env::var("HOME").unwrap()).join(".claude.json");
    let (_repo_dir, repo) = scratch_repo();
    let work = tempfile::tempdir().unwrap();
    let hub = hub(&work.path().join("hub.db"), &repo).await;
    runner(&socket, hub.addr, &work.path().join("runner"), &claude_json).await;
    let svc = hub.service.clone();
    let deadline = Instant::now() + Duration::from_secs(20);
    while svc.readiness("mac") != Readiness::Ready {
        assert!(Instant::now() < deadline, "runner never connected");
        tokio::time::sleep(Duration::from_millis(200)).await;
    }

    let bot = Caller::new("live-test");
    let ask = |name: &str| {
        format!(
            "Use your Bash tool to run exactly this command: touch {name} . Run nothing else. If it is denied, reply DENIED and stop."
        )
    };
    let started = svc
        .start(
            &bot,
            StartRequest {
                project_id: "live".into(),
                prompt: ask("allowed.txt"),
                machine_id: Some("mac".into()),
                agent: Some(Agent::Claude),
                notify: false,
                request_id: Some(format!("live-{}", std::process::id())),
            },
        )
        .await;
    let worktree_cell = std::sync::Mutex::new(None::<String>);
    let outcome = std::panic::AssertUnwindSafe(async {
        let started = started.as_ref().unwrap();
        eprintln!("started: {started:?}");
        let id = started.session_id.clone();
        let view = svc.get(&id, None, Duration::ZERO).await.unwrap();
        *worktree_cell.lock().unwrap() = view.worktree_path.clone();
        let worktree = PathBuf::from(view.worktree_path.clone().unwrap());
        assert!(view.muted && !view.notifies, "notify=false starts muted");
        let mut seen = Vec::new();

        // 1. Approve through the hub: the tool runs.
        let approval = poll_until(&svc, &id, "first approval", &mut seen, pending).await;
        let out = svc.decide(&bot, &id, &approval, true, None).await.unwrap();
        assert_eq!(
            (out.outcome, out.delivery),
            (Decision::Allow, Some(Delivery::Delivered))
        );
        let deadline = Instant::now() + TURN;
        while !worktree.join("allowed.txt").exists() {
            assert!(Instant::now() < deadline, "allowed.txt never appeared");
            tokio::time::sleep(Duration::from_millis(500)).await;
        }
        let replay = svc.decide(&bot, &id, &approval, false, None).await.unwrap();
        assert_eq!((replay.outcome, replay.replayed), (Decision::Allow, true));

        // 2. acceptEdits: a file edit goes through without an approval.
        poll_until(&svc, &id, "idle after the first turn", &mut seen, |v| {
            (v.agent_status == "idle" || v.agent_status == "done").then_some(())
        })
        .await;
        let before = svc.audit_log(Some(&id), 500).await.unwrap().iter().filter(|e| e.kind == "approval_requested").count();
        svc.prompt(&bot, &id, "Use your Write tool to create notes.md containing the single line: hello. Do nothing else.", None)
            .await
            .unwrap();
        let deadline = Instant::now() + TURN;
        while !worktree.join("notes.md").exists() {
            assert!(Instant::now() < deadline, "notes.md never appeared");
            let v = svc.get(&id, None, Duration::ZERO).await.unwrap();
            assert!(v.pending_approvals.is_empty(), "an edit asked for approval: {:?}", v.pending_approvals);
            tokio::time::sleep(Duration::from_millis(500)).await;
        }
        let after = svc.audit_log(Some(&id), 500).await.unwrap().iter().filter(|e| e.kind == "approval_requested").count();
        assert_eq!(before, after, "the edit was accepted without the hook");

        // 3. A follow-up prompt through the hub, denied through the hub: the tool does not run.
        poll_until(&svc, &id, "idle after the edit", &mut seen, |v| {
            (v.agent_status == "idle" || v.agent_status == "done").then_some(())
        })
        .await;
        svc.prompt(&bot, &id, &ask("denied.txt"), None)
            .await
            .unwrap();
        let approval = poll_until(&svc, &id, "second approval", &mut seen, pending).await;
        let out = svc.decide(&bot, &id, &approval, false, None).await.unwrap();
        assert_eq!(out.outcome, Decision::Deny);
        poll_until(&svc, &id, "idle after the deny", &mut seen, |v| {
            (v.pending_approvals.is_empty()
                && (v.agent_status == "idle" || v.agent_status == "done"))
                .then_some(())
        })
        .await;
        assert!(
            !worktree.join("denied.txt").exists(),
            "denied command must not run"
        );
        let tail = svc.read(&id, 40).await.unwrap();
        eprintln!("tail:\n{}", tail.text);
        assert!(
            tail.text.contains("Denied by PermissionRequest hook") || tail.text.contains("DENIED")
        );
        eprintln!("statuses seen: {seen:?}");
        assert!(seen.iter().any(|s| s == "working"));

        let kinds: Vec<String> = svc
            .audit_log(Some(&id), 200)
            .await
            .unwrap()
            .into_iter()
            .map(|e| e.kind)
            .collect();
        eprintln!("audit: {kinds:?}");
        for kind in ["session_start", "approval_requested", "decision", "prompt"] {
            assert!(kinds.iter().any(|k| k == kind), "{kind} missing");
        }

        let stopped = svc.stop(&bot, &id, false).await.unwrap();
        eprintln!("stopped: {stopped:?}");
        assert_eq!(
            svc.get(&id, None, Duration::ZERO).await.unwrap().lifecycle,
            Lifecycle::Stopped
        );
    })
    .catch_unwind()
    .await;

    if let Ok(s) = &started {
        let _ = svc.stop(&bot, &s.session_id, false).await;
    }
    if let Some(wt) = worktree_cell.lock().unwrap().take() {
        let _ = Command::new("git")
            .arg("-C")
            .arg(&repo)
            .args(["worktree", "remove", "--force", &wt])
            .status();
        if let Some(parent) = Path::new(&wt).parent() {
            let _ = std::fs::remove_dir(parent);
        }
    }
    remove_trust_key(&claude_json, &repo);
    hub.shutdown().await;
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
