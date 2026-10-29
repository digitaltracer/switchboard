//! Switchboard's automatic reads must leave a full-screen agent's pane still. Herdr collects
//! history beyond the visible screen by scrolling the agent's own interface up and back, which a
//! client attached to the session sees as a redraw.
//!
//! Skipped unless SWITCHBOARD_LIVE_HERDR_SOCKET names a throwaway session's socket
//! (`…/herdr/sessions/<name>/herdr.sock`, started with `herdr --session <name> server` and the
//! `HERDR_*` variables unset) and SWITCHBOARD_LIVE_AGENTS includes `claude`. The session must
//! already run Claude Code with `"tui": "fullscreen"` as a named agent, 40 or so rows tall; the
//! test never edits Claude's config. It attaches a headless client through a PTY, prompts the
//! agent, and once it has been idle past the WhatsApp hold, counts the bytes the client redraws
//! after each read. An 80-line history read must redraw, or the test fails as uncalibrated.

use std::io::Read;
use std::os::fd::{FromRawFd, OwnedFd};
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use switchboard::herdr::Client;
use switchboard::proto::{AgentSummary, Op};
use switchboard::runner::hooks::HookHub;
use switchboard::runner::sessions::{Sessions, SessionsConfig};
use tokio::net::TcpListener;
use tokio::sync::mpsc;
use tokio::time::Instant;

const TURN: Duration = Duration::from_secs(180);
/// How long a read's redraw is counted for.
const SETTLE: Duration = Duration::from_secs(2);

fn enabled() -> Option<PathBuf> {
    let agents = std::env::var("SWITCHBOARD_LIVE_AGENTS").ok()?;
    agents
        .split(',')
        .any(|a| a.trim() == "claude")
        .then(|| std::env::var_os("SWITCHBOARD_LIVE_HERDR_SOCKET").map(PathBuf::from))
        .flatten()
}

/// A Herdr client attached to the session in a PTY of `rows` by 120, counting what it draws.
struct Attached {
    child: Child,
    drawn: Arc<AtomicUsize>,
}

impl Attached {
    fn new(session: &str, rows: u16) -> Self {
        let (mut master, mut slave) = (0, 0);
        let mut size = libc::winsize {
            ws_row: rows,
            ws_col: 120,
            ws_xpixel: 0,
            ws_ypixel: 0,
        };
        // SAFETY: both out-pointers are valid; no name buffer or termios is asked for.
        let rc = unsafe {
            libc::openpty(
                &mut master,
                &mut slave,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                &mut size,
            )
        };
        assert_eq!(rc, 0, "openpty");
        // SAFETY: `openpty` just returned these descriptors and nothing else owns them.
        let (master, slave) =
            unsafe { (OwnedFd::from_raw_fd(master), OwnedFd::from_raw_fd(slave)) };
        let mut cmd = Command::new("herdr");
        cmd.args(["session", "attach", session])
            .env("TERM", "xterm-256color")
            .stdin(Stdio::from(slave.try_clone().unwrap()))
            .stdout(Stdio::from(slave.try_clone().unwrap()))
            .stderr(Stdio::from(slave));
        for (k, _) in std::env::vars() {
            if k.starts_with("HERDR_") {
                cmd.env_remove(k);
            }
        }
        let child = cmd.spawn().expect("herdr on PATH");
        let drawn = Arc::new(AtomicUsize::new(0));
        let counted = drawn.clone();
        let mut out = std::fs::File::from(master);
        std::thread::spawn(move || {
            let mut buf = [0u8; 8192];
            while let Ok(n) = out.read(&mut buf) {
                if n == 0 {
                    break;
                }
                counted.fetch_add(n, Ordering::SeqCst);
            }
        });
        Self { child, drawn }
    }

    /// Bytes drawn in the `SETTLE` after `read` runs.
    async fn redraw_after<F: Future>(&self, read: F) -> usize {
        let before = self.drawn.load(Ordering::SeqCst);
        read.await;
        tokio::time::sleep(SETTLE).await;
        self.drawn.load(Ordering::SeqCst) - before
    }
}

impl Drop for Attached {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

async fn agents(sessions: &Sessions) -> Vec<AgentSummary> {
    serde_json::from_value(sessions.handle(Op::Agents).await.unwrap()).unwrap()
}

#[tokio::test]
async fn automatic_reads_leave_a_full_screen_agent_still() {
    let Some(socket) = enabled() else {
        eprintln!("skipped: set SWITCHBOARD_LIVE_HERDR_SOCKET and SWITCHBOARD_LIVE_AGENTS=claude");
        return;
    };
    let session = socket
        .parent()
        .and_then(|d| d.file_name())
        .expect("…/sessions/<name>/herdr.sock")
        .to_string_lossy()
        .into_owned();
    let home = PathBuf::from(std::env::var_os("HOME").unwrap());
    let claude = std::env::var_os("CLAUDE_CONFIG_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| home.join(".claude"));
    let (tx, _events) = mpsc::channel(256);
    let hooks = HookHub::new(tx.clone(), Duration::from_secs(120));
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let hook_addr = listener.local_addr().unwrap();
    let state = tempfile::tempdir().unwrap();
    let sessions = Sessions::new(
        Client::new(&socket).with_timeout(Duration::from_secs(30)),
        hooks,
        tx,
        SessionsConfig {
            state_dir: state.path().to_owned(),
            claude_config: state.path().join("unused-claude.json"),
            claude_mode: Default::default(),
            claude_projects: claude.join("projects"),
            codex_sessions: home.join(".codex/sessions"),
            exe: PathBuf::from(env!("CARGO_BIN_EXE_switchboard")),
            hook_addr,
            codex_version_cmd: vec!["codex".into(), "--version".into()],
            claude_version_cmd: vec!["claude".into(), "--version".into()],
            liveness_timeout: Duration::from_secs(30),
            startup_timeout: Duration::from_secs(30),
        },
    );
    let agent = agents(&sessions)
        .await
        .into_iter()
        .find(|a| a.kind.as_deref() == Some("claude"))
        .expect("a named Claude agent in the test session")
        .name;
    let attached = Attached::new(&session, 40);
    tokio::time::sleep(Duration::from_secs(3)).await;

    sessions
        .handle(Op::PromptAgent {
            agent: agent.clone(),
            text: "Reply with the single word: ok".into(),
        })
        .await
        .unwrap();
    let deadline = Instant::now() + TURN;
    let mut worked = false;
    loop {
        assert!(Instant::now() < deadline, "the turn never ended");
        let status = agents(&sessions)
            .await
            .into_iter()
            .find(|a| a.name == agent)
            .map(|a| a.agent_status)
            .unwrap_or_default();
        worked |= status == "working";
        if worked && matches!(status.as_str(), "idle" | "done") {
            break;
        }
        tokio::time::sleep(Duration::from_millis(500)).await;
    }
    // Past the WhatsApp bridge's 3 s hold, and the end-of-turn redraw.
    tokio::time::sleep(Duration::from_secs(5)).await;

    let idle = attached.redraw_after(async {}).await;
    assert_eq!(
        idle, 0,
        "the pane redraws on its own, so nothing can be measured"
    );

    let read = |lines, visible| Op::ReadAgent {
        agent: agent.clone(),
        lines,
        visible,
        ansi: false,
    };
    let history = attached
        .redraw_after(async { sessions.handle(read(80, false)).await.unwrap() })
        .await;
    assert!(
        history > 0,
        "uncalibrated: an 80-line history read didn't redraw; is Claude full screen?"
    );
    tokio::time::sleep(SETTLE).await;

    // What the finished notice sends now, and what the blocked notice and the phone's screen tab send.
    let notice = attached
        .redraw_after(async {
            let op = Op::Transcript {
                agent: agent.clone(),
                after: None,
                before: None,
                wait_ms: None,
            };
            sessions
                .handle(op)
                .await
                .expect("the agent's conversation log")
        })
        .await;
    let screen = attached
        .redraw_after(async { sessions.handle(read(0, true)).await.unwrap() })
        .await;
    eprintln!("redrawn: history {history} B, finished notice {notice} B, screen {screen} B");
    assert_eq!(notice, 0, "the finished notice moved the pane");
    assert_eq!(screen, 0, "a visible read moved the pane");
}
