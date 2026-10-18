//! Checks Switchboard's hand-written Herdr types against the vendored schema from
//! `herdr api schema` (Herdr 0.9.1, protocol 22).

use serde_json::{Value, json};
use switchboard::herdr::types::*;
use switchboard::herdr::{Mirror, PROTOCOL};

fn schema() -> Value {
    let text = std::fs::read_to_string(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/herdr-api-schema.json"
    ))
    .unwrap();
    serde_json::from_str(&text).unwrap()
}

/// A validator for one named schema in the bundle; its `$ref`s resolve against the bundle root.
fn validator(name: &str) -> jsonschema::Validator {
    let mut root = schema();
    root.as_object_mut()
        .unwrap()
        .insert("$ref".into(), json!(format!("#/schemas/{name}")));
    jsonschema::validator_for(&root).unwrap()
}

fn assert_valid(v: &jsonschema::Validator, instance: &Value) {
    let errors: Vec<String> = v.iter_errors(instance).map(|e| e.to_string()).collect();
    assert!(errors.is_empty(), "{instance}\n{errors:#?}");
}

fn all_methods() -> Vec<Method> {
    vec![
        Method::Ping(Empty {}),
        Method::SessionSnapshot(Empty {}),
        Method::WorktreeCreate(WorktreeCreateParams::default()),
        Method::WorktreeCreate(WorktreeCreateParams {
            cwd: Some("/srv/repo".into()),
            branch: Some("sb/s1".into()),
            base: Some("main".into()),
            path: Some("/srv/wt/s1".into()),
            label: Some("s1".into()),
            focus: false,
        }),
        Method::WorktreeList(WorktreeListParams {
            cwd: Some("/srv/repo".into()),
            workspace_id: None,
        }),
        Method::WorktreeRemove(WorktreeRemoveParams {
            workspace_id: "w2".into(),
        }),
        Method::AgentStart(AgentStartParams {
            name: "sb-s1".into(),
            kind: "claude".into(),
            pane_id: "w2:p1".into(),
            args: vec!["--settings".into(), "/var/lib/sb/s1/settings.json".into()],
            timeout_ms: Some(60_000),
        }),
        Method::AgentPrompt(AgentPromptParams {
            target: "sb-s1".into(),
            text: "fix the bug".into(),
            wait: Some(WaitOptions {
                until: vec![AgentStatus::Working, AgentStatus::Blocked],
                timeout_ms: Some(5_000),
            }),
        }),
        Method::AgentPrompt(AgentPromptParams {
            target: "w2:p1".into(),
            text: "x".into(),
            wait: None,
        }),
        Method::AgentWait(AgentWaitParams {
            target: "sb-s1".into(),
            until: vec![AgentStatus::Idle, AgentStatus::Done, AgentStatus::Blocked],
            timeout_ms: Some(25_000),
        }),
        Method::AgentRead(AgentReadParams {
            target: "sb-s1".into(),
            source: ReadSource::RecentUnwrapped,
            lines: Some(80),
            format: ReadFormat::Text,
            strip_ansi: true,
        }),
        Method::AgentSendKeys(AgentSendKeysParams {
            target: "sb-s1".into(),
            keys: vec!["esc".into()],
        }),
        Method::WorkspaceClose(WorkspaceCloseParams {
            workspace_id: "w2".into(),
        }),
        Method::EventsSubscribe(EventsSubscribeParams {
            subscriptions: {
                let mut s = Subscription::lifecycle();
                s.push(Subscription::WorkspaceCreated);
                s.push(Subscription::WorktreeCreated);
                s.push(Subscription::PaneCreated);
                s.push(Subscription::PaneAgentStatusChanged {
                    pane_id: "w2:p1".into(),
                    agent_status: None,
                });
                s.push(Subscription::PaneAgentStatusChanged {
                    pane_id: "w2:p1".into(),
                    agent_status: Some(AgentStatus::Blocked),
                });
                s
            },
        }),
    ]
}

#[test]
fn vendored_schema_is_the_supported_protocol() {
    assert_eq!(schema()["protocol"], json!(PROTOCOL));
}

#[test]
fn every_request_matches_the_schema() {
    let v = validator("request");
    let methods = all_methods();
    for m in &methods {
        let req = serde_json::to_value(Request {
            id: "sb-1".into(),
            method: m,
        })
        .unwrap();
        assert_eq!(req["method"], m.name());
        assert_valid(&v, &req);
    }
    let names: std::collections::BTreeSet<_> = methods.iter().map(Method::name).collect();
    assert_eq!(names.len(), 12, "a sample for each method Switchboard uses");
}

#[test]
fn schema_rejects_a_bad_request() {
    let v = validator("request");
    assert!(!v.is_valid(&json!({"id": "1", "method": "agent.start", "params": {"name": "x"}})));
    assert!(!v.is_valid(&json!({"id": "1", "method": "no.such_method", "params": {}})));
}

#[test]
fn removal_and_close_never_send_force_or_group() {
    for m in all_methods() {
        let params = serde_json::to_value(&m).unwrap()["params"].clone();
        assert!(
            params.get("force").is_none() && params.get("close_group").is_none(),
            "{params}"
        );
    }
}

fn agent_info(status: &str) -> Value {
    json!({"terminal_id": "term_1", "agent_status": status, "workspace_id": "w2", "tab_id": "w2:t1",
           "pane_id": "w2:p1", "focused": false, "revision": 4, "name": "sb-s1", "agent": "claude",
           "interactive_ready": true, "launch_pending": false, "state_change_seq": 2})
}

fn pane_info(status: &str) -> Value {
    json!({"pane_id": "w2:p1", "terminal_id": "term_1", "workspace_id": "w2", "tab_id": "w2:t1",
           "focused": false, "cwd": "/wt/sb-s1", "agent_status": status, "revision": 0})
}

fn workspace_info() -> Value {
    json!({"workspace_id": "w2", "number": 2, "label": "sb-s1", "focused": false, "pane_count": 1,
           "tab_count": 1, "active_tab_id": "w2:t1", "agent_status": "unknown",
           "worktree": {"repo_key": "/srv/repo/.git", "repo_name": "repo", "repo_root": "/srv/repo",
                        "checkout_path": "/wt/sb-s1", "is_linked_worktree": true}})
}

fn worktree_info() -> Value {
    json!({"path": "/wt/sb-s1", "branch": "sb/s1", "is_bare": false, "is_detached": false,
           "is_prunable": false, "is_linked_worktree": true, "open_workspace_id": "w2", "label": "repo"})
}

/// Shapes observed from Herdr 0.9.1, with paths replaced.
fn sample_results() -> Vec<Value> {
    vec![
        json!({"type": "pong", "version": "0.9.1", "protocol": 22,
               "capabilities": {"live_handoff": true, "detached_server_daemon": false,
                                "endpoint_protocol_generation": 1, "surface_interest": true,
                                "health_check": true}}),
        json!({"type": "session_snapshot", "snapshot": {"version": "0.9.1", "protocol": 22,
               "workspaces": [workspace_info()], "tabs": [], "panes": [pane_info("idle")],
               "layouts": [], "agents": [agent_info("idle")]}}),
        json!({"type": "worktree_created", "workspace": workspace_info(),
               "tab": {"tab_id": "w2:t1", "workspace_id": "w2", "number": 1, "label": "1",
                       "focused": false, "pane_count": 1, "agent_status": "unknown"},
               "root_pane": pane_info("unknown"), "worktree": worktree_info()}),
        json!({"type": "worktree_list",
               "source": {"repo_key": "/srv/repo/.git", "repo_name": "repo", "repo_root": "/srv/repo",
                          "source_checkout_path": "/srv/repo", "source_workspace_id": "w1"},
               "worktrees": [worktree_info()]}),
        json!({"type": "worktree_removed", "workspace_id": "w2", "path": "/wt/sb-s1", "forced": false}),
        json!({"type": "agent_started", "agent": agent_info("idle"), "argv": ["claude"]}),
        json!({"type": "agent_prompted", "agent": agent_info("working")}),
        json!({"type": "agent_info", "agent": agent_info("blocked")}),
        json!({"type": "pane_read", "read": {"pane_id": "w2:p1", "workspace_id": "w2", "tab_id": "w2:t1",
               "source": "recent_unwrapped", "format": "text", "text": "hello", "revision": 9,
               "truncated": false}}),
        json!({"type": "subscription_started"}),
        json!({"type": "ok"}),
    ]
}

#[test]
fn sample_results_match_schema_and_deserialize() {
    let v = validator("success_response");
    for result in sample_results() {
        let resp = json!({"id": "sb-1", "result": result});
        assert_valid(&v, &resp);
        let parsed: Response = serde_json::from_value(resp).unwrap();
        let r = parsed.result.unwrap();
        assert!(
            !matches!(r, ResponseResult::Other),
            "{result} fell through to Other"
        );
    }
}

#[test]
fn error_response_matches_schema() {
    let v = validator("error_response");
    let resp =
        json!({"id": "sb-1", "error": {"code": "agent_blocked", "message": "agent is blocked"}});
    assert_valid(&v, &resp);
    let parsed: Response = serde_json::from_value(resp).unwrap();
    assert_eq!(parsed.error.unwrap().code, "agent_blocked");
}

#[test]
fn events_match_schema_and_parse() {
    let lifecycle = validator("event");
    let scoped = validator("subscription_event");
    let lifecycle_events = vec![
        json!({"event": "workspace_closed", "data": {"type": "workspace_closed", "workspace_id": "w2",
               "workspace": workspace_info()}}),
        json!({"event": "worktree_removed", "data": {"type": "worktree_removed", "workspace_id": "w2",
               "worktree": worktree_info(), "forced": false}}),
        json!({"event": "pane_exited", "data": {"type": "pane_exited", "pane_id": "w2:p1", "workspace_id": "w2"}}),
        json!({"event": "pane_closed", "data": {"type": "pane_closed", "pane_id": "w2:p1", "workspace_id": "w2"}}),
        json!({"event": "pane_created", "data": {"type": "pane_created", "pane": pane_info("unknown")}}),
        json!({"event": "pane_agent_status_changed", "data": {"type": "pane_agent_status_changed",
               "pane_id": "w2:p1", "workspace_id": "w2", "agent_status": "blocked"}}),
    ];
    for e in &lifecycle_events {
        assert_valid(&lifecycle, e);
    }
    let status = json!({"event": "pane.agent_status_changed",
                        "data": {"pane_id": "w2:p1", "workspace_id": "w2", "agent_status": "done"}});
    assert_valid(&scoped, &status);

    let mut mirror = Mirror::new();
    let parsed: Vec<Event> = lifecycle_events
        .into_iter()
        .chain([status])
        .map(|e| {
            serde_json::from_value::<EventFrame>(e)
                .unwrap()
                .into_event()
                .unwrap()
        })
        .collect();
    assert!(
        parsed.iter().all(|e| !matches!(e, Event::Other(_))),
        "{parsed:#?}"
    );
    assert_eq!(
        parsed.last().unwrap(),
        &Event::AgentStatusChanged {
            pane_id: "w2:p1".into(),
            workspace_id: "w2".into(),
            agent_status: AgentStatus::Done
        }
    );
    for e in &parsed {
        mirror.apply(e);
    }
}

#[test]
fn every_agent_status_in_schema_is_known() {
    let statuses = schema()["schemas"]["success_response"]["$defs"]["AgentStatus"]["enum"].clone();
    for s in statuses.as_array().unwrap() {
        let parsed: AgentStatus = serde_json::from_value(s.clone()).unwrap();
        assert_eq!(serde_json::to_value(parsed).unwrap(), *s);
    }
}
