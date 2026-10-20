//! The WhatsApp front door against a named Herdr test session and real Claude Code: the hub
//! with the bridge in self-chat mode on a fake transport, and the real runner over WebSocket.
//! Skipped unless SWITCHBOARD_LIVE_HERDR_SOCKET names a test session's socket and
//! SWITCHBOARD_LIVE_AGENTS includes `claude`. Pre-trusts a scratch repo in ~/.claude.json and
//! removes that key afterwards.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;
use std::time::Duration;

use futures_util::FutureExt;
use serde_json::Value;
use switchboard::config::HubConfig;
use switchboard::herdr::Client;
use switchboard::hub::runners::Readiness;
use switchboard::hub::serve;
use switchboard::hub::service::{Clock, SessionService};
use switchboard::proto::auth::hash_token;
use switchboard::runner::forward_named_agents;
use switchboard::runner::hooks::HookHub;
use switchboard::runner::link::{self, LinkConfig};
use switchboard::runner::sessions::{Sessions, SessionsConfig};
use switchboard::runner::watch::watch;
use switchboard::store::{Lifecycle, Store};
use switchboard::whatsapp::relay::{self, Relay};
use switchboard::whatsapp::selfchat::{MARKER, SelfChat};
use switchboard::whatsapp::transport::{FakeTransport, Inbound};
use tokio::net::TcpListener;
use tokio::sync::mpsc;
use tokio::time::Instant;
use url::Url;

const ME: &str = "447700900123";
const TOKEN: &str = "live-mac-token";
const TURN: Duration = Duration::from_secs(240);

fn enabled() -> bool {
    std::env::var("SWITCHBOARD_LIVE_AGENTS")
        .is_ok_and(|v| v.split(',').any(|a| a.trim() == "claude"))
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

/// Waits for the next message from the bridge matching `pred`, skipping others, and returns it.
async fn expect(
    t: &FakeTransport,
    seen: &mut usize,
    what: &str,
    pred: impl Fn(&str) -> bool,
) -> String {
    let deadline = Instant::now() + TURN;
    loop {
        let sent = t.sent();
        while let Some((_, text)) = sent.get(*seen) {
            *seen += 1;
            assert!(text.starts_with(MARKER), "{text}");
            let text = &text[MARKER.len()..];
            eprintln!("--- bridge:\n{text}\n");
            if pred(text) {
                return text.to_owned();
            }
        }
        assert!(Instant::now() < deadline, "timed out waiting for {what}");
        tokio::time::sleep(Duration::from_millis(300)).await;
    }
}

/// The number in "approval N:".
fn approval_number(text: &str) -> i64 {
    let rest = &text[text.find("approval ").unwrap() + "approval ".len()..];
    rest[..rest.find(':').unwrap()].parse().unwrap()
}

#[tokio::test(flavor = "multi_thread")]
async fn whatsapp_starts_a_claude_session_and_decides_its_approvals() {
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
    let db = work.path().join("hub.db");
    let cfg = HubConfig::parse(&format!(
        r#"
        public_url = "http://localhost:1"
        listen = "127.0.0.1:0"
        database = "{}"
        default_machine = "mac"
        [[machines]]
        id = "mac"
        token_hash = "{}"
        [[projects]]
        id = "livewa"
        description = "scratch repo for the live WhatsApp test"
        repos = {{ mac = "{}" }}
        "#,
        db.display(),
        hash_token(TOKEN),
        repo.display()
    ))
    .unwrap();
    let svc = SessionService::new(cfg, Store::open(&db).await.unwrap(), Clock::System);
    svc.recover().await.unwrap();
    let hub = serve(svc.clone(), TcpListener::bind("127.0.0.1:0").await.unwrap()).unwrap();

    let (tx, events) = mpsc::channel(256);
    let hooks = HookHub::new(tx.clone(), Duration::from_secs(120));
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let hook_addr = listener.local_addr().unwrap();
    tokio::spawn(hooks.clone().serve(listener));
    let sessions = Sessions::new(
        Client::new(&socket).with_timeout(Duration::from_secs(30)),
        hooks,
        tx,
        SessionsConfig {
            state_dir: work.path().join("runner"),
            claude_config: claude_json.clone(),
            exe: PathBuf::from(env!("CARGO_BIN_EXE_switchboard")),
            hook_addr,
            codex_version_cmd: vec!["codex".into(), "--version".into()],
            liveness_timeout: Duration::from_secs(30),
            startup_timeout: Duration::from_secs(30),
        },
    );
    tokio::spawn(watch(sessions.clone()));
    tokio::spawn(forward_named_agents(
        Client::new(&socket),
        sessions.clone(),
        Duration::from_secs(2),
    ));
    let link_cfg = LinkConfig {
        url: Url::parse(&format!("ws://{}/runner", hub.addr)).unwrap(),
        token: TOKEN.into(),
        machine_id: "mac".into(),
        herdr_version: None,
    };
    tokio::spawn(link::run(link_cfg, sessions, events));
    let deadline = Instant::now() + Duration::from_secs(20);
    while svc.readiness("mac") != Readiness::Ready {
        assert!(Instant::now() < deadline, "runner never connected");
        tokio::time::sleep(Duration::from_millis(200)).await;
    }

    let transport = Arc::new(FakeTransport::default());
    let (inbound, inbound_rx) = mpsc::channel(64);
    let mut relay = Relay::self_chat(svc.clone(), Arc::new(SelfChat::new(ME)));
    let hub_events = svc.subscribe();
    let t = transport.clone();
    tokio::spawn(async move { relay::run(&mut relay, &*t, inbound_rx, hub_events).await });
    let mut n = 0;
    let mut say = |text: &str| {
        n += 1;
        let m = Inbound {
            id: format!("live-{n}"),
            chat: format!("{ME}@s.whatsapp.net"),
            sender: format!("{ME}:0@s.whatsapp.net"),
            sender_alt: None,
            is_group: false,
            from_me: true,
            text: text.into(),
        };
        let inbound = inbound.clone();
        async move { inbound.send(m).await.unwrap() }
    };
    let ask = |name: &str| {
        format!(
            "Use your Bash tool to run exactly this command: touch {name} . Run nothing else. If it is denied, reply DENIED and stop."
        )
    };

    let worktree = std::sync::Mutex::new(None::<PathBuf>);
    let mut seen = 0;
    let outcome = std::panic::AssertUnwindSafe(async {
        // 1. A message naming the project starts a session and delivers the prompt.
        say(&format!("In livewa: {}", ask("allowed.txt"))).await;
        let started = expect(&transport, &mut seen, "started", |t| {
            t.starts_with("Started ")
        })
        .await;
        let s = svc.list().await.unwrap().remove(0);
        *worktree.lock().unwrap() = s.worktree_path.clone().map(PathBuf::from);
        let wt = PathBuf::from(s.worktree_path.clone().unwrap());
        assert!(
            started.contains(&format!("branch sb/{}", s.id)),
            "{started}"
        );

        // 4. Its permission request arrives with a number; approving it runs the tool.
        let note = expect(&transport, &mut seen, "first approval", |t| {
            t.contains("asks to use Bash")
        })
        .await;
        assert!(note.contains("touch allowed.txt"), "{note}");
        let a = approval_number(&note);
        say(&format!("approve {a}")).await;
        expect(&transport, &mut seen, "approved", |t| {
            t == format!("Approved {a}.")
        })
        .await;
        let deadline = Instant::now() + TURN;
        while !wt.join("allowed.txt").exists() {
            assert!(Instant::now() < deadline, "allowed.txt never appeared");
            tokio::time::sleep(Duration::from_millis(500)).await;
        }
        expect(&transport, &mut seen, "finished", |t| {
            t.contains("finished:")
        })
        .await;

        // 2. A follow-up through `use this session`, denied from chat: the tool doesn't run.
        say(&format!("use this session: {}", ask("denied.txt"))).await;
        let note = expect(&transport, &mut seen, "second approval", |t| {
            t.contains("asks to use Bash")
        })
        .await;
        assert!(note.contains("touch denied.txt"), "{note}");
        let d = approval_number(&note);
        say(&format!("deny {d}")).await;
        expect(&transport, &mut seen, "denied", |t| {
            t == format!("Denied {d}.")
        })
        .await;
        expect(&transport, &mut seen, "finished after deny", |t| {
            t.contains("finished:")
        })
        .await;
        assert!(
            !wt.join("denied.txt").exists(),
            "denied command must not run"
        );
        say(&format!("approve {d}")).await;
        expect(&transport, &mut seen, "replay", |t| {
            t == format!("Approval {d} was already denied.")
        })
        .await;

        let label = format!("livewa-{}", &s.id[..4]);
        say(&format!("stop {label}")).await;
        expect(&transport, &mut seen, "stopped", |t| {
            t.starts_with(&format!("Stopped {label}."))
        })
        .await;
        assert_eq!(
            svc.get(&s.id, None, Duration::ZERO)
                .await
                .unwrap()
                .lifecycle,
            Lifecycle::Stopped
        );
        let kinds: Vec<String> = svc
            .audit_log(Some(&s.id), 200)
            .await
            .unwrap()
            .into_iter()
            .map(|e| e.kind)
            .collect();
        eprintln!("audit: {kinds:?}");
        for kind in [
            "session_start",
            "approval_requested",
            "decision",
            "prompt",
            "stop",
        ] {
            assert!(kinds.iter().any(|k| k == kind), "{kind} missing");
        }
    })
    .catch_unwind()
    .await;

    if let Some(wt) = worktree.lock().unwrap().take() {
        let _ = Command::new("git")
            .arg("-C")
            .arg(&repo)
            .args(["worktree", "remove", "--force"])
            .arg(&wt)
            .status();
        if let Some(parent) = wt.parent() {
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
