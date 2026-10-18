use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use super::*;
use crate::whatsapp::limits::{BURST, HOLD, REFILL};
use crate::whatsapp::transport::FakeTransport;
use crate::whatsapp::watchlist::WatchStore;

const ME: &str = "447700900123";
const OTHER: &str = "15550001111";

#[derive(Clone, Default)]
struct FakeAgents {
    live: Arc<Mutex<Vec<AgentSummary>>>,
    output: Arc<Mutex<HashMap<String, String>>>,
    screens: Arc<Mutex<HashMap<String, String>>>,
    blocked: Arc<Mutex<Vec<String>>>,
    prompts: Arc<Mutex<Vec<(String, String)>>>,
}

impl FakeAgents {
    fn with(agents: &[(&str, AgentStatus)]) -> Self {
        let f = Self::default();
        *f.live.lock().unwrap() = agents
            .iter()
            .enumerate()
            .map(|(i, (n, s))| AgentSummary {
                name: Some((*n).into()),
                pane_id: format!("w{}:p1", i + 1),
                kind: Some("claude".into()),
                status: *s,
            })
            .collect();
        f
    }
}

impl AgentControl for FakeAgents {
    async fn list(&self) -> Result<Vec<AgentSummary>, String> {
        Ok(self.live.lock().unwrap().clone())
    }

    async fn read(&self, target: &str, _lines: u32) -> Result<String, String> {
        Ok(self
            .output
            .lock()
            .unwrap()
            .get(target)
            .cloned()
            .unwrap_or_default())
    }

    async fn screen(&self, target: &str) -> Result<String, String> {
        Ok(self
            .screens
            .lock()
            .unwrap()
            .get(target)
            .cloned()
            .unwrap_or_default())
    }

    async fn prompt(&self, target: &str, text: &str) -> Result<PromptOutcome, String> {
        if self.blocked.lock().unwrap().iter().any(|b| b == target) {
            return Ok(PromptOutcome::Blocked {
                screen: self.screen(target).await?,
            });
        }
        if !self
            .live
            .lock()
            .unwrap()
            .iter()
            .any(|a| a.label() == target)
        {
            return Ok(PromptOutcome::NotFound);
        }
        self.prompts
            .lock()
            .unwrap()
            .push((target.into(), text.into()));
        Ok(PromptOutcome::Sent)
    }
}

fn from(number: &str, text: &str) -> Inbound {
    Inbound {
        sender: format!("{number}@s.whatsapp.net"),
        sender_alt: None,
        is_group: false,
        from_me: false,
        text: text.into(),
    }
}

fn relay(agents: FakeAgents) -> Relay<FakeAgents> {
    Relay::new(agents, vec![ME.into()], WatchStore::in_memory())
}

/// A relay whose watch state lives in `path`, as after a bridge restart.
fn relay_at(agents: FakeAgents, path: &std::path::Path) -> Relay<FakeAgents> {
    Relay::new(agents, vec![ME.into()], WatchStore::load(path).unwrap())
}

async fn reply(r: &mut Relay<FakeAgents>, text: &str) -> String {
    let out = r.handle(&from(ME, text)).await;
    assert!(out.iter().all(|o| o.to == ME && o.about.is_none()));
    out.into_iter()
        .map(|o| o.text)
        .collect::<Vec<_>>()
        .join("\n---\n")
}

/// Drives one status transition through the debouncer and returns the notifications.
async fn transition(
    r: &mut Relay<FakeAgents>,
    agent: &str,
    from: AgentStatus,
    to: AgentStatus,
) -> Vec<String> {
    let t0 = Instant::now();
    r.observe(agent, from, t0);
    let _ = r.due(t0 + HOLD).await;
    r.observe(agent, to, t0 + HOLD);
    r.due(t0 + HOLD * 2)
        .await
        .into_iter()
        .map(|o| o.text)
        .collect()
}

#[tokio::test]
async fn allowlist_drops_others_silently_and_logs_no_content() {
    let logs = Arc::new(Mutex::new(Vec::<u8>::new()));
    let sink = logs.clone();
    let subscriber = tracing_subscriber::fmt()
        .with_writer(move || LogSink(sink.clone()))
        .with_ansi(false)
        .finish();
    let _guard = tracing::subscriber::set_default(subscriber);

    let mut r = relay(FakeAgents::with(&[("sb-1", AgentStatus::Idle)]));
    assert!(
        r.handle(&from(OTHER, "secret plan: rm -rf everything"))
            .await
            .is_empty()
    );
    assert!(r.handle(&from(OTHER, "mute all")).await.is_empty());
    assert!(
        !r.watch_state().mute_all,
        "strangers can't change watch state"
    );
    let group = Inbound {
        is_group: true,
        ..from(ME, "status")
    };
    assert!(r.handle(&group).await.is_empty());
    let mine = Inbound {
        from_me: true,
        ..from(ME, "status")
    };
    assert!(r.handle(&mine).await.is_empty());
    let lid_only = Inbound {
        sender: "123456789012345@lid".into(),
        ..from(ME, "status")
    };
    assert!(
        r.handle(&lid_only).await.is_empty(),
        "a LID without the phone JID can't be matched"
    );

    let logged = String::from_utf8(logs.lock().unwrap().clone()).unwrap();
    assert!(
        logged.contains(&format!("{OTHER}@s.whatsapp.net")),
        "{logged}"
    );
    assert!(
        !logged.contains("secret plan"),
        "content must never be logged: {logged}"
    );
    assert!(r.agents.prompts.lock().unwrap().is_empty());
}

struct LogSink(Arc<Mutex<Vec<u8>>>);

impl std::io::Write for LogSink {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(buf);
        Ok(buf.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

#[tokio::test]
async fn phone_jid_in_sender_alt_is_accepted() {
    let mut r = relay(FakeAgents::with(&[("sb-1", AgentStatus::Idle)]));
    let lid = Inbound {
        sender: "123456789012345@lid".into(),
        sender_alt: Some(format!("{ME}:3@s.whatsapp.net")),
        ..from(ME, "status")
    };
    let out = r.handle(&lid).await;
    assert_eq!(out.len(), 1);
    assert_eq!(out[0].to, ME);
}

#[tokio::test]
async fn status_and_list_show_watch_state() {
    let agents = FakeAgents::with(&[
        ("sb-3f9a", AgentStatus::Working),
        ("switchboard-impl", AgentStatus::Blocked),
        ("sb-77", AgentStatus::Idle),
    ]);
    agents.live.lock().unwrap().push(AgentSummary {
        name: None,
        pane_id: "w9:p1".into(),
        kind: Some("codex".into()),
        status: AgentStatus::Idle,
    });
    let mut r = relay(agents);
    assert_eq!(
        reply(&mut r, "status").await,
        "sb-3f9a: working · watching\nsb-77: idle · watching"
    );
    assert_eq!(reply(&mut r, "mute sb-77").await, "Muted sb-77.");
    assert_eq!(
        reply(&mut r, "watch ghost").await,
        "Watching ghost. It isn't running now; you'll hear from it once it is."
    );
    assert_eq!(
        reply(&mut r, "status").await,
        "sb-3f9a: working · watching\nsb-77: idle · muted\nghost: not running · watching (opted in)"
    );
    assert_eq!(
        reply(&mut r, "list").await,
        "sb-3f9a (claude): working · watching\n\
         switchboard-impl (claude): blocked (waiting for approval) · not watched\n\
         sb-77 (claude): idle · muted\n\
         w9:p1 (codex): idle · no name, can't be watched"
    );
    assert_eq!(
        reply(&mut r, "mute all").await,
        "Muted all notifications, including agents created later. Send unmute all to resume."
    );
    let status = reply(&mut r, "status").await;
    assert!(status.starts_with("All notifications are muted. Send unmute all to resume.\nsb-3f9a: working · muted (all)"), "{status}");
    assert!(
        reply(&mut r, "list")
            .await
            .ends_with("All notifications are muted. Send unmute all to resume.")
    );
}

#[tokio::test]
async fn status_with_nothing_watched_explains_auto_watch() {
    let mut r = relay(FakeAgents::with(&[("reviewer", AgentStatus::Idle)]));
    assert!(reply(&mut r, "status").await.starts_with(
        "No watched agents are running. Switchboard's agents (sb-...) are watched automatically"
    ));
}

#[tokio::test]
async fn commands_work_on_any_live_agent_watched_or_not() {
    let agents = FakeAgents::with(&[("reviewer", AgentStatus::Idle), ("sb-1", AgentStatus::Idle)]);
    agents.output.lock().unwrap().insert(
        "reviewer".into(),
        "\u{1b}[32mline1\u{1b}[0m\nline2\nline3\n\n".into(),
    );
    let mut r = relay(agents.clone());
    assert_eq!(r.selected(), None);
    assert!(
        reply(&mut r, "read")
            .await
            .starts_with("Pick an agent first")
    );
    assert!(
        reply(&mut r, "fix it")
            .await
            .starts_with("Pick an agent first")
    );
    assert_eq!(
        reply(&mut r, "use nobody").await,
        "No live agent called nobody. Send list to see them."
    );
    assert_eq!(
        reply(&mut r, "use reviewer").await,
        "Messages now go to reviewer."
    );
    assert_eq!(reply(&mut r, "read 2").await, "line2\nline3");
    assert_eq!(
        reply(&mut r, "read the failing test and fix it").await,
        "Sent to reviewer."
    );
    assert_eq!(
        reply(&mut r, "mute reviewer").await,
        "Muted reviewer. It isn't watched, so it wasn't notifying anyway."
    );
    assert_eq!(
        reply(&mut r, "keep going").await,
        "Sent to reviewer.",
        "muting never blocks prompts"
    );
    assert_eq!(
        agents.prompts.lock().unwrap().as_slice(),
        [
            (
                "reviewer".to_string(),
                "read the failing test and fix it".to_string()
            ),
            ("reviewer".to_string(), "keep going".to_string()),
        ]
    );
    assert!(
        reply(&mut r, "help")
            .await
            .contains("can't approve or deny")
    );
    agents
        .live
        .lock()
        .unwrap()
        .retain(|a| a.name.as_deref() != Some("reviewer"));
    assert!(
        reply(&mut r, "again")
            .await
            .contains("isn't running any more")
    );
}

#[tokio::test]
async fn prompt_to_blocked_agent_is_refused_with_the_dialog_excerpt() {
    let agents = FakeAgents::with(&[("sb-1", AgentStatus::Blocked)]);
    agents.blocked.lock().unwrap().push("sb-1".into());
    agents.screens.lock().unwrap().insert(
        "sb-1".into(),
        "Bash command\n  rm -rf build\nDo you want to proceed?\n❯ 1. Yes\n  2. No\n".into(),
    );
    let mut r = relay(agents.clone());
    reply(&mut r, "use sb-1").await;
    let text = reply(&mut r, "carry on").await;
    assert!(
        text.starts_with("sb-1 is waiting at a dialog, so I didn't send that."),
        "{text}"
    );
    assert!(text.contains("Claude's remote control"));
    assert!(text.contains("Do you want to proceed?"));
    assert!(
        agents.prompts.lock().unwrap().is_empty(),
        "nothing reached the agent"
    );
}

#[tokio::test(start_paused = true)]
async fn new_switchboard_agents_notify_without_any_configuration() {
    let agents = FakeAgents::with(&[]);
    agents.screens.lock().unwrap().insert(
        "sb-new".into(),
        "Allow write to /etc/hosts?\n❯ 1. Yes\n".into(),
    );
    agents
        .output
        .lock()
        .unwrap()
        .insert("sb-new".into(), "all 12 tests pass\n".into());
    let mut r = Relay::new(
        agents,
        vec![ME.into(), "15550002222".into()],
        WatchStore::in_memory(),
    );

    let t0 = Instant::now();
    r.observe("sb-new", AgentStatus::Working, t0);
    r.observe("reviewer", AgentStatus::Working, t0);
    let _ = r.due(t0 + HOLD).await;
    r.observe("sb-new", AgentStatus::Blocked, t0 + HOLD);
    r.observe("reviewer", AgentStatus::Blocked, t0 + HOLD);
    let blocked = r.due(t0 + HOLD * 2).await;
    assert_eq!(
        blocked.len(),
        2,
        "one per allowed sender, none for the unwatched agent"
    );
    assert_eq!(
        (blocked[0].to.as_str(), blocked[1].to.as_str()),
        (ME, "15550002222")
    );
    assert!(
        blocked[0]
            .text
            .starts_with("sb-new is waiting for approval:")
    );
    assert!(blocked[0].text.contains("Allow write to /etc/hosts?"));

    r.observe("sb-new", AgentStatus::Done, t0 + HOLD * 3);
    let done = r.due(t0 + HOLD * 4).await;
    assert_eq!(done[0].text, "sb-new finished:\n\nall 12 tests pass");
}

#[tokio::test(start_paused = true)]
async fn mute_and_unmute_persist_across_a_restart() {
    let dir = tempfile::tempdir().unwrap();
    let path = WatchStore::path_for(&dir.path().join("whatsapp.db"));
    let agents = FakeAgents::with(&[
        ("sb-1", AgentStatus::Working),
        ("sb-2", AgentStatus::Working),
    ]);

    let mut r = relay_at(agents.clone(), &path);
    assert_eq!(reply(&mut r, "mute sb-1").await, "Muted sb-1.");
    drop(r);

    let mut r = relay_at(agents.clone(), &path);
    assert!(
        reply(&mut r, "status")
            .await
            .contains("sb-1: working · muted")
    );
    assert!(
        transition(&mut r, "sb-1", AgentStatus::Working, AgentStatus::Blocked)
            .await
            .is_empty()
    );
    assert_eq!(
        transition(&mut r, "sb-2", AgentStatus::Working, AgentStatus::Blocked)
            .await
            .len(),
        1,
        "other agents unaffected"
    );
    assert_eq!(reply(&mut r, "unmute sb-1").await, "Unmuted sb-1.");
    assert_eq!(reply(&mut r, "unmute sb-1").await, "sb-1 wasn't muted.");
    drop(r);

    let mut r = relay_at(agents, &path);
    assert!(
        reply(&mut r, "status")
            .await
            .contains("sb-1: working · watching")
    );
    assert_eq!(
        transition(&mut r, "sb-1", AgentStatus::Working, AgentStatus::Blocked)
            .await
            .len(),
        1
    );
}

#[tokio::test(start_paused = true)]
async fn mute_all_covers_agents_created_later_until_unmute_all() {
    let dir = tempfile::tempdir().unwrap();
    let path = WatchStore::path_for(&dir.path().join("whatsapp.db"));
    let agents = FakeAgents::with(&[("sb-1", AgentStatus::Working)]);
    let mut r = relay_at(agents.clone(), &path);
    reply(&mut r, "watch switchboard-impl").await;
    reply(&mut r, "mute all").await;
    assert_eq!(
        reply(&mut r, "unmute sb-1").await,
        "All notifications are muted. Send unmute all first."
    );
    drop(r);

    // After a restart, an agent that didn't exist when "mute all" was sent stays quiet too.
    let mut r = relay_at(agents.clone(), &path);
    assert!(r.watch_state().mute_all);
    for agent in ["sb-1", "sb-created-later", "switchboard-impl"] {
        assert!(
            transition(&mut r, agent, AgentStatus::Working, AgentStatus::Blocked)
                .await
                .is_empty(),
            "{agent}"
        );
    }
    assert_eq!(
        reply(&mut r, "unmute all").await,
        "Notifications are back on for every watched agent."
    );
    drop(r);

    let mut r = relay_at(agents, &path);
    assert!(!r.watch_state().mute_all);
    assert_eq!(
        transition(
            &mut r,
            "sb-created-even-later",
            AgentStatus::Working,
            AgentStatus::Blocked
        )
        .await
        .len(),
        1
    );
    assert_eq!(
        transition(
            &mut r,
            "switchboard-impl",
            AgentStatus::Working,
            AgentStatus::Done
        )
        .await
        .len(),
        1,
        "opt-in kept"
    );
}

#[tokio::test(start_paused = true)]
async fn watch_opts_in_an_agent_switchboard_did_not_create() {
    let dir = tempfile::tempdir().unwrap();
    let path = WatchStore::path_for(&dir.path().join("whatsapp.db"));
    let agents = FakeAgents::with(&[("switchboard-impl", AgentStatus::Working)]);
    let mut r = relay_at(agents.clone(), &path);
    assert!(
        transition(
            &mut r,
            "switchboard-impl",
            AgentStatus::Working,
            AgentStatus::Blocked
        )
        .await
        .is_empty(),
        "not watched yet"
    );

    assert_eq!(
        reply(&mut r, "watch switchboard-impl").await,
        "Watching switchboard-impl."
    );
    assert_eq!(
        reply(&mut r, "watch sb-1").await,
        "sb-1 is a Switchboard agent, so it's watched already."
    );
    assert!(
        reply(&mut r, "watch w1:p4")
            .await
            .starts_with("Only named agents can be watched.")
    );
    drop(r);

    let mut r = relay_at(agents, &path);
    let notes = transition(
        &mut r,
        "switchboard-impl",
        AgentStatus::Working,
        AgentStatus::Blocked,
    )
    .await;
    assert_eq!(notes.len(), 1);
    assert!(notes[0].starts_with("switchboard-impl is waiting for approval:"));
    assert!(
        reply(&mut r, "list")
            .await
            .contains("switchboard-impl (claude): working · watching (opted in)")
    );
}

#[tokio::test]
async fn a_failed_save_changes_nothing() {
    let dir = tempfile::tempdir().unwrap();
    let mut r = relay_at(
        FakeAgents::with(&[]),
        &dir.path().join("no-such-dir/whatsapp-watch.json"),
    );
    assert!(
        reply(&mut r, "mute all")
            .await
            .starts_with("Couldn't save that, so nothing changed")
    );
    assert!(!r.watch_state().mute_all);
}

#[tokio::test(start_paused = true)]
async fn end_to_end_with_rate_limit_and_reconnect() {
    let agents = FakeAgents::with(&[("sb-1", AgentStatus::Idle)]);
    let transport = Arc::new(FakeTransport::default());
    let (in_tx, in_rx) = mpsc::channel(64);
    let (st_tx, st_rx) = mpsc::channel(64);
    let t = transport.clone();
    let task = tokio::spawn(async move {
        let mut r = relay(agents);
        run(&mut r, &*t, in_rx, st_rx).await;
    });
    let status = || (ME.to_string(), "sb-1: idle · watching".to_string());

    in_tx.send(from(OTHER, "status")).await.unwrap();
    in_tx.send(from(ME, "status")).await.unwrap();
    tokio::time::sleep(Duration::from_millis(10)).await;
    assert_eq!(transport.take(), [status()]);

    // A burst beyond the bucket is held back, then trickles out.
    for _ in 0..(BURST + 3) {
        in_tx.send(from(ME, "help")).await.unwrap();
    }
    tokio::time::sleep(Duration::from_millis(10)).await;
    assert_eq!(transport.take().len(), BURST as usize - 1);
    tokio::time::sleep(REFILL * 4 + Duration::from_secs(1)).await;
    assert_eq!(transport.take().len(), 4);

    // Link down: replies wait and go out after it comes back.
    transport.set_down(true);
    st_tx
        .send(("sb-1".into(), AgentStatus::Idle))
        .await
        .unwrap();
    in_tx.send(from(ME, "status")).await.unwrap();
    tokio::time::sleep(Duration::from_secs(5)).await;
    assert!(transport.sent().is_empty());
    transport.set_down(false);
    // The bucket is nearly empty after the burst, so the reply waits for the next token.
    tokio::time::sleep(REFILL + Duration::from_secs(1)).await;
    assert_eq!(transport.take(), [status()]);

    drop(in_tx);
    task.await.unwrap();
}
