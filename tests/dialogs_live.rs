//! A real agent's permission dialog, read and answered by keys through the runner, as for an
//! agent started in Herdr rather than by Switchboard. Skipped unless
//! SWITCHBOARD_LIVE_HERDR_SOCKET names a throwaway Herdr session's socket and
//! SWITCHBOARD_LIVE_AGENTS includes `codex` (Codex 0.165.0). The test starts its own agent in a
//! scratch repo in that session, chooses the dialog's deny row, and checks nothing ran.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

use switchboard::cli::Agent;
use switchboard::herdr::Client;
use switchboard::herdr::types::{AgentPromptParams, AgentStartParams, WorkspaceCreateParams};
use switchboard::proto::{DialogView, Op};
use switchboard::runner::codex::{CodexPane, clear_startup};
use switchboard::runner::hooks::HookHub;
use switchboard::runner::sessions::{Sessions, SessionsConfig};
use tokio::sync::mpsc;
use tokio::time::Instant;

fn enabled(agent: &str) -> bool {
    std::env::var("SWITCHBOARD_LIVE_AGENTS").is_ok_and(|v| v.split(',').any(|a| a.trim() == agent))
}

fn scratch_repo(dir: &Path) -> PathBuf {
    let repo = dir.join("repo");
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
    git(&["init", "-q"]);
    std::fs::write(repo.join("notes.txt"), "hello\n").unwrap();
    git(&["add", "."]);
    git(&[
        "-c",
        "user.name=t",
        "-c",
        "user.email=t@t",
        "commit",
        "-qm",
        "init",
    ]);
    repo
}

async fn dialog(sessions: &Sessions, agent: &str) -> Option<DialogView> {
    let v = sessions
        .handle(Op::Dialog {
            agent: agent.into(),
        })
        .await
        .ok()?;
    serde_json::from_value(v["dialog"].clone()).ok()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_codex_dialog_is_denied_by_arrows_and_one_enter() {
    let Some(socket) = std::env::var_os("SWITCHBOARD_LIVE_HERDR_SOCKET") else {
        eprintln!("skipped: SWITCHBOARD_LIVE_HERDR_SOCKET is not set");
        return;
    };
    if !enabled("codex") {
        eprintln!("skipped: SWITCHBOARD_LIVE_AGENTS doesn't include codex");
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    // Codex resolves symlinks such as macOS's /var, so its trust override must name the real path.
    let repo = std::fs::canonicalize(scratch_repo(dir.path())).unwrap();
    let herdr = Client::new(&socket).with_timeout(Duration::from_secs(60));
    let pane = herdr
        .workspace_create(WorkspaceCreateParams {
            cwd: Some(repo.to_string_lossy().into_owned()),
            label: Some("sb-dialogs-live".into()),
            focus: false,
        })
        .await
        .unwrap();
    let name = "sb-dialogs-codex";
    let untrusted = format!(
        "projects={{{}={{trust_level=\"untrusted\"}}}}",
        switchboard::agents::codex::toml_str(&repo.to_string_lossy())
    );
    let started = herdr
        .agent_start(AgentStartParams {
            name: name.into(),
            kind: "codex".into(),
            pane_id: pane.pane_id.clone(),
            args: ["-s", "read-only", "-a", "on-request", "-c", &untrusted]
                .map(String::from)
                .to_vec(),
            timeout_ms: Some(60_000),
        })
        .await;
    if let Err(e) = &started {
        assert_eq!(e.server_code(), Some("agent_not_ready"), "{e}");
    }
    let codex = CodexPane::new(&herdr, Agent::Codex, name).unwrap();
    clear_startup(&codex, "0.165.0", Duration::from_secs(30))
        .await
        .expect("the restricted startup screen clears");
    // Herdr takes prompts once it has finished the launch, which can lag Codex's own screen.
    let ready_by = Instant::now() + Duration::from_secs(30);
    loop {
        let sent = herdr
            .agent_prompt(AgentPromptParams {
                target: name.into(),
                text: "Run exactly this shell command: touch made-by-codex.txt . If the sandbox blocks it, request escalated permission for exactly that command. Do nothing else.".into(),
                wait: None,
            })
            .await;
        match sent {
            Ok(_) => break,
            Err(e) if e.server_code() == Some("agent_not_ready") && Instant::now() < ready_by => {
                tokio::time::sleep(Duration::from_millis(500)).await;
            }
            Err(e) => panic!("prompting codex: {e}"),
        }
    }

    let (tx, _events) = mpsc::channel(64);
    let state = tempfile::tempdir().unwrap();
    let sessions = Sessions::new(
        Client::new(&socket).with_timeout(Duration::from_secs(60)),
        HookHub::new(tx.clone(), Duration::from_secs(60)),
        tx,
        SessionsConfig {
            state_dir: state.path().to_owned(),
            claude_config: PathBuf::from("/nonexistent/claude.json"),
            claude_mode: Default::default(),
            claude_projects: PathBuf::from("/nonexistent/claude/projects"),
            codex_sessions: PathBuf::from("/nonexistent/codex/sessions"),
            exe: PathBuf::from(env!("CARGO_BIN_EXE_switchboard")),
            hook_addr: "127.0.0.1:9".parse().unwrap(),
            codex_version_cmd: vec!["codex".into(), "--version".into()],
            claude_version_cmd: vec!["claude".into(), "--version".into()],
            liveness_timeout: Duration::from_secs(30),
            startup_timeout: Duration::from_secs(30),
        },
    );
    let deadline = Instant::now() + Duration::from_secs(180);
    let d = loop {
        if let Some(d) = dialog(&sessions, name).await
            && d.answerable
        {
            break d;
        }
        assert!(Instant::now() < deadline, "no answerable dialog appeared");
        tokio::time::sleep(Duration::from_secs(1)).await;
    };
    eprintln!("dialog: {}", serde_json::to_string(&d).unwrap());
    let no = d.options.iter().find(|o| o.is_deny).expect("a deny row");
    let yes = d.options.iter().find(|o| o.offered && !o.is_deny).unwrap();
    // Without the approve right only the deny may be chosen, and nothing is sent.
    let refused = sessions
        .handle(Op::Answer {
            agent: name.into(),
            occurrence: d.occurrence.clone(),
            option: yes.id.clone(),
            label_sha256: yes.label_sha256.clone(),
            deny_only: true,
        })
        .await
        .unwrap_err();
    assert_eq!(
        refused.code,
        switchboard::proto::ErrorCode::ApproveNotAllowed
    );
    sessions
        .handle(Op::Answer {
            agent: name.into(),
            occurrence: d.occurrence.clone(),
            option: no.id.clone(),
            label_sha256: no.label_sha256.clone(),
            deny_only: true,
        })
        .await
        .expect("the deny is answered");
    tokio::time::sleep(Duration::from_secs(2)).await;
    assert!(
        !repo.join("made-by-codex.txt").exists(),
        "the denied command never ran"
    );
    let after = dialog(&sessions, name).await;
    assert!(
        after.is_none_or(|a| a.occurrence != d.occurrence),
        "the dialog closed"
    );
    let _ = herdr.workspace_close(&pane.workspace_id).await;
}
