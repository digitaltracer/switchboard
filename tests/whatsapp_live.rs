//! The WhatsApp bridge against real Herdr and a real Claude Code agent, with a fake WhatsApp
//! transport. Skipped unless SWITCHBOARD_LIVE_HERDR_SOCKET names a test session's socket and
//! SWITCHBOARD_LIVE_AGENTS includes `claude`. Pre-trusts a scratch repo in ~/.claude.json and
//! removes that key afterwards.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;
use std::time::Duration;

use futures_util::FutureExt;
use serde_json::Value;
use switchboard::herdr::Client;
use switchboard::herdr::types::{AgentStartParams, WorktreeCreateParams};
use switchboard::whatsapp::agents::HerdrAgents;
use switchboard::whatsapp::relay::{self, Relay};
use switchboard::whatsapp::transport::{FakeTransport, Inbound};
use switchboard::whatsapp::watch;
use switchboard::whatsapp::watchlist::WatchStore;
use tokio::sync::mpsc;
use tokio::time::Instant;

const ME: &str = "447700900123";
/// A Switchboard-style name, so the bridge watches it with no configuration.
const AGENT: &str = "sb-walive";

fn from_me(text: &str) -> Inbound {
    Inbound {
        sender: format!("{ME}@s.whatsapp.net"),
        sender_alt: None,
        is_group: false,
        from_me: false,
        text: text.into(),
    }
}

/// Waits until the fake transport has sent a message matching `pred`, and returns it.
async fn expect(t: &FakeTransport, what: &str, pred: impl Fn(&str) -> bool) -> String {
    let deadline = Instant::now() + Duration::from_secs(120);
    loop {
        if let Some((_, text)) = t
            .sent()
            .into_iter()
            .find(|(to, text)| to == ME && pred(text))
        {
            eprintln!("--- {what}:\n{text}\n");
            return text;
        }
        assert!(
            Instant::now() < deadline,
            "timed out waiting for {what}; sent so far: {:#?}",
            t.sent()
        );
        tokio::time::sleep(Duration::from_millis(500)).await;
    }
}

fn git(dir: &Path, args: &[&str]) {
    assert!(
        Command::new("git")
            .arg("-C")
            .arg(dir)
            .args(args)
            .status()
            .unwrap()
            .success()
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn bridge_status_prompt_notifications_and_blocked_refusal() {
    let Some(socket) = std::env::var_os("SWITCHBOARD_LIVE_HERDR_SOCKET") else {
        eprintln!("skipped: SWITCHBOARD_LIVE_HERDR_SOCKET not set");
        return;
    };
    if !std::env::var("SWITCHBOARD_LIVE_AGENTS").is_ok_and(|v| v.split(',').any(|a| a == "claude"))
    {
        eprintln!("skipped: claude not in SWITCHBOARD_LIVE_AGENTS");
        return;
    }
    let claude_json = PathBuf::from(std::env::var("HOME").unwrap()).join(".claude.json");
    let repo_dir = tempfile::tempdir().unwrap();
    let repo = repo_dir.path().canonicalize().unwrap();
    git(&repo, &["init", "-q", "-b", "main"]);
    git(
        &repo,
        &[
            "-c",
            "user.name=sb",
            "-c",
            "user.email=sb@test",
            "commit",
            "-q",
            "--allow-empty",
            "-m",
            "init",
        ],
    );
    switchboard::agents::claude::pretrust(&claude_json, &repo).unwrap();

    let herdr = Client::new(&socket).with_timeout(Duration::from_secs(30));
    let created = herdr
        .worktree_create(WorktreeCreateParams {
            cwd: Some(repo.to_string_lossy().into_owned()),
            branch: Some("sb/wa-live".into()),
            ..Default::default()
        })
        .await
        .unwrap();
    let workspace = created.workspace.workspace_id.clone();
    let worktree = PathBuf::from(&created.worktree.path);

    let body =
        std::panic::AssertUnwindSafe(scenario(&socket, &created.root_pane.pane_id, &worktree))
            .catch_unwind()
            .await;

    let _ = herdr.workspace_close(&workspace).await;
    git(
        &repo,
        &["worktree", "remove", "--force", &created.worktree.path],
    );
    if let Some(parent) = worktree.parent() {
        let _ = std::fs::remove_dir(parent);
    }
    remove_trust_key(&claude_json, &repo);
    if let Err(panic) = body {
        std::panic::resume_unwind(panic);
    }
}

async fn scenario(socket: &std::ffi::OsStr, pane: &str, worktree: &Path) {
    let herdr = Client::new(socket).with_timeout(Duration::from_secs(30));
    // The bridge is already running when the session's agent appears.
    let transport = Arc::new(FakeTransport::default());
    let (status_tx, status_rx) = mpsc::channel(256);
    tokio::spawn(watch::watch(Client::new(socket), status_tx));
    let (in_tx, in_rx) = mpsc::channel(64);
    let t = transport.clone();
    let relay_socket = socket.to_owned();
    tokio::spawn(async move {
        let mut r = Relay::new(
            HerdrAgents::new(Client::new(&relay_socket)),
            vec![ME.into()],
            WatchStore::in_memory(),
        );
        relay::run(&mut r, &*t, in_rx, status_rx).await;
    });

    for attempt in 0.. {
        let started = herdr
            .agent_start(AgentStartParams {
                name: AGENT.into(),
                kind: "claude".into(),
                pane_id: pane.into(),
                args: vec!["--permission-mode".into(), "default".into()],
                timeout_ms: Some(60_000),
            })
            .await;
        match started {
            Err(e) if e.server_code() == Some("agent_pane_busy") && attempt < 20 => {
                tokio::time::sleep(Duration::from_millis(500)).await
            }
            other => {
                other.unwrap();
                break;
            }
        }
    }
    // Herdr may keep the launch pending for a while after the agent's UI is up.
    let deadline = Instant::now() + Duration::from_secs(60);
    while !herdr
        .session_snapshot()
        .await
        .unwrap()
        .agents
        .iter()
        .any(|a| a.name.as_deref() == Some(AGENT) && a.interactive_ready && !a.launch_pending)
    {
        assert!(Instant::now() < deadline, "agent never became ready");
        tokio::time::sleep(Duration::from_millis(500)).await;
    }

    // 1. Status from the allowed number; nothing for anyone else.
    in_tx
        .send(Inbound {
            sender: "15550001111@s.whatsapp.net".into(),
            ..from_me("status")
        })
        .await
        .unwrap();
    in_tx.send(from_me("status")).await.unwrap();
    expect(&transport, "status reply", |t| {
        t.starts_with(&format!("{AGENT}: ")) && t.ends_with("· watching")
    })
    .await;
    assert!(transport.sent().iter().all(|(to, _)| to == ME));
    in_tx.send(from_me(&format!("use {AGENT}"))).await.unwrap();
    expect(&transport, "use reply", |t| {
        t == format!("Messages now go to {AGENT}.")
    })
    .await;

    // 2. A plain message is a prompt; finishing produces the last lines.
    in_tx
        .send(from_me(
            "Reply with exactly the word PONGWA and nothing else. Do not use any tools.",
        ))
        .await
        .unwrap();
    expect(&transport, "prompt ack", |t| {
        t == format!("Sent to {AGENT}.")
    })
    .await;
    // The reply itself, on its own line, not just Claude's session title.
    let reply_line = |t: &str| {
        t.lines()
            .any(|l| l.trim().trim_start_matches('⏺').trim() == "PONGWA")
    };
    expect(&transport, "finished notification", |t| {
        t.starts_with(&format!("{AGENT} finished:")) && reply_line(t)
    })
    .await;

    // 3. A permission dialog produces the excerpt; a prompt while blocked is refused.
    in_tx
        .send(from_me("Use your Bash tool to run exactly this command: touch wa-probe.txt . Run nothing else."))
        .await
        .unwrap();
    let blocked = expect(&transport, "blocked notification", |t| {
        t.starts_with(&format!("{AGENT} is waiting for approval:"))
    })
    .await;
    assert!(blocked.contains("wa-probe.txt"), "excerpt shows the dialog");
    in_tx.send(from_me("never mind, just stop")).await.unwrap();
    let refused = expect(&transport, "refusal", |t| {
        t.contains("is waiting at a dialog, so I didn't send that")
    })
    .await;
    assert!(refused.contains("wa-probe.txt"));
    tokio::time::sleep(Duration::from_secs(3)).await;
    assert!(
        !worktree.join("wa-probe.txt").exists(),
        "nothing approved the command"
    );
    let still = herdr.session_snapshot().await.unwrap();
    let agent = still
        .agents
        .iter()
        .find(|a| a.name.as_deref() == Some(AGENT))
        .unwrap();
    assert_eq!(
        serde_json::to_value(agent.agent_status).unwrap(),
        "blocked",
        "the bridge left the dialog alone"
    );
}

fn remove_trust_key(claude_json: &Path, repo: &Path) {
    let text = std::fs::read_to_string(claude_json).unwrap();
    let mut doc: Value = serde_json::from_str(&text).unwrap();
    if let Some(p) = doc.get_mut("projects").and_then(Value::as_object_mut) {
        p.remove(&repo.to_string_lossy().into_owned());
    }
    std::fs::write(claude_json, serde_json::to_vec_pretty(&doc).unwrap()).unwrap();
}
