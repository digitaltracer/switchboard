use axum::body::Body;
use axum::http::Request as HttpRequest;
use serde_json::{Value, json};
use tokio::sync::mpsc;
use tower::ServiceExt;

use super::*;
use crate::auth::device::tests::Phone;
use crate::auth::device::{approval_message, token_message};
use crate::config::HubConfig;
use crate::hub::service::Clock;
use crate::proto::auth::hash_token;
use crate::proto::{
    ErrorCode, Event, Frame, Hello, Op, Outcome, PROTOCOL_VERSION, ProtoError, Resp,
};
use crate::store::Store;

const SPEC: &str = "api/openapi.json";

#[test]
fn the_committed_spec_matches_the_code() {
    let committed = std::fs::read_to_string(SPEC).unwrap_or_default();
    assert!(
        committed == openapi::spec_json(),
        "{SPEC} is out of date; run `cargo run -- admin openapi > {SPEC}` and commit it"
    );
}

/// Old clients keep working: every operation they call still exists, every field they read is
/// still sent with the same type, and no request gains a field they don't send.
#[test]
fn old_client_specs_are_still_served() {
    let new: Value = serde_json::from_str(&openapi::spec_json()).unwrap();
    for entry in std::fs::read_dir("tests/fixtures/app-v1/specs").unwrap() {
        let path = entry.unwrap().path();
        let old: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        let problems = compat_problems(&old, &new);
        assert!(problems.is_empty(), "{}: {problems:#?}", path.display());
    }
}

fn schema_refs(v: &Value, out: &mut Vec<String>) {
    match v {
        Value::Object(m) => {
            if let Some(Value::String(r)) = m.get("$ref") {
                out.push(r.trim_start_matches("#/components/schemas/").to_owned());
            }
            m.values().for_each(|x| schema_refs(x, out));
        }
        Value::Array(a) => a.iter().for_each(|x| schema_refs(x, out)),
        _ => {}
    }
}

fn required(s: &Value) -> Vec<String> {
    s["required"]
        .as_array()
        .map(|a| {
            a.iter()
                .filter_map(|x| x.as_str().map(str::to_owned))
                .collect()
        })
        .unwrap_or_default()
}

fn compat_problems(old: &Value, new: &Value) -> Vec<String> {
    let mut problems = Vec::new();
    let (mut requests, mut responses) = (Vec::new(), Vec::new());
    for (path, ops) in old["paths"].as_object().unwrap() {
        for (method, op) in ops.as_object().unwrap() {
            if new["paths"][path][method].is_null() {
                problems.push(format!("{method} {path} is gone"));
            }
            schema_refs(&op["requestBody"], &mut requests);
            schema_refs(&op["responses"], &mut responses);
        }
    }
    // Follow nested references from response types, e.g. Session to Approval.
    let mut i = 0;
    while i < responses.len() {
        let mut more = Vec::new();
        schema_refs(&old["components"]["schemas"][&responses[i]], &mut more);
        for m in more {
            if !responses.contains(&m) {
                responses.push(m);
            }
        }
        i += 1;
    }
    let schemas = |v: &Value, n: &str| v["components"]["schemas"][n].clone();
    for name in &responses {
        let (o, n) = (schemas(old, name), schemas(new, name));
        if n.is_null() {
            problems.push(format!("response type {name} is gone"));
            continue;
        }
        for field in required(&o) {
            if !required(&n).contains(&field) {
                problems.push(format!("{name}.{field} is no longer always sent"));
            }
            if o["properties"][&field]["type"] != n["properties"][&field]["type"] {
                problems.push(format!("{name}.{field} changed type"));
            }
        }
    }
    for name in &requests {
        let (o, n) = (schemas(old, name), schemas(new, name));
        for field in required(&n) {
            if !required(&o).contains(&field) {
                problems.push(format!("request {name} now requires {field}"));
            }
        }
    }
    problems
}

// An in-process hub with the app routes and a scripted runner.

struct Rig {
    app: Router,
    svc: Arc<SessionService>,
    devices: Arc<Devices>,
    runner: mpsc::Receiver<Frame>,
    generation: u64,
}

fn config(extra: &str) -> HubConfig {
    HubConfig::parse(&format!(
        r#"
        public_url = "https://hub.example.com"
        listen = "127.0.0.1:0"
        database = "/var/lib/switchboard/hub.db"
        default_machine = "mac"
        [[machines]]
        id = "mac"
        token_hash = "{}"
        [[projects]]
        id = "switchboard"
        description = "this repo"
        repos = {{ mac = "/Users/me/switchboard" }}
        {extra}
        "#,
        hash_token("t")
    ))
    .unwrap()
}

async fn rig_with(extra: &str) -> Rig {
    let store = Store::open_in_memory().await.unwrap();
    let svc = SessionService::new(config(extra), store.clone(), Clock::System);
    let devices = Arc::new(Devices::new(store, Clock::System));
    let hello = Hello {
        machine_id: "mac".into(),
        protocol_version: PROTOCOL_VERSION,
        runner_version: "test".into(),
        herdr_version: None,
        sessions: vec![],
    };
    let (generation, runner) = svc.runner_connected("mac", hello).await;
    let app = routes(AppState {
        svc: svc.clone(),
        devices: devices.clone(),
    });
    Rig {
        app,
        svc,
        devices,
        runner,
        generation,
    }
}

async fn rig() -> Rig {
    rig_with("").await
}

struct Reply {
    status: StatusCode,
    body: Value,
}

impl Rig {
    async fn call(
        &self,
        method: &str,
        path: &str,
        token: Option<&str>,
        body: Option<Value>,
    ) -> Reply {
        let mut req = HttpRequest::builder()
            .method(method)
            .uri(format!("{PREFIX}{path}"))
            .header("content-type", "application/json")
            .header(BUILD_HEADER, "1");
        if let Some(t) = token {
            req = req.header("authorization", format!("Bearer {t}"));
        }
        let body = body.map_or(Body::empty(), |b| Body::from(b.to_string()));
        let resp = self
            .app
            .clone()
            .oneshot(req.body(body).unwrap())
            .await
            .unwrap();
        let status = resp.status();
        let bytes = axum::body::to_bytes(resp.into_body(), 1 << 20)
            .await
            .unwrap();
        Reply {
            status,
            body: serde_json::from_slice(&bytes).unwrap_or(Value::Null),
        }
    }

    /// A request in the background, for one the scripted runner has to answer first.
    fn call_later(
        &self,
        method: &'static str,
        path: &str,
        token: &str,
        body: Value,
    ) -> tokio::task::JoinHandle<Reply> {
        let app = self.app.clone();
        let (path, token) = (path.to_owned(), token.to_owned());
        tokio::spawn(async move {
            let req = HttpRequest::builder()
                .method(method)
                .uri(format!("{PREFIX}{path}"))
                .header("content-type", "application/json")
                .header("authorization", format!("Bearer {token}"))
                .body(Body::from(body.to_string()))
                .unwrap();
            let resp = app.oneshot(req).await.unwrap();
            let status = resp.status();
            let bytes = axum::body::to_bytes(resp.into_body(), 1 << 20)
                .await
                .unwrap();
            Reply {
                status,
                body: serde_json::from_slice(&bytes).unwrap_or(Value::Null),
            }
        })
    }

    async fn req(&mut self) -> crate::proto::Req {
        match tokio::time::timeout(Duration::from_secs(5), self.runner.recv())
            .await
            .unwrap()
            .unwrap()
        {
            Frame::Req(r) => r,
            other => panic!("unexpected {other:?}"),
        }
    }

    async fn ok(&self, id: u64, v: Value) {
        self.svc
            .runner_frame(
                "mac",
                self.generation,
                Frame::Resp(Resp {
                    id,
                    outcome: Outcome::Ok(v),
                }),
            )
            .await;
    }

    async fn err(&self, id: u64, code: ErrorCode, message: &str) {
        self.svc
            .runner_frame(
                "mac",
                self.generation,
                Frame::Resp(Resp {
                    id,
                    outcome: Outcome::Err(ProtoError::new(code, message)),
                }),
            )
            .await;
    }

    async fn event(&self, e: Event) {
        self.svc
            .runner_frame("mac", self.generation, Frame::Event(e))
            .await;
    }

    /// Pairs `phone` through the API, confirms it as the admin would, and returns (device, token).
    async fn pair(&self, phone: &Phone) -> (String, String) {
        let code = self.devices.issue_code().await.unwrap();
        let r = self
            .call(
                "POST",
                "/pair",
                None,
                Some(
                    json!({"code": code.code, "name": "iPhone", "auth_key": phone.auth.public(),
                            "approval_key": phone.approval.public(), "client_build": 1}),
                ),
            )
            .await;
        assert_eq!(r.status, StatusCode::OK, "{}", r.body);
        let device = r.body["device_id"].as_str().unwrap().to_owned();
        assert_eq!(r.body["state"], "pending");
        let pending = self
            .call("GET", &format!("/pair/{device}"), None, None)
            .await;
        assert_eq!(pending.body["state"], "pending");
        let (devices, d2) = (self.devices.clone(), device.clone());
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(50)).await;
            devices.confirm(&d2, true).await.unwrap();
        });
        let active = self
            .call("GET", &format!("/pair/{device}?wait_ms=5000"), None, None)
            .await;
        assert_eq!(
            active.body["state"], "active",
            "the long-poll sees the confirmation"
        );
        (device.clone(), self.token(phone, &device).await)
    }

    async fn token(&self, phone: &Phone, device: &str) -> String {
        let ch = self
            .call(
                "POST",
                "/auth/challenge",
                None,
                Some(json!({"device_id": device})),
            )
            .await
            .body;
        let msg = token_message(
            ch["hub_id"].as_str().unwrap(),
            device,
            ch["nonce"].as_str().unwrap(),
            ch["expires_at"].as_i64().unwrap(),
        );
        let t = self
            .call(
                "POST",
                "/auth/token",
                None,
                Some(json!({"device_id": device, "nonce": ch["nonce"],
                            "signature": phone.auth.sign(&msg), "message_version": 1})),
            )
            .await;
        assert_eq!(t.status, StatusCode::OK, "{}", t.body);
        t.body["access_token"].as_str().unwrap().to_owned()
    }

    /// Starts a session through the API with the runner accepting it.
    async fn start(&mut self, token: &str, request_id: &str) -> String {
        let app = self.app.clone();
        let token = token.to_owned();
        let body = json!({"project_id": "switchboard", "prompt": format!("task {request_id}"),
                          "request_id": request_id});
        let call = tokio::spawn(async move {
            let req = HttpRequest::builder()
                .method("POST")
                .uri(format!("{PREFIX}/sessions"))
                .header("content-type", "application/json")
                .header("authorization", format!("Bearer {token}"))
                .body(Body::from(body.to_string()))
                .unwrap();
            let resp = app.oneshot(req).await.unwrap();
            let bytes = axum::body::to_bytes(resp.into_body(), 1 << 20)
                .await
                .unwrap();
            serde_json::from_slice::<Value>(&bytes).unwrap()
        });
        let req = self.req().await;
        let Op::StartSession {
            session_id, branch, ..
        } = req.op
        else {
            panic!("expected a start");
        };
        self.ok(
            req.id,
            json!({"session_id": session_id, "branch": branch, "worktree_path": "/wt",
                               "workspace_id": "w1", "pane_id": "w1:p1"}),
        )
        .await;
        let started = call.await.unwrap();
        assert_eq!(started["outcome"], "started", "{started}");
        assert_eq!(started["session_id"], session_id.as_str());
        session_id
    }

    async fn approval(&self, sid: &str, id: &str, tool: &str, input: Value) {
        self.event(Event::ApprovalRequested {
            session_id: sid.into(),
            approval_id: id.into(),
            tool_name: tool.into(),
            tool_input: input,
            deadline_ms: 280_000,
        })
        .await;
    }
}

#[tokio::test]
async fn meta_is_open_and_everything_else_needs_a_device_token() {
    let r = rig().await;
    let meta = r.call("GET", "/meta", None, None).await;
    assert_eq!(meta.status, StatusCode::OK);
    assert_eq!(meta.body["min_client_build"], MIN_CLIENT_BUILD);
    assert_eq!(
        meta.body["hub_id"],
        r.devices.hub_id().await.unwrap().as_str()
    );
    for path in [
        "/sessions",
        "/projects",
        "/machines",
        "/devices",
        "/sessions/x",
    ] {
        let reply = r.call("GET", path, None, None).await;
        assert_eq!(reply.status, StatusCode::UNAUTHORIZED, "{path}");
        assert_eq!(reply.body["error"]["code"], "invalid_token");
        let reply = r.call("GET", path, Some("made-up"), None).await;
        assert_eq!(reply.status, StatusCode::UNAUTHORIZED, "{path}");
    }
}

#[tokio::test]
async fn builds_older_than_the_minimum_are_told_to_update() {
    let r = rig().await;
    let req = HttpRequest::builder()
        .uri(format!("{PREFIX}/sessions"))
        .header(BUILD_HEADER, "0")
        .body(Body::empty())
        .unwrap();
    let resp = r.app.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::UPGRADE_REQUIRED);
    let meta = HttpRequest::builder()
        .uri(format!("{PREFIX}/meta"))
        .header(BUILD_HEADER, "0")
        .body(Body::empty())
        .unwrap();
    assert_eq!(
        r.app.clone().oneshot(meta).await.unwrap().status(),
        StatusCode::OK
    );
}

#[tokio::test]
async fn pair_list_start_and_read_sessions_through_the_api() {
    let mut r = rig().await;
    let phone = Phone::new();
    let (device, token) = r.pair(&phone).await;
    let t = Some(token.as_str());

    let projects = r.call("GET", "/projects", t, None).await;
    assert_eq!(projects.body[0]["id"], "switchboard");
    let machines = r.call("GET", "/machines", t, None).await;
    assert_eq!(machines.body[0]["status"], "ready");
    assert_eq!(machines.body[0]["default"], true);

    let empty = r.call("GET", "/sessions", t, None).await;
    let rev = empty.body["revision"].as_i64().unwrap();
    assert_eq!(empty.body["sessions"], json!([]));
    let sid = r.start(&token, "rq-1").await;
    let list = r
        .call(
            "GET",
            &format!("/sessions?after_revision={rev}&wait_ms=2000"),
            t,
            None,
        )
        .await;
    assert!(list.body["revision"].as_i64().unwrap() > rev);
    let s = &list.body["sessions"][0];
    assert_eq!(s["id"], sid.as_str());
    assert_eq!(s["label"], format!("switchboard-{}", &sid[..4]));
    assert_eq!(
        (s["lifecycle"].as_str(), s["machine_status"].as_str()),
        (Some("running"), Some("ready"))
    );

    let op = r.call("GET", "/operations/rq-1", t, None).await;
    assert_eq!(
        (op.body["state"].as_str(), op.body["session_id"].as_str()),
        (Some("done"), Some(sid.as_str()))
    );
    let missing = r.call("GET", "/operations/nope", t, None).await;
    assert_eq!(missing.body["error"]["code"], "unknown_operation");

    let detail = r.call("GET", &format!("/sessions/{sid}"), t, None).await;
    let revision = detail.body["revision"].as_i64().unwrap();
    let (app, tok, sid2) = (r.app.clone(), token.clone(), sid.clone());
    let waiter = tokio::spawn(async move {
        let req = HttpRequest::builder()
            .uri(format!(
                "{PREFIX}/sessions/{sid2}?after_revision={revision}&wait_ms=5000"
            ))
            .header("authorization", format!("Bearer {tok}"))
            .body(Body::empty())
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        serde_json::from_slice::<Value>(
            &axum::body::to_bytes(resp.into_body(), 1 << 20)
                .await
                .unwrap(),
        )
        .unwrap()
    });
    tokio::time::sleep(Duration::from_millis(30)).await;
    r.event(Event::AgentStatus {
        session_id: sid.clone(),
        agent_status: "working".into(),
        revision: 0,
    })
    .await;
    assert_eq!(waiter.await.unwrap()["agent_status"], "working");

    let app = r.app.clone();
    let (tok, sid2) = (token.clone(), sid.clone());
    let read = tokio::spawn(async move {
        let req = HttpRequest::builder()
            .uri(format!("{PREFIX}/sessions/{sid2}/output?lines=10"))
            .header("authorization", format!("Bearer {tok}"))
            .body(Body::empty())
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        serde_json::from_slice::<Value>(
            &axum::body::to_bytes(resp.into_body(), 1 << 20)
                .await
                .unwrap(),
        )
        .unwrap()
    });
    let req = r.req().await;
    assert!(matches!(req.op, Op::Read { lines: 10, .. }));
    r.ok(
        req.id,
        json!({"text": "export API_TOKEN=abc123secret", "truncated": false}),
    )
    .await;
    let out = read.await.unwrap();
    assert!(
        !out["text"].as_str().unwrap().contains("abc123secret"),
        "{out}"
    );

    let muted = r
        .call(
            "PUT",
            &format!("/sessions/{sid}/mute"),
            t,
            Some(json!({"muted": true})),
        )
        .await;
    assert_eq!(
        (
            muted.body["muted"].as_bool(),
            muted.body["notifies"].as_bool()
        ),
        (Some(true), Some(false))
    );

    let devices = r.call("GET", "/devices", t, None).await;
    assert_eq!(devices.body[0]["id"], device.as_str());
    assert_eq!(devices.body[0]["this_device"], true);
    let push = r
        .call(
            "PUT",
            "/devices/self/push",
            t,
            Some(json!({"token": "AB12", "environment": "sandbox"})),
        )
        .await;
    assert_eq!(push.status, StatusCode::NO_CONTENT);
    let d = r.devices.device(&device).await.unwrap().unwrap();
    assert_eq!(d.push_token.as_deref(), Some("ab12"));
}

#[tokio::test]
async fn deny_needs_only_a_token_but_approve_needs_the_approval_keys_signature() {
    let mut r = rig().await;
    let phone = Phone::new();
    let (device, token) = r.pair(&phone).await;
    let t = Some(token.as_str());
    let sid = r.start(&token, "rq-2").await;
    r.approval(&sid, "a1", "Bash", json!({"command": "rm -rf build"}))
        .await;
    let s = r.call("GET", &format!("/sessions/{sid}"), t, None).await;
    let a = &s.body["pending_approvals"][0];
    assert_eq!(
        (a["kind"].as_str(), a["input_text"].as_str()),
        (Some("permission"), Some("rm -rf build"))
    );

    // A stolen token alone can't approve.
    let unsigned = r
        .call(
            "POST",
            "/approvals/a1/decision",
            t,
            Some(json!({"decision": "allow"})),
        )
        .await;
    assert_eq!(unsigned.status, StatusCode::BAD_REQUEST);
    let ch = r
        .call("POST", "/approvals/a1/challenge", t, None)
        .await
        .body;
    let msg = approval_message(
        ch["hub_id"].as_str().unwrap(),
        &device,
        "a1",
        "allow",
        ch["nonce"].as_str().unwrap(),
        ch["expires_at"].as_i64().unwrap(),
    );
    let auth_key_sig = phone.auth.sign(&msg);
    let wrong = r
        .call("POST", "/approvals/a1/decision", t,
              Some(json!({"decision": "allow", "nonce": ch["nonce"], "signature": auth_key_sig, "message_version": 1})))
        .await;
    assert_eq!(wrong.body["error"]["code"], "bad_signature");

    let app = r.app.clone();
    let (tok, body) = (
        token.clone(),
        json!({"decision": "allow", "nonce": ch["nonce"],
        "signature": phone.approval.sign(&msg), "message_version": 1, "request_id": "d-1"}),
    );
    let decide = tokio::spawn(async move {
        let req = HttpRequest::builder()
            .method("POST")
            .uri(format!("{PREFIX}/approvals/a1/decision"))
            .header("content-type", "application/json")
            .header("authorization", format!("Bearer {tok}"))
            .body(Body::from(body.to_string()))
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        serde_json::from_slice::<Value>(
            &axum::body::to_bytes(resp.into_body(), 1 << 20)
                .await
                .unwrap(),
        )
        .unwrap()
    });
    let req = r.req().await;
    assert!(matches!(req.op, Op::Decide { allow: true, .. }));
    r.ok(req.id, json!({})).await;
    let decided = decide.await.unwrap();
    assert_eq!(
        (decided["outcome"].as_str(), decided["delivery"].as_str()),
        (Some("allow"), Some("delivered"))
    );

    r.approval(&sid, "a2", "Bash", json!({"command": "ls"}))
        .await;
    let app = r.app.clone();
    let tok = token.clone();
    let deny = tokio::spawn(async move {
        let req = HttpRequest::builder()
            .method("POST")
            .uri(format!("{PREFIX}/approvals/a2/decision"))
            .header("content-type", "application/json")
            .header("authorization", format!("Bearer {tok}"))
            .body(Body::from(json!({"decision": "deny"}).to_string()))
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        serde_json::from_slice::<Value>(
            &axum::body::to_bytes(resp.into_body(), 1 << 20)
                .await
                .unwrap(),
        )
        .unwrap()
    });
    let req = r.req().await;
    assert!(matches!(req.op, Op::Decide { allow: false, .. }));
    r.ok(req.id, json!({})).await;
    assert_eq!(deny.await.unwrap()["outcome"], "deny");
}

#[tokio::test]
async fn questions_and_plans_are_labelled_and_cant_be_approved_from_the_app() {
    let mut r = rig().await;
    let phone = Phone::new();
    let (_device, token) = r.pair(&phone).await;
    let t = Some(token.as_str());
    let sid = r.start(&token, "rq-3").await;
    r.approval(
        &sid,
        "q1",
        "AskUserQuestion",
        json!({"questions": [{"question": "Red or blue?", "options": []}]}),
    )
    .await;
    r.approval(&sid, "p1", "ExitPlanMode", json!({"plan": "1. do it"}))
        .await;
    let s = r.call("GET", &format!("/sessions/{sid}"), t, None).await;
    let pending = s.body["pending_approvals"].as_array().unwrap();
    assert_eq!(pending[0]["kind"], "question");
    assert_eq!(pending[0]["questions"], json!(["Red or blue?"]));
    assert_eq!(
        (pending[1]["kind"].as_str(), pending[1]["plan"].as_str()),
        (Some("plan"), Some("1. do it"))
    );
    for id in ["q1", "p1"] {
        let reply = r
            .call("POST", &format!("/approvals/{id}/decision"), t,
                  Some(json!({"decision": "allow", "nonce": "n", "signature": "s", "message_version": 1})))
            .await;
        assert_eq!(reply.body["error"]["code"], "answer_in_herdr", "{id}");
    }
}

#[tokio::test]
async fn a_revoked_device_is_refused_on_its_next_request() {
    let r = rig().await;
    let (p1, p2) = (Phone::new(), Phone::new());
    let (d1, t1) = r.pair(&p1).await;
    let (_d2, t2) = r.pair(&p2).await;
    assert_eq!(
        r.call("GET", "/sessions", Some(&t1), None).await.status,
        StatusCode::OK
    );
    let revoked = r
        .call("DELETE", &format!("/devices/{d1}"), Some(&t2), None)
        .await;
    assert_eq!(revoked.status, StatusCode::NO_CONTENT);
    assert_eq!(
        r.call("GET", "/sessions", Some(&t1), None).await.status,
        StatusCode::UNAUTHORIZED
    );
    let again = r
        .call("DELETE", &format!("/devices/{d1}"), Some(&t2), None)
        .await;
    assert_eq!(again.status, StatusCode::NOT_FOUND);
    let audit = r.svc.audit_log(None, 50).await.unwrap();
    assert!(
        audit
            .iter()
            .any(|e| e.kind == "device_revoked" && e.actor.starts_with("app:"))
    );
}

#[tokio::test]
async fn a_lost_start_response_can_be_looked_up_by_its_request_id() {
    let mut r = rig().await;
    let phone = Phone::new();
    let (_d, token) = r.pair(&phone).await;
    let app = r.app.clone();
    let tok = token.clone();
    let call = tokio::spawn(async move {
        let req = HttpRequest::builder()
            .method("POST")
            .uri(format!("{PREFIX}/sessions"))
            .header("content-type", "application/json")
            .header("authorization", format!("Bearer {tok}"))
            .body(Body::from(
                json!({"project_id": "switchboard", "prompt": "x", "request_id": "lost-1"})
                    .to_string(),
            ))
            .unwrap();
        app.oneshot(req).await.unwrap().status()
    });
    let _start = r.req().await;
    // The phone gives up waiting; the start is still in flight.
    let pending = loop {
        let op = r
            .call("GET", "/operations/lost-1", Some(&token), None)
            .await;
        if op.status == StatusCode::OK {
            break op;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    };
    assert_eq!(pending.body["state"], "pending");
    assert!(pending.body["session_id"].is_string());
    r.svc.runner_disconnected("mac", r.generation).await;
    assert_eq!(
        call.await.unwrap(),
        StatusCode::GATEWAY_TIMEOUT,
        "outcome unknown"
    );
    let op = r
        .call("GET", "/operations/lost-1", Some(&token), None)
        .await;
    assert_eq!(op.body["state"], "outcome_unknown");
}

#[tokio::test]
async fn old_client_requests_are_still_accepted() {
    let mut r = rig().await;
    let phone = Phone::new();
    let code = r.devices.issue_code().await.unwrap();
    let fixture = |name: &str| -> Value {
        let text =
            std::fs::read_to_string(format!("tests/fixtures/app-v1/requests/{name}.json")).unwrap();
        serde_json::from_str(&text).unwrap()
    };
    let mut pair = fixture("pair");
    pair["code"] = json!(code.code);
    pair["auth_key"] = json!(phone.auth.public());
    pair["approval_key"] = json!(phone.approval.public());
    let reply = r.call("POST", "/pair", None, Some(pair)).await;
    assert_eq!(reply.status, StatusCode::OK, "{}", reply.body);
    let device = reply.body["device_id"].as_str().unwrap().to_owned();
    r.devices.confirm(&device, true).await.unwrap();
    let token = r.token(&phone, &device).await;

    let app = r.app.clone();
    let start = fixture("start");
    let tok = token.clone();
    let call = tokio::spawn(async move {
        let req = HttpRequest::builder()
            .method("POST")
            .uri(format!("{PREFIX}/sessions"))
            .header("content-type", "application/json")
            .header("authorization", format!("Bearer {tok}"))
            .body(Body::from(start.to_string()))
            .unwrap();
        app.oneshot(req).await.unwrap().status()
    });
    let req = r.req().await;
    let Op::StartSession {
        session_id, branch, ..
    } = req.op
    else {
        panic!()
    };
    r.ok(req.id, json!({"session_id": session_id, "branch": branch}))
        .await;
    assert_eq!(call.await.unwrap(), StatusCode::OK);
    r.approval(&session_id, "a1", "Bash", json!({"command": "ls"}))
        .await;
    let app = r.app.clone();
    let deny = fixture("deny");
    let call = tokio::spawn(async move {
        let req = HttpRequest::builder()
            .method("POST")
            .uri(format!("{PREFIX}/approvals/a1/decision"))
            .header("content-type", "application/json")
            .header("authorization", format!("Bearer {token}"))
            .body(Body::from(deny.to_string()))
            .unwrap();
        app.oneshot(req).await.unwrap().status()
    });
    let req = r.req().await;
    r.ok(req.id, json!({})).await;
    assert_eq!(call.await.unwrap(), StatusCode::OK);
}

#[test]
fn the_compatibility_check_catches_breaking_changes() {
    let old: Value = serde_json::from_str(&openapi::spec_json()).unwrap();
    let mut new = old.clone();
    new["components"]["schemas"]["Session"]["required"]
        .as_array_mut()
        .unwrap()
        .retain(|f| f != "lifecycle");
    new["components"]["schemas"]["PromptRequest"]["required"]
        .as_array_mut()
        .unwrap()
        .push(json!("urgency"));
    new["paths"].as_object_mut().unwrap().remove("/app/v1/meta");
    let problems = compat_problems(&old, &new);
    assert_eq!(problems.len(), 3, "{problems:#?}");
    assert!(compat_problems(&old, &old).is_empty());
}

// One message box: the hub decides between a new session and an existing one.

fn agents_reply() -> Value {
    json!([
        {"name": "mono1/claude", "pane_id": "w1:p1", "kind": "claude", "agent_status": "blocked",
         "session_id": null, "herdr_session": "mono1"},
        {"name": "mono1/w1:p2", "pane_id": "w1:p2", "kind": "claude", "agent_status": "idle",
         "session_id": null, "herdr_session": "mono1",
         "title": "Rotate ghp_abcdefghijklmnopqrstuvwxyz0123456789"},
        {"name": "sb-task", "pane_id": "w2:p1", "kind": "claude", "agent_status": "working",
         "session_id": "s-task", "herdr_session": "switchboard-1a2b"}
    ])
}

#[tokio::test]
async fn a_message_naming_a_project_starts_there_and_plain_text_then_goes_to_it() {
    let mut r = rig().await;
    let (_, token) = r.pair(&Phone::new()).await;
    let call = r.call_later(
        "POST",
        "/messages",
        &token,
        json!({"text": "in switchboard, fix the flaky test", "request_id": "m1"}),
    );
    let req = r.req().await;
    let Op::StartSession {
        session_id,
        branch,
        prompt,
        ..
    } = req.op
    else {
        panic!("expected a start");
    };
    assert_eq!(prompt, "in switchboard, fix the flaky test");
    r.ok(
        req.id,
        json!({"session_id": session_id, "branch": branch, "worktree_path": "/wt",
               "workspace_id": "w1", "pane_id": "w1:p1"}),
    )
    .await;
    let started = call.await.unwrap();
    assert_eq!(started.status, StatusCode::OK, "{}", started.body);
    assert_eq!(started.body["outcome"], "started");
    assert_eq!(started.body["start"]["session_id"], session_id.as_str());
    assert_eq!(started.body["current"]["kind"], "session");
    assert_eq!(started.body["current"]["session_id"], session_id.as_str());

    let call = r.call_later(
        "POST",
        "/messages",
        &token,
        json!({"text": "also add a test", "request_id": "m2",
               "current": {"session_id": session_id}}),
    );
    let req = r.req().await;
    match req.op {
        Op::Prompt {
            session_id: to,
            text,
        } => assert_eq!(
            (to.as_str(), text.as_str()),
            (session_id.as_str(), "also add a test")
        ),
        other => panic!("expected a prompt, got {other:?}"),
    }
    r.ok(req.id, json!({"agent_status": "working"})).await;
    let sent = call.await.unwrap();
    assert_eq!(
        (
            sent.body["outcome"].as_str(),
            sent.body["current"]["session_id"].as_str()
        ),
        (Some("sent"), Some(session_id.as_str()))
    );
}

#[tokio::test]
async fn with_nothing_current_the_hub_asks_which_project_and_the_answer_starts_it() {
    let mut r = rig_with(
        "[[projects]]\nid = \"kraft\"\ndescription = \"k\"\nrepos = { mac = \"/Users/me/kraft\" }",
    )
    .await;
    let (_, token) = r.pair(&Phone::new()).await;
    let ask = r
        .call(
            "POST",
            "/messages",
            Some(&token),
            Some(json!({"text": "fix the flaky test", "request_id": "m1"})),
        )
        .await;
    assert_eq!(ask.body["outcome"], "choose_project", "{}", ask.body);
    let options: Vec<&str> = ask.body["options"]
        .as_array()
        .unwrap()
        .iter()
        .map(|o| o["project_id"].as_str().unwrap())
        .collect();
    assert_eq!(options, ["switchboard", "kraft"]);
    assert!(ask.body.get("current").is_none());

    let call = r.call_later(
        "POST",
        "/messages",
        &token,
        json!({"text": "fix the flaky test", "request_id": "m1", "project_id": "kraft"}),
    );
    let req = r.req().await;
    let Op::StartSession {
        session_id,
        project_id,
        branch,
        ..
    } = req.op
    else {
        panic!("expected a start");
    };
    assert_eq!(project_id, "kraft");
    r.ok(
        req.id,
        json!({"session_id": session_id, "branch": branch, "worktree_path": "/wt",
               "workspace_id": "w1", "pane_id": "w1:p1"}),
    )
    .await;
    assert_eq!(call.await.unwrap().body["outcome"], "started");

    let unknown = r
        .call(
            "POST",
            "/messages",
            Some(&token),
            Some(json!({"text": "x", "request_id": "m3", "project_id": "nope"})),
        )
        .await;
    assert_eq!(unknown.status, StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn use_reaches_an_agent_switchboard_didnt_start_and_it_becomes_current() {
    let mut r = rig().await;
    let (_, token) = r.pair(&Phone::new()).await;
    let call = r.call_later(
        "POST",
        "/messages",
        &token,
        json!({"text": "use mono1/claude: rerun the tests", "request_id": "m1"}),
    );
    let req = r.req().await;
    assert!(matches!(req.op, Op::Agents), "{:?}", req.op);
    r.ok(req.id, agents_reply()).await;
    let req = r.req().await;
    match req.op {
        Op::PromptAgent { agent, text } => {
            assert_eq!(
                (agent.as_str(), text.as_str()),
                ("mono1/claude", "rerun the tests")
            )
        }
        other => panic!("expected an agent prompt, got {other:?}"),
    }
    r.ok(req.id, json!({"outcome": "sent"})).await;
    let sent = call.await.unwrap();
    assert_eq!(sent.body["outcome"], "sent", "{}", sent.body);
    assert_eq!(
        (
            sent.body["current"]["kind"].as_str(),
            sent.body["current"]["agent"].as_str()
        ),
        (Some("agent"), Some("mono1/claude"))
    );

    // At a dialog, nothing is typed and the reason comes back.
    let call = r.call_later(
        "POST",
        "/messages",
        &token,
        json!({"text": "and the lint", "request_id": "m2",
               "current": {"machine_id": "mac", "agent": "mono1/claude"}}),
    );
    let req = r.req().await;
    assert!(matches!(req.op, Op::PromptAgent { .. }));
    r.ok(
        req.id,
        json!({"outcome": "blocked", "screen": "Allow? (y/n)"}),
    )
    .await;
    let refused = call.await.unwrap();
    assert_eq!(refused.body["outcome"], "refused");
    assert!(
        refused.body["message"]
            .as_str()
            .unwrap()
            .contains("Answer it in Herdr")
    );
}

#[tokio::test]
async fn an_agent_that_leaves_its_pane_is_listed_as_ended() {
    let mut r = rig().await;
    let (_, token) = r.pair(&Phone::new()).await;
    let call = r.call_later("GET", "/agents", &token, Value::Null);
    let req = r.req().await;
    r.ok(req.id, agents_reply()).await;
    let first = call.await.unwrap();
    assert_eq!(first.body["ended"], json!([]));

    let call = r.call_later("GET", "/agents", &token, Value::Null);
    let req = r.req().await;
    let mut still = agents_reply();
    still.as_array_mut().unwrap().remove(1);
    r.ok(req.id, still).await;
    let second = call.await.unwrap();
    assert_eq!(second.body["agents"].as_array().unwrap().len(), 1);
    let ended = second.body["ended"].as_array().unwrap();
    assert_eq!(ended.len(), 1, "{}", second.body);
    assert_eq!(
        (
            ended[0]["name"].as_str(),
            ended[0]["herdr_session"].as_str()
        ),
        (Some("mono1/w1:p2"), Some("mono1"))
    );
    let title = ended[0]["title"].as_str().unwrap();
    assert!(!title.contains("ghp_abc"), "kept masked: {title}");
}

#[tokio::test]
async fn the_app_lists_and_reads_agents_switchboard_didnt_start_masked() {
    let mut r = rig().await;
    let (_, token) = r.pair(&Phone::new()).await;
    let call = r.call_later("GET", "/agents", &token, Value::Null);
    let req = r.req().await;
    assert!(matches!(req.op, Op::Agents));
    r.ok(req.id, agents_reply()).await;
    let list = call.await.unwrap();
    assert_eq!(list.status, StatusCode::OK, "{}", list.body);
    let agents = list.body["agents"].as_array().unwrap();
    assert_eq!(
        agents.len(),
        2,
        "a task's own agent is a session, not listed here"
    );
    assert_eq!(
        (
            agents[0]["name"].as_str(),
            agents[0]["herdr_session"].as_str(),
            agents[0]["agent_status"].as_str()
        ),
        (Some("mono1/claude"), Some("mono1"), Some("blocked"))
    );
    let unnamed = &agents[1];
    assert_eq!(
        unnamed["name"], "mono1/w1:p2",
        "an unnamed agent goes by its pane"
    );
    let title = unnamed["title"].as_str().unwrap();
    assert!(
        title.starts_with("Rotate") && !title.contains("ghp_abc"),
        "{title}"
    );

    const SECRET: &str = "ghp_abcdefghijklmnopqrstuvwxyz0123456789";
    let call = r.call_later(
        "GET",
        "/agents/output?machine_id=mac&agent=mono1/claude&lines=20",
        &token,
        Value::Null,
    );
    let req = r.req().await;
    match req.op {
        Op::ReadAgent {
            agent,
            lines,
            visible,
            ..
        } => {
            assert_eq!(
                (agent.as_str(), lines, visible),
                ("mono1/claude", 20, false)
            )
        }
        other => panic!("expected a read, got {other:?}"),
    }
    r.ok(
        req.id,
        json!({"text": format!("export GITHUB_TOKEN={SECRET}\nok\n")}),
    )
    .await;
    let out = call.await.unwrap();
    let text = out.body["text"].as_str().unwrap();
    assert!(!text.contains(SECRET) && text.contains("ok"), "{text}");
}

#[tokio::test]
async fn preview_says_where_a_message_would_go_without_doing_it() {
    let mut r = rig_with(
        "[[projects]]\nid = \"kraft\"\ndescription = \"k\"\nrepos = { mac = \"/Users/me/kraft\" }",
    )
    .await;
    let (_, token) = r.pair(&Phone::new()).await;
    let preview = |body: Value| r.call("POST", "/messages/preview", Some(&token), Some(body));

    let start = preview(json!({"text": "in kraft, run the benchmarks"})).await;
    assert_eq!(start.status, StatusCode::OK, "{}", start.body);
    assert_eq!(
        (
            start.body["outcome"].as_str(),
            start.body["project_id"].as_str(),
            start.body["machine_id"].as_str()
        ),
        (Some("start"), Some("kraft"), Some("mac"))
    );

    let ask = preview(json!({"text": "fix the flaky test"})).await;
    assert_eq!(ask.body["outcome"], "choose_project", "{}", ask.body);
    assert_eq!(ask.body["options"].as_array().unwrap().len(), 2);

    let sid = r.start(&token, "s1").await;
    let send = r
        .call(
            "POST",
            "/messages/preview",
            Some(&token),
            Some(json!({"text": "also add a test", "current": {"session_id": sid}})),
        )
        .await;
    assert_eq!(
        (
            send.body["outcome"].as_str(),
            send.body["target"]["session_id"].as_str()
        ),
        (Some("send"), Some(sid.as_str()))
    );
    // Nothing reached the runner: a preview never starts or sends.
    assert!(r.runner.try_recv().is_err());
}

#[tokio::test]
async fn changes_come_from_the_runner_with_paths_masked() {
    let mut r = rig().await;
    let (_, token) = r.pair(&Phone::new()).await;
    let sid = r.start(&token, "c1").await;
    let call = r.call_later(
        "GET",
        &format!("/sessions/{sid}/changes"),
        &token,
        Value::Null,
    );
    let req = r.req().await;
    assert!(matches!(req.op, Op::Changes { ref session_id } if *session_id == sid));
    r.ok(
        req.id,
        json!({"files": [
            {"path": "src/a.rs", "added": 3, "deleted": 1},
            {"path": "export API_TOKEN=abc123secret", "added": null, "deleted": null}
        ], "truncated": false}),
    )
    .await;
    let got = call.await.unwrap();
    assert_eq!(got.status, StatusCode::OK, "{}", got.body);
    assert_eq!(
        got.body["files"][0],
        json!({"path": "src/a.rs", "added": 3, "deleted": 1})
    );
    assert!(got.body["files"][1].get("added").is_none());
    assert!(
        !got.body.to_string().contains("abc123secret"),
        "{}",
        got.body
    );
}

#[tokio::test]
async fn a_conversation_comes_from_the_runner_a_page_at_a_time_masked() {
    const SECRET: &str = "ghp_abcdefghijklmnopqrstuvwxyz0123456789";
    let mut r = rig().await;
    let (_, token) = r.pair(&Phone::new()).await;
    let sid = r.start(&token, "t1").await;
    let call = r.call_later(
        "GET",
        &format!("/sessions/{sid}/transcript?before=4096"),
        &token,
        Value::Null,
    );
    let req = r.req().await;
    assert_eq!(
        req.op,
        Op::Transcript {
            agent: crate::runner::sessions::agent_name(&sid),
            after: None,
            before: Some(4096)
        },
        "the session's own agent"
    );
    r.ok(
        req.id,
        json!({"items": [
            {"id": "u1:0", "kind": "user", "text": "fix the flaky test", "done": false, "failed": false},
            {"id": "t1", "kind": "tool", "text": format!("Ran export GITHUB_TOKEN={SECRET}"), "tool": "Bash",
             "input": "{}", "output": "ok", "done": true, "failed": false, "at": "2026-10-26T09:00:00Z"}
        ], "start": 1024, "end": 4096}),
    )
    .await;
    let got = call.await.unwrap();
    assert_eq!(got.status, StatusCode::OK, "{}", got.body);
    assert_eq!(
        (got.body["start"].as_u64(), got.body["end"].as_u64()),
        (Some(1024), Some(4096))
    );
    assert_eq!(got.body["items"][0]["text"], "fix the flaky test");
    assert!(got.body["items"][0].get("tool").is_none());
    assert_eq!(got.body["items"][1]["done"], true);
    assert!(!got.body.to_string().contains(SECRET), "{}", got.body);

    let call = r.call_later(
        "GET",
        "/agents/transcript?machine_id=mac&agent=mono1/w1:p2&after=10",
        &token,
        Value::Null,
    );
    let req = r.req().await;
    assert_eq!(
        req.op,
        Op::Transcript {
            agent: "mono1/w1:p2".into(),
            after: Some(10),
            before: None
        }
    );
    r.err(
        req.id,
        ErrorCode::BadRequest,
        "only Claude agents have a conversation to read",
    )
    .await;
    let got = call.await.unwrap();
    assert_eq!(got.status, StatusCode::BAD_REQUEST, "{}", got.body);
}

#[tokio::test]
async fn a_visible_read_takes_only_the_screen_so_nothing_scrolls() {
    let mut r = rig().await;
    let (_, token) = r.pair(&Phone::new()).await;
    let sid = r.start(&token, "v1").await;
    let call = r.call_later(
        "GET",
        &format!("/sessions/{sid}/output?visible=true"),
        &token,
        Value::Null,
    );
    let req = r.req().await;
    assert_eq!(
        req.op,
        Op::ReadAgent {
            agent: crate::runner::sessions::agent_name(&sid),
            lines: 0,
            visible: true,
            ansi: false,
        }
    );
    r.ok(req.id, json!({"text": "the screen"})).await;
    assert_eq!(call.await.unwrap().body["text"], "the screen");

    let call = r.call_later(
        "GET",
        "/agents/output?machine_id=mac&agent=mono1/claude&visible=true",
        &token,
        Value::Null,
    );
    let req = r.req().await;
    assert!(
        matches!(req.op, Op::ReadAgent { visible: true, .. }),
        "{:?}",
        req.op
    );
    r.ok(req.id, json!({"text": "its screen"})).await;
    assert_eq!(call.await.unwrap().body["text"], "its screen");
}
