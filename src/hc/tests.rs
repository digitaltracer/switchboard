use std::collections::HashMap;
use std::time::Duration;

use serde_json::{Value, json};

use super::events::{Change, Item};
use super::fake::{FakeCore, Script};
use super::layout::{self, LayoutError, Which};
use super::*;

fn env(pairs: &[(&str, &str)]) -> impl Fn(&str) -> Option<String> + use<> {
    let map: HashMap<String, String> = pairs
        .iter()
        .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
        .collect();
    move |k| map.get(k).cloned()
}

#[test]
fn the_socket_is_found_in_hcs_own_order() {
    let home = Path::new("/home/u");
    let none = |_: &Path| false;
    let all = |_: &Path| true;
    let all_vars = [
        ("HC_SOCKET_PATH", "/a.sock"),
        ("HC_SOCKET", "/b.sock"),
        ("XDG_RUNTIME_DIR", "/run/x"),
        ("XDG_STATE_HOME", "/state"),
        ("HC_SESSION", "work"),
    ];
    assert_eq!(
        find_socket(env(&all_vars), home, 501, all),
        Path::new("/a.sock")
    );
    assert_eq!(
        find_socket(env(&all_vars[1..]), home, 501, all),
        Path::new("/b.sock")
    );
    assert_eq!(
        find_socket(env(&all_vars[2..]), home, 501, all),
        Path::new("/run/x/herdr-core/work.sock")
    );
    assert_eq!(
        find_socket(env(&all_vars[3..]), home, 501, all),
        Path::new("/run/user/501/herdr-core/work.sock")
    );
    assert_eq!(
        find_socket(env(&all_vars[3..]), home, 501, none),
        Path::new("/state/herdr-core/work/work.sock")
    );
    assert_eq!(
        find_socket(env(&[]), home, 501, none),
        Path::new("/home/u/.local/state/herdr-core/default/default.sock")
    );
    // Empty values count as unset.
    assert_eq!(
        find_socket(
            env(&[("HC_SOCKET_PATH", ""), ("HC_SESSION", "")]),
            home,
            1,
            none
        ),
        Path::new("/home/u/.local/state/herdr-core/default/default.sock")
    );
}

#[test]
fn addresses_are_this_machines_only() {
    assert!(is_address("t7") && is_address("t1b") && !is_address("t") && !is_address("w1:p2"));
    assert!(!is_address("vps/t7") && check_local("vps/t7").is_err() && check_local("t7").is_ok());
}

#[tokio::test]
async fn every_call_starts_with_hello_and_reports_the_core() {
    let core = FakeCore::start();
    let c = core.client();
    let hello = c.hello().await.unwrap();
    assert_eq!((hello.generation, hello.core.id.as_str()), (1, "core-a"));
    core.add(json!({ "name": "w" }));
    let list = c.list().await.unwrap();
    assert_eq!(list[0].address, "t1");
    assert_eq!(list[0].foreground.as_ref().unwrap().pid, 1001);
    assert_eq!(core.hellos(), 2);
    assert_eq!(core.methods(), ["terminal.list"]);
}

#[tokio::test]
async fn errors_split_into_not_sent_unknown_and_refused() {
    let core = FakeCore::start();
    let c = core.client().with_timeout(Duration::from_millis(300));
    core.add(json!({}));

    let gone = Client::new(core.socket.with_extension("nope"))
        .list()
        .await
        .unwrap_err();
    assert!(gone.server_gone() && !gone.outcome_unknown());

    core.script("terminal.send", Script::Drop);
    let dropped = c.send("t1", "hi").await.unwrap_err();
    assert!(dropped.outcome_unknown(), "{dropped}");

    core.script("terminal.label", Script::Hang);
    let hung = c.label("t1", &Default::default()).await.unwrap_err();
    assert!(hung.outcome_unknown(), "{hung}");

    core.script("terminal.send", Script::Error("blocked"));
    let refused = c.send("t1", "hi").await.unwrap_err();
    assert_eq!(refused.kind(), Some("blocked"));
    assert!(!refused.outcome_unknown());

    let unknown_key = c.send_keys("t1", &["nope".into()]).await.unwrap_err();
    assert_eq!(unknown_key.kind(), Some("invalid_params"));

    let other_machine = c.send("vps/t1", "hi").await.unwrap_err();
    assert!(matches!(other_machine, HcError::InvalidArgument(_)));
}

#[tokio::test]
async fn a_core_of_another_generation_is_refused_before_any_request() {
    let core = FakeCore::start();
    core.script("terminal.list", Script::Result(json!({ "terminals": [] })));
    // A raw hello at generation 2 is refused by the core itself.
    let e = {
        let c = core.client();
        let (mut s, _) = (
            BufReader::new(UnixStream::connect(c.socket()).await.unwrap()),
            (),
        );
        write_request(&mut s, 9, "hello", json!({ "generation": 2 }))
            .await
            .unwrap();
        let line = read_line(&mut s, "hello").await.unwrap();
        parse_reply("hello", 9, &line).unwrap_err()
    };
    assert_eq!(e.kind(), Some("unsupported_generation"));
    assert!(core.requests().is_empty());
}

#[tokio::test]
async fn the_client_never_starts_a_core() {
    let dir = tempfile::tempdir().unwrap();
    let socket = dir.path().join("default.sock");
    let c = Client::new(&socket);
    for e in [
        c.hello().await.unwrap_err(),
        c.list().await.unwrap_err(),
        c.subscribe().await.err().unwrap(),
    ] {
        assert!(e.server_gone(), "{e}");
    }
    assert!(!socket.exists());
    assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 0);
}

#[tokio::test]
async fn typed_calls_send_what_the_schema_names() {
    let core = FakeCore::start();
    let c = core.client();
    let mut labels = std::collections::BTreeMap::new();
    labels.insert("switchboard.session".to_owned(), "s1".to_owned());
    let t = c
        .run(&RunParams {
            argv: vec!["/usr/bin/env".into(), "claude".into()],
            cwd: "/w".into(),
            name: "sb-s1".into(),
            labels: labels.clone(),
            keep: true,
            no_parent: true,
        })
        .await
        .unwrap();
    core.set_screen(&t.address, "hello");
    assert_eq!(c.read(&t.address, 0).await.unwrap().text, "hello");
    assert_eq!(c.send(&t.address, "go").await.unwrap().turn, 0);
    c.send_keys(&t.address, &["down".into(), "enter".into()])
        .await
        .unwrap();
    assert_eq!(c.list_labelled(&labels).await.unwrap().len(), 1);
    c.kill(&t.address).await.unwrap();
    c.rm(&t.address).await.unwrap();
    assert_eq!(
        c.get(&t.address).await.unwrap_err().kind(),
        Some("not_found")
    );
    let reqs = core.requests();
    assert_eq!(
        reqs[0]["params"],
        json!({ "argv": ["/usr/bin/env", "claude"], "cwd": "/w", "name": "sb-s1",
                "labels": { "switchboard.session": "s1" }, "keep": true, "no_parent": true })
    );
    assert_eq!(reqs[2]["params"], json!({ "terminal": "t1", "text": "go" }));
    assert!(reqs.iter().all(|r| r["params"].get("force").is_none()));
    assert_eq!(reqs[0]["jsonrpc"], "2.0");
}

#[tokio::test]
async fn the_event_stream_gives_the_snapshot_then_changes_in_order() {
    let core = FakeCore::start();
    core.add(json!({ "reported": true, "status": { "state": "idle", "app": "codex" } }));
    let mut s = core.client().subscribe().await.unwrap();
    assert_eq!(s.core_id(), "core-a");
    let Some(Ok(Item::Snapshot { seq: 0, terminals })) = s.next().await else {
        panic!("no snapshot")
    };
    assert_eq!(
        terminals[0].status.as_ref().unwrap().app.as_deref(),
        Some("codex")
    );

    core.change(
        "t1",
        "status",
        json!({ "status": { "state": "blocked", "app": "codex", "kind": "permission" },
                "reported": true, "records": {} }),
    );
    core.change("t1", "status", json!({ "status": null, "reported": true }));
    core.change(
        "t1",
        "foreground",
        json!({ "pid": 7, "name": "zsh", "argv": ["zsh"], "cwd": "/" }),
    );
    core.heartbeat();
    core.emit("doc", None, json!({ "name": "herdr-app", "version": 3 }));
    core.emit("restarted", Some("t1"), json!({ "pid": 9 }));
    core.missed();
    let mut got = Vec::new();
    for _ in 0..7 {
        got.push(s.next().await.unwrap().unwrap());
    }
    let Item::Delta {
        seq: 1,
        change: Change::Status {
            status: Some(st), ..
        },
        ..
    } = &got[0]
    else {
        panic!("{:?}", got[0])
    };
    assert_eq!(
        (st.state.as_str(), st.kind.as_deref()),
        ("blocked", Some("permission"))
    );
    assert!(matches!(
        &got[1],
        Item::Delta {
            seq: 2,
            change: Change::Status {
                status: None,
                reported: Some(true),
                ..
            },
            ..
        }
    ));
    assert!(
        matches!(&got[2], Item::Delta { change: Change::Foreground(Some(f)), .. } if f.name == "zsh")
    );
    assert_eq!(got[3], Item::Heartbeat(3));
    assert!(
        matches!(&got[4], Item::Delta { terminal: None, change: Change::Doc { name }, .. } if name == "herdr-app")
    );
    assert!(
        matches!(&got[5], Item::Delta { change: Change::Other { kind, .. }, .. } if kind == "restarted")
    );
    assert_eq!(got[6], Item::Missed);

    core.drop_subscribers();
    assert!(s.next().await.is_none());
}

#[tokio::test]
async fn a_silent_stream_ends() {
    let core = FakeCore::start();
    let mut s = core
        .client()
        .subscribe()
        .await
        .unwrap()
        .with_silence(Duration::from_millis(100));
    assert!(matches!(s.next().await, Some(Ok(Item::Snapshot { .. }))));
    let t = tokio::time::Instant::now();
    assert!(s.next().await.is_none());
    assert!(t.elapsed() >= Duration::from_millis(100));
}

fn layout_doc() -> Value {
    json!({
        "spaces": [
            { "id": "sp1", "name": "repo", "folder": "/r", "color": "blue",
              "tabs": [ { "id": "tb1", "name": null, "zoom": 2,
                          "root": { "content": { "terminal": "t1" }, "children": [
                              { "content": { "terminal": "t2" } } ] } } ] },
            { "id": "sp2", "folder": "/s", "tabs": [] }
        ],
        "app_owned": { "keep": [1, 2, 3] }
    })
}

#[test]
fn the_layout_is_read_as_spaces_and_tabs() {
    let spaces = types::spaces(&layout_doc());
    assert_eq!(spaces.len(), 2);
    assert_eq!(spaces[0].name.as_deref(), Some("repo"));
    assert_eq!(spaces[0].tabs[0].terminals, ["t1", "t2"]);
    assert_eq!(spaces[1].name, None);
    assert!(types::spaces(&Value::Null).is_empty());
}

#[test]
fn a_rename_changes_one_name_and_keeps_every_other_field() {
    let mut doc = layout_doc();
    layout::set_name(&mut doc, Which::Tab("tb1"), "fix login").unwrap();
    layout::set_name(&mut doc, Which::Space("sp2"), "other").unwrap();
    let mut want = layout_doc();
    want["spaces"][0]["tabs"][0]["name"] = json!("fix login");
    want["spaces"][1]["name"] = json!("other");
    assert_eq!(doc, want);
    assert_eq!(doc.to_string(), want.to_string(), "field order kept");

    for (which, why) in [
        (Which::Tab("vps/tb1"), "other machine"),
        (Which::Tab("tb9"), "missing"),
        (Which::Space(""), "empty"),
    ] {
        assert!(layout::set_name(&mut doc, which, "x").is_err(), "{why}");
    }
    let mut dup = layout_doc();
    dup["spaces"][1]["id"] = json!("sp1");
    assert!(matches!(
        layout::set_name(&mut dup, Which::Space("sp1"), "x"),
        Err(LayoutError::Refused(_))
    ));
    assert!(matches!(
        layout::set_name(&mut Value::Null, Which::Space("sp1"), "x"),
        Err(LayoutError::Missing)
    ));
    let mut big = layout_doc();
    big["app_owned"]["blob"] = json!("x".repeat(layout::MAX_DOC));
    assert!(matches!(
        layout::set_name(&mut big, Which::Space("sp1"), "x"),
        Err(LayoutError::Refused(_))
    ));
}

#[tokio::test]
async fn a_layout_write_is_compare_and_set() {
    let core = FakeCore::start();
    let c = core.client();
    assert!(matches!(
        layout::rename_tab(&c, "tb1", "x").await,
        Err(LayoutError::Missing)
    ));
    core.set_doc(layout_doc());
    layout::rename_tab(&c, "tb1", "fix login").await.unwrap();
    let (doc, version) = core.doc();
    assert_eq!(
        (doc["spaces"][0]["tabs"][0]["name"].as_str(), version),
        (Some("fix login"), 2)
    );
    assert_eq!(doc["app_owned"], json!({ "keep": [1, 2, 3] }));

    // Someone else writes between the read and the write.
    core.script("doc.set", Script::Error("conflict"));
    assert!(matches!(
        layout::rename_space(&c, "sp1", "y").await,
        Err(LayoutError::Conflict)
    ));
}
