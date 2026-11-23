use std::sync::atomic::{AtomicUsize, Ordering};

use serde_json::{Value, json};

use super::*;
use crate::hc::fake::FakeCore;

fn term(v: Value) -> Terminal {
    let mut base = json!({
        "address": "t1", "pid": 100, "created_ms": 5, "state": "live", "argv": ["/bin/zsh"],
        "cwd": "/w", "labels": {}, "turn": 1,
        "foreground": { "pid": 101, "name": "node", "argv": ["node"], "cwd": "/w/live" },
    });
    for (k, val) in v.as_object().unwrap() {
        base[k] = val.clone();
    }
    serde_json::from_value(base).unwrap()
}

fn claude(state: &str) -> Value {
    json!({ "reported": true, "status": { "state": state, "app": "claude-code" } })
}

fn list(terminals: &[Terminal], doc: &Value) -> Vec<Facts> {
    let all = terminals
        .iter()
        .map(|t| (t.address.clone(), t.clone()))
        .collect();
    listing(
        "core-a",
        terminals.iter(),
        &spaces(doc),
        &HashMap::new(),
        &all,
    )
}

#[test]
fn agents_are_live_local_claude_codex_or_reporting_programs() {
    let ts = [
        term(claude("idle")),
        term(json!({ "address": "t2", "argv": ["/opt/bin/codex", "--no-daemon"] })),
        term(
            json!({ "address": "t3", "argv": ["/usr/bin/env", "-u", "HERDR_ENV", "/x/claude", "--settings", "s"] }),
        ),
        term(
            json!({ "address": "t4", "reported": true, "status": { "state": "working", "app": "aider" } }),
        ),
        term(json!({ "address": "t5" })),
        term(json!({ "address": "t6", "state": "finished", "argv": ["claude"] })),
        term(json!({ "address": "vps/t7", "argv": ["claude"] })),
        term(json!({ "address": "t8", "argv": ["claude"], "pid": null })),
        term(json!({ "address": "t9", "argv": ["claude"],
                     "foreground": { "pid": 9, "name": "-zsh", "argv": ["zsh"], "cwd": "/" } })),
    ];
    let got: Vec<(String, Option<String>)> = list(&ts, &Value::Null)
        .into_iter()
        .map(|f| (f.target, f.kind))
        .collect();
    let want: Vec<(String, Option<String>)> = [
        ("t1", "claude"),
        ("t2", "codex"),
        ("t3", "claude"),
        ("t4", "aider"),
    ]
    .iter()
    .map(|(a, k)| ((*a).to_owned(), Some((*k).to_owned())))
    .collect();
    assert_eq!(got, want);
}

#[test]
fn statuses_map_to_the_runners_words() {
    let s = |state: Value| status_of(&term(json!({ "status": state })));
    assert_eq!(s(json!({ "state": "idle" })), AgentStatus::Idle);
    assert_eq!(s(json!({ "state": "working" })), AgentStatus::Working);
    assert_eq!(s(json!({ "state": "blocked" })), AgentStatus::Blocked);
    assert_eq!(s(json!({ "state": "done" })), AgentStatus::Done);
    assert_eq!(s(json!({ "state": "error" })), AgentStatus::Done);
    assert_eq!(s(Value::Null), AgentStatus::Unknown);
}

#[test]
fn names_come_from_the_terminal_then_its_tab_then_its_address() {
    let doc = json!({ "spaces": [
        { "id": "sp1", "name": "Switchboard", "folder": "/r/sb",
          "tabs": [ { "id": "tb1", "name": "fix login" }, { "id": "tb2" } ] },
        { "id": "sp2", "folder": "/r/other", "tabs": [] },
    ] });
    let ts = [
        term(json!({ "argv": ["claude"], "name": "sb-1a2b",
                     "labels": { "herdr.space": "sp1", "herdr.tab": "tb1" } })),
        term(json!({ "address": "t2", "argv": ["claude"],
                     "labels": { "herdr.space": "sp1", "herdr.tab": "tb1" } })),
        term(json!({ "address": "t3", "argv": ["codex"],
                     "labels": { "herdr.space": "sp2", "herdr.tab": "tb2" } })),
        term(json!({ "address": "t4", "argv": ["claude"], "parent": "t3" })),
        term(json!({ "address": "t5", "argv": ["claude"] })),
    ];
    let got: Vec<(String, String, String)> = list(&ts, &doc)
        .into_iter()
        .map(|f| (f.target, f.name.unwrap(), f.space.unwrap()))
        .collect();
    let want = [
        ("t1", "sb-1a2b", "Switchboard"),
        ("t2", "fix login", "Switchboard"),
        ("t3", "t3", "other"),
        ("t4", "t4", "other"),
        ("t5", "t5", "core"),
    ]
    .map(|(a, b, c)| (a.to_owned(), b.to_owned(), c.to_owned()));
    assert_eq!(got, want);
}

#[test]
fn an_unnamed_space_is_called_after_where_its_first_terminal_is_not_its_stale_folder() {
    // As seen on the owner's machine: the space shows as "switchboard" in the app, but its saved
    // folder is another project's.
    let doc = json!({ "spaces": [
        { "id": "sp1", "name": "", "folder": "/Users/o/work/manual-integrations",
          "tabs": [
            { "id": "tb1", "name": "", "root": { "content": { "terminal": "t9" } } },
            { "id": "tb2", "name": "options-test", "root": { "content": { "terminal": "t2" } } },
          ] },
    ] });
    let ts = [
        term(
            json!({ "address": "t9", "argv": ["claude"], "cwd": "/Users/o/personal/switchboard",
                     "foreground": { "pid": 9, "name": "2.1.325", "argv": ["claude"],
                                     "cwd": "/Users/o/personal/switchboard" },
                     "labels": { "herdr.space": "sp1", "herdr.tab": "tb1" } }),
        ),
        term(
            json!({ "address": "t2", "argv": ["claude"], "cwd": "/Users/o/elsewhere",
                     "labels": { "herdr.space": "sp1", "herdr.tab": "tb2" } }),
        ),
    ];
    let spaces: Vec<(String, String)> = list(&ts, &doc)
        .into_iter()
        .map(|f| (f.target, f.space.unwrap()))
        .collect();
    assert_eq!(
        spaces,
        [
            ("t9".into(), "switchboard".into()),
            ("t2".into(), "switchboard".into())
        ]
    );
}

#[test]
fn identity_is_core_address_pid_and_start() {
    let f = &list(&[term(claude("blocked"))], &Value::Null)[0];
    assert_eq!(f.identity.as_deref(), Some("core-a:t1:100:5"));
    assert_eq!(f.pids, [101, 100]);
    assert_eq!(f.cwd.as_deref(), Some("/w/live"));
    let other = &list(
        &[term(json!({ "argv": ["claude"], "pid": 200 }))],
        &Value::Null,
    )[0];
    assert_ne!(f.identity, other.identity);
}

async fn until(f: impl Fn() -> bool) {
    for _ in 0..300 {
        if f() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
}

fn core_with(terminals: Vec<Terminal>) -> Arc<Core> {
    let core = Core::new("/nonexistent/core.sock");
    core.up("core-a");
    core.install(4, terminals);
    core
}

fn delta(seq: u64, change: Change) -> Item {
    Item::Delta {
        seq,
        terminal: Some("t1".into()),
        change,
    }
}

#[test]
fn the_mirror_applies_changes_in_order_and_resyncs_on_any_gap() {
    let core = core_with(vec![term(claude("idle"))]);
    let status = |state: &str| Change::Status {
        status: Some(crate::hc::types::Status {
            state: state.into(),
            app: Some("claude-code".into()),
            ..Default::default()
        }),
        reported: Some(true),
        status_ms: Some(9),
    };
    assert_eq!(core.apply(delta(5, status("working"))), Applied::Changed);
    assert_eq!(core.facts().unwrap()[0].status, AgentStatus::Working);
    assert_eq!(
        core.apply(delta(5, status("idle"))),
        Applied::Nothing,
        "old"
    );
    assert_eq!(core.apply(Item::Heartbeat(5)), Applied::Nothing);
    assert_eq!(core.apply(delta(7, status("idle"))), Applied::Resync, "gap");
    assert!(core.facts().is_none(), "not synced");

    let core = core_with(vec![term(claude("idle"))]);
    assert_eq!(core.apply(Item::Heartbeat(6)), Applied::Resync);
    let core = core_with(vec![term(claude("idle"))]);
    assert_eq!(core.apply(Item::Missed), Applied::Resync);
    let core = core_with(vec![term(claude("idle"))]);
    let restarted = Change::Other {
        kind: "restarted".into(),
        data: json!({}),
    };
    assert_eq!(core.apply(delta(5, restarted)), Applied::Resync);
}

#[test]
fn claude_clearing_its_status_keeps_its_kind_and_a_shell_in_front_ends_it() {
    let core = core_with(vec![term(json!({ "reported": true,
        "status": { "state": "working", "app": "claude-code" } }))]);
    let cleared = Change::Status {
        status: None,
        reported: Some(true),
        status_ms: None,
    };
    core.apply(delta(5, cleared));
    let f = core.facts().unwrap();
    assert_eq!(
        (f[0].kind.as_deref(), f[0].status),
        (Some("claude"), AgentStatus::Unknown)
    );
    let shell = Change::Foreground(Some(crate::hc::types::Foreground {
        pid: 300,
        name: "zsh".into(),
        argv: vec!["zsh".into()],
        cwd: "/".into(),
    }));
    core.apply(delta(6, shell));
    assert!(core.facts().unwrap().is_empty(), "the agent left");
}

#[test]
fn ended_terminals_leave_and_new_ones_join() {
    let core = core_with(vec![term(claude("idle"))]);
    let mut t2 = term(claude("idle"));
    t2.address = "t2".into();
    core.apply(Item::Delta {
        seq: 5,
        terminal: Some("t2".into()),
        change: Change::Created(Box::new(t2)),
    });
    assert_eq!(core.facts().unwrap().len(), 2);
    core.apply(delta(6, Change::Finished));
    let left: Vec<String> = core
        .facts()
        .unwrap()
        .into_iter()
        .map(|f| f.target)
        .collect();
    assert_eq!(left, ["t2"]);
}

#[test]
fn another_core_on_the_socket_drops_the_mirror() {
    let core = core_with(vec![term(claude("idle"))]);
    assert!(!core.up("core-a"));
    assert!(core.up("core-b"));
    assert!(core.facts().is_none());
}

#[tokio::test]
async fn a_new_codex_working_at_its_composer_shows_idle() {
    let fake = FakeCore::start();
    let codex_screen = std::fs::read_to_string(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/codex-0.163.0/ready.txt"
    ))
    .unwrap();
    let a = fake.add(json!({ "argv": ["codex"], "turn": 0, "reported": true,
                             "status": { "state": "working", "app": "codex" } }));
    fake.set_screen(&a, &codex_screen);
    let b = fake.add(json!({ "argv": ["codex"], "turn": 1, "reported": true,
                             "status": { "state": "working", "app": "codex" } }));
    fake.set_screen(&b, &codex_screen);
    let core = Core::new(&fake.socket);
    let facts = core.agents().await.unwrap();
    let got: Vec<(String, AgentStatus)> = facts.into_iter().map(|f| (f.target, f.status)).collect();
    assert_eq!(
        got,
        [(a, AgentStatus::Idle), (b, AgentStatus::Working)],
        "only before its first turn"
    );
}

#[tokio::test]
async fn following_mirrors_the_core_resubscribes_and_notices_a_new_core() {
    let fake = FakeCore::start();
    fake.add(json!({ "argv": ["claude"] }));
    fake.set_doc(json!({ "spaces": [] }));
    let core = Core::new(&fake.socket);
    let (changes, replaced) = (Arc::new(AtomicUsize::new(0)), Arc::new(AtomicUsize::new(0)));
    let (c, r) = (changes.clone(), replaced.clone());
    let task = tokio::spawn(follow(
        core.clone(),
        move || {
            c.fetch_add(1, Ordering::Relaxed);
        },
        move || {
            r.fetch_add(1, Ordering::Relaxed);
        },
    ));
    until(|| core.is_synced()).await;
    assert_eq!(core.facts().unwrap().len(), 1);

    fake.create(json!({ "argv": ["codex"] }));
    until(|| core.facts().is_some_and(|f| f.len() == 2)).await;
    assert_eq!(core.facts().unwrap().len(), 2);
    assert!(changes.load(Ordering::Relaxed) >= 2);

    // A dropped stream is followed again from a fresh snapshot.
    fake.drop_subscribers();
    fake.add(json!({ "argv": ["claude"] }));
    until(|| core.facts().is_some_and(|f| f.len() == 3)).await;
    assert_eq!(core.facts().unwrap().len(), 3);
    until(|| fake.subscribers() == 1).await;

    // A gap resubscribes too.
    fake.skip_seq();
    fake.create(json!({ "argv": ["claude"] }));
    until(|| core.facts().is_some_and(|f| f.len() == 4)).await;
    assert_eq!(core.facts().unwrap().len(), 4);

    fake.set_core_id("core-b");
    fake.drop_subscribers();
    until(|| replaced.load(Ordering::Relaxed) == 1).await;
    assert_eq!(replaced.load(Ordering::Relaxed), 1);
    assert_eq!(core.core_id().as_deref(), Some("core-b"));
    task.abort();
}

#[test]
fn the_program_is_found_through_env() {
    let p = |a: &[&str]| program(&a.iter().map(|s| (*s).to_owned()).collect::<Vec<_>>());
    assert_eq!(
        p(&[
            "/usr/bin/env",
            "-u",
            "A",
            "-u",
            "B",
            "X=1",
            "/bin/codex",
            "--no-daemon"
        ]),
        "codex"
    );
    assert_eq!(p(&["claude"]), "claude");
    assert_eq!(p(&[]), "");
}

#[test]
fn only_the_key_paths_send_keys_to_core_terminals() {
    let dir = concat!(env!("CARGO_MANIFEST_DIR"), "/src");
    let mut offenders = Vec::new();
    let mut stack = vec![std::path::PathBuf::from(dir)];
    while let Some(p) = stack.pop() {
        for entry in std::fs::read_dir(&p).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                stack.push(path);
            } else if path.extension().is_some_and(|e| e == "rs") {
                let text = std::fs::read_to_string(&path).unwrap();
                let rel = path
                    .strip_prefix(dir)
                    .unwrap()
                    .to_string_lossy()
                    .into_owned();
                let calls = text.matches(concat!(".send", "_keys(")).count();
                let allowed = ["runner/codex.rs", "runner/dialogs.rs"];
                if calls > 0 && !allowed.contains(&rel.as_str()) && !rel.starts_with("hc/") {
                    offenders.push(rel.clone());
                }
                // Nothing asks the core to type into a blocked program.
                if text.contains(concat!("\"for", "ce\"")) && !rel.starts_with("hc/") {
                    offenders.push(format!("{rel}: force"));
                }
            }
        }
    }
    assert!(offenders.is_empty(), "{offenders:?}");
}

#[tokio::test]
async fn codexs_deny_esc_goes_to_its_core_terminal() {
    use crate::cli::Agent;
    use crate::runner::backend::Server;
    use crate::runner::codex::{CodexPane, FallbackOutcome, deny_dialog};
    let fixture = |name: &str| {
        std::fs::read_to_string(format!(
            "{}/tests/fixtures/codex-0.163.0/{name}.txt",
            env!("CARGO_MANIFEST_DIR")
        ))
        .unwrap()
    };
    let fake = FakeCore::start();
    let t = fake.add(json!({ "argv": ["codex"], "name": "sb-1a2b", "reported": true,
                             "status": { "state": "blocked", "app": "codex", "kind": "permission" } }));
    fake.queue_screens(
        &t,
        vec![fixture("permission-command"), fixture("after-deny-command")],
    );
    let core = Core::new(&fake.socket);
    core.up("core-a");
    let pane = CodexPane::on(Server::Core(core), Agent::Codex, "sb-1a2b").unwrap();
    assert!(matches!(
        deny_dialog(&pane).await,
        FallbackOutcome::Denied { .. }
    ));
    let keys: Vec<Value> = fake
        .requests()
        .into_iter()
        .filter(|q| q["method"] == "terminal.keys")
        .map(|q| q["params"].clone())
        .collect();
    assert_eq!(keys, [json!({ "terminal": t, "keys": ["esc"] })]);
}

/// Screens read with `terminal.read` from core terminals.
fn core_fixture(path: &str) -> String {
    std::fs::read_to_string(format!(
        "{}/tests/fixtures/dialogs/core/{path}.txt",
        env!("CARGO_MANIFEST_DIR")
    ))
    .unwrap()
}

#[test]
fn dialogs_read_from_core_terminals_are_recognised() {
    use crate::agents::dialog::{DialogKind, Recognised};
    use crate::runner::dialogs::recognise;
    let cases = [
        (
            "claude",
            "claude-2.1.322/question-single",
            DialogKind::Question,
            5,
            None,
        ),
        (
            "claude",
            "claude-2.1.322/permission-bash",
            DialogKind::Permission,
            4,
            Some(4),
        ),
        ("claude", "claude-2.1.322/plan", DialogKind::Plan, 3, None),
        (
            "codex",
            "codex-0.167.0/permission-command",
            DialogKind::Permission,
            3,
            Some(3),
        ),
    ];
    for (kind, path, want, rows, deny) in cases {
        let Recognised::Dialog(d) = recognise(Some(kind), &core_fixture(path)) else {
            panic!("{path} not recognised");
        };
        assert_eq!(
            (d.kind, d.options.len(), d.selected),
            (want, rows, 0),
            "{path}"
        );
        assert!(d.actionable, "{path}");
        assert_eq!(
            d.options.iter().find(|o| o.is_deny).map(|o| o.number),
            deny,
            "{path}"
        );
    }
}

#[tokio::test]
async fn agents_in_tabs_of_the_same_name_get_names_of_their_own() {
    use crate::runner::backend::Server;
    let fake = FakeCore::start();
    fake.set_doc(json!({ "spaces": [
        { "id": "sp1", "name": "a", "tabs": [ { "id": "tb1", "name": "orch" } ] },
        { "id": "sp2", "name": "b", "tabs": [ { "id": "tb2", "name": "orch" },
                                              { "id": "tb3", "name": "orch-t1" } ] },
    ] }));
    let tab = |tab: &str| json!({ "argv": ["claude"], "labels": { "herdr.tab": tab } });
    let (t1, t2) = (fake.add(tab("tb1")), fake.add(tab("tb2")));
    let core = Core::new(&fake.socket);
    core.up("core-a");
    let names: Vec<String> = core
        .agents()
        .await
        .unwrap()
        .into_iter()
        .map(|f| f.local_name())
        .collect();
    assert_eq!(names, [format!("orch-{t1}"), format!("orch-{t2}")]);

    let server = Server::Core(core.clone());
    server.prompt(&format!("orch-{t2}"), "hi").await.unwrap();
    server.prompt(&format!("orch-{t1}"), "hi").await.unwrap();
    let sent: Vec<Value> = fake
        .requests()
        .into_iter()
        .filter(|q| q["method"] == "terminal.send")
        .map(|q| q["params"]["terminal"].clone())
        .collect();
    assert_eq!(sent, [json!(t2), json!(t1)]);

    // A tab named like another's listing makes that name ambiguous: refused, nothing sent.
    fake.add(tab("tb3"));
    let e = server
        .prompt(&format!("orch-{t1}"), "hi")
        .await
        .unwrap_err();
    assert!(e.is_not_found(), "{e}");
    assert_eq!(
        fake.methods()
            .iter()
            .filter(|m| *m == "terminal.send")
            .count(),
        2
    );
}

#[tokio::test]
async fn a_prompt_goes_only_to_the_process_the_mirror_lists() {
    use crate::runner::backend::Server;
    let fake = FakeCore::start();
    let a = fake.add(json!({ "argv": ["claude"], "name": "a" }));
    let b = fake.add(json!({ "argv": ["claude"], "name": "b" }));
    let core = Core::new(&fake.socket);
    core.up("core-a");
    core.install(0, fake.client().list().await.unwrap());
    // Without events: a shell back in front of one, another process in the other.
    fake.patch(
        &a,
        json!({ "foreground": { "pid": 7, "name": "zsh", "argv": ["zsh"], "cwd": "/" } }),
    );
    fake.patch(&b, json!({ "pid": 4242 }));
    assert_eq!(
        core.facts().unwrap().len(),
        2,
        "the mirror still lists both"
    );
    let server = Server::Core(core);
    for name in ["a", "b"] {
        let e = server.prompt(name, "hi").await.unwrap_err();
        assert!(e.is_not_found(), "{name}: {e}");
    }
    assert!(!fake.methods().contains(&"terminal.send".to_owned()));
}

#[tokio::test]
async fn a_silent_stream_is_followed_again_though_the_core_answers() {
    let fake = FakeCore::start();
    let core = Core::new(&fake.socket);
    let silence = Duration::from_millis(600);
    // Presence ticks well inside the silence window.
    let task = tokio::spawn(follow_every(
        core.clone(),
        Duration::from_millis(50),
        silence,
        || {},
        || {},
    ));
    until(|| core.is_synced()).await;
    let subscribes = || {
        fake.methods()
            .iter()
            .filter(|m| *m == "events.subscribe")
            .count()
    };
    // Heartbeats keep it alive past the window.
    for _ in 0..12 {
        fake.heartbeat();
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    assert_eq!(subscribes(), 1);
    let quiet = tokio::time::Instant::now();
    until(|| subscribes() == 2).await;
    assert_eq!(subscribes(), 2, "resubscribed after the silence");
    assert!(quiet.elapsed() >= silence - Duration::from_millis(150));
    assert!(fake.hellos() > 10, "the core kept answering");
    task.abort();
}

#[test]
fn a_message_is_signed_with_its_terminals_agent_else_its_name() {
    let core = Core::new("/nonexistent");
    core.up("core-a");
    core.install(
        1,
        vec![
            term(json!({ "address": "t1", "name": "planner", "reported": true,
                         "status": { "state": "idle", "app": "claude-code" } })),
            term(json!({ "address": "t2", "name": "notes", "argv": ["/bin/zsh"],
                         "foreground": { "pid": 9, "name": "-zsh", "argv": ["zsh"], "cwd": "/" } })),
            term(json!({ "address": "t3", "argv": ["/bin/zsh"],
                         "foreground": { "pid": 9, "name": "-zsh", "argv": ["zsh"], "cwd": "/" } })),
        ],
    );
    assert_eq!(core.sender("t1").as_deref(), Some("planner"));
    assert_eq!(
        core.sender("t2").as_deref(),
        Some("notes"),
        "a shell by its name"
    );
    assert_eq!(core.sender("t3"), None, "nothing to call it");
    assert_eq!(core.sender("t9"), None);
    assert_eq!(
        core.sender("vps/t1"),
        None,
        "another machine's is never looked up"
    );
}
