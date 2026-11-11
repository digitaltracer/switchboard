//! Checks the herdr-core methods, params and result fields Switchboard uses against the vendored
//! `hc schema` (herdr-core 0.1.20, protocol generation 1).

use serde_json::Value;
use switchboard::hc::GENERATION;

fn schema() -> Value {
    let text = std::fs::read_to_string(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/hc-api-schema.json"
    ))
    .unwrap();
    serde_json::from_str(&text).unwrap()
}

fn method<'a>(s: &'a Value, name: &str) -> &'a Value {
    s["methods"]
        .as_array()
        .unwrap()
        .iter()
        .find(|m| m["name"] == name)
        .unwrap_or_else(|| panic!("no method {name}"))
}

fn component<'a>(s: &'a Value, name: &str) -> &'a Value {
    let c = &s["components"]["schemas"][name];
    assert!(c.is_object(), "no schema {name}");
    c
}

fn result_name(m: &Value) -> String {
    m["result"]["schema"]["$ref"]
        .as_str()
        .unwrap()
        .rsplit('/')
        .next()
        .unwrap()
        .to_owned()
}

fn assert_fields(s: &Value, name: &str, fields: &[&str]) {
    let props = component(s, name)["properties"].as_object().unwrap();
    for f in fields {
        assert!(props.contains_key(*f), "{name} has no field {f}");
    }
}

/// Each method Switchboard calls, the params it sends, and the result fields it reads.
const USED: &[(&str, &[&str], &[&str])] = &[
    (
        "hello",
        &["generation"],
        &["generation", "version", "core", "methods", "features"],
    ),
    ("terminal.list", &["all", "labels"], &["terminals"]),
    ("terminal.get", &["terminal"], &[]),
    (
        "terminal.read",
        &["terminal", "lines"],
        &["text", "complete", "how"],
    ),
    ("terminal.send", &["terminal", "text"], &["turn", "status"]),
    ("terminal.keys", &["terminal", "keys"], &["turn", "status"]),
    (
        "terminal.run",
        &["argv", "cwd", "name", "labels", "keep", "no_parent"],
        &["address"],
    ),
    ("terminal.kill", &["terminal"], &[]),
    ("terminal.rm", &["terminal"], &["removed"]),
    ("terminal.label", &["terminal", "labels"], &[]),
    ("doc.get", &["name"], &["value", "version"]),
    ("doc.set", &["name", "value", "version"], &["version"]),
    ("events.subscribe", &[], &["seq", "terminals"]),
];

#[test]
fn every_method_param_and_result_field_used_exists() {
    let s = schema();
    for (name, params, fields) in USED {
        let m = method(&s, name);
        let known: Vec<&str> = m["params"]
            .as_array()
            .unwrap()
            .iter()
            .map(|p| p["name"].as_str().unwrap())
            .collect();
        for p in *params {
            assert!(known.contains(p), "{name} takes no param {p}");
        }
        assert_fields(&s, &result_name(m), fields);
    }
}

#[test]
fn the_records_read_have_the_fields_read() {
    let s = schema();
    assert_fields(
        &s,
        "Terminal",
        &[
            "address",
            "pid",
            "created_ms",
            "state",
            "argv",
            "cwd",
            "name",
            "title",
            "labels",
            "reported",
            "status",
            "status_ms",
            "turn",
            "foreground",
            "parent",
        ],
    );
    assert_fields(&s, "Status", &["state", "app", "kind", "msg"]);
    assert_fields(&s, "Foreground", &["pid", "name", "argv", "cwd"]);
    assert_fields(&s, "CoreInfo", &["id", "name"]);
    // The result types the client deserializes are terminals.
    for m in ["terminal.get", "terminal.kill", "terminal.label"] {
        assert_eq!(result_name(method(&s, m)), "Terminal");
    }
    assert_eq!(result_name(method(&s, "terminal.run")), "RunResult");
    assert_fields(&s, "RunResult", &["address", "pid", "created_ms", "labels"]);
}

#[test]
fn the_error_kinds_relied_on_are_documented() {
    let s = schema();
    let kinds = |name: &str| -> Vec<String> {
        method(&s, name)["errors"]
            .as_array()
            .unwrap()
            .iter()
            .map(|e| e["data"]["kind"].as_str().unwrap().to_owned())
            .collect()
    };
    assert!(kinds("hello").contains(&"unsupported_generation".to_owned()));
    for k in ["blocked", "not_found", "finished"] {
        assert!(kinds("terminal.send").contains(&k.to_owned()), "send: {k}");
    }
    assert!(kinds("terminal.keys").contains(&"invalid_params".to_owned()));
    assert!(kinds("doc.set").contains(&"conflict".to_owned()));
    assert!(kinds("terminal.rm").contains(&"live".to_owned()));
}

#[test]
fn the_vendored_schema_is_generation_one_and_has_no_force_use() {
    let s = schema();
    assert!(
        s["info"]["description"]
            .as_str()
            .unwrap()
            .contains(&format!("generation {GENERATION}"))
    );
    // `force` exists on send, and Switchboard never sends it.
    let send = method(&s, "terminal.send");
    assert!(
        send["params"]
            .as_array()
            .unwrap()
            .iter()
            .any(|p| p["name"] == "force")
    );
    assert!(!USED.iter().any(|(_, params, _)| params.contains(&"force")));
}
