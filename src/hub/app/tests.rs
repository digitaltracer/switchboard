use axum::body::Body;
use axum::http::Request as HttpRequest;
use serde_json::{Value, json};
use tokio::sync::mpsc;
use tower::ServiceExt;

use super::*;
use crate::auth::device::tests::Phone;
use crate::auth::device::{
    AnswerFields, answer_message, approval_message, label_sha256, token_message,
};
use crate::config::HubConfig;
use crate::hub::service::Clock;
use crate::proto::auth::hash_token;
use crate::proto::{
    ErrorCode, Event, Frame, Hello, Op, Outcome, PROTOCOL_VERSION, ProtoError, Resp,
};
use crate::store::{PushEnv, PushProvider, Store};

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
    rig_advertising(extra, vec![]).await
}

/// A rig whose runner advertises `capabilities`; others are an older runner's.
async fn rig_advertising(extra: &str, capabilities: Vec<String>) -> Rig {
    let store = Store::open_in_memory().await.unwrap();
    let svc = SessionService::new(config(extra), store.clone(), Clock::System);
    let devices = Arc::new(Devices::new(store, Clock::System));
    let hello = Hello {
        machine_id: "mac".into(),
        protocol_version: PROTOCOL_VERSION,
        runner_version: "test".into(),
        herdr_version: None,
        sessions: vec![],
        capabilities,
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
async fn questions_and_plans_are_answered_with_a_signed_choice() {
    let mut r = rig().await;
    let phone = Phone::new();
    let (device, token) = r.pair(&phone).await;
    let t = Some(token.as_str());
    let sid = r.start(&token, "rq-3").await;
    r.approval(
        &sid,
        "q1",
        "AskUserQuestion",
        json!({"questions": [{"question": "Red or blue?", "header": "Colour",
            "options": [{"label": "Red"}, {"label": "Blue", "description": "the sky"}]}]}),
    )
    .await;
    r.approval(&sid, "p1", "ExitPlanMode", json!({"plan": "1. do it"}))
        .await;
    let s = r.call("GET", &format!("/sessions/{sid}"), t, None).await;
    let pending = s.body["pending_approvals"].as_array().unwrap();
    assert_eq!(pending[0]["kind"], "question");
    assert_eq!(pending[0]["questions"], json!(["Red or blue?"]));
    let q = &pending[0]["question_options"][0];
    assert_eq!(
        (q["header"].as_str(), q["multi_select"].as_bool()),
        (Some("Colour"), Some(false))
    );
    assert_eq!(
        q["options"][1],
        json!({"id": "0.1", "label": "Blue", "description": "the sky"})
    );
    assert_eq!(
        (pending[1]["kind"].as_str(), pending[1]["plan"].as_str()),
        (Some("plan"), Some("1. do it"))
    );

    let ch = r
        .call("POST", "/approvals/q1/challenge", t, None)
        .await
        .body;
    assert_eq!(ch["message_version"], 3);
    let label = label_sha256(&["Blue".into()]);
    let msg = answer_message(
        ch["hub_id"].as_str().unwrap(),
        &device,
        &AnswerFields {
            target: "approval:q1",
            occurrence: "q1",
            option: "0.1",
            label: &label,
            request_id: "rq-a1",
        },
        ch["nonce"].as_str().unwrap(),
        ch["expires_at"].as_i64().unwrap(),
    );
    // A signature over another choice doesn't verify.
    let wrong = r
        .call("POST", "/approvals/q1/decision", t,
              Some(json!({"decision": "allow", "nonce": ch["nonce"], "signature": phone.approval.sign(&msg),
                          "message_version": 3, "request_id": "rq-a1", "choices": ["0.0"]})))
        .await;
    assert_eq!(wrong.body["error"]["code"], "bad_signature");
    let v1 = r
        .call("POST", "/approvals/q1/decision", t,
              Some(json!({"decision": "allow", "nonce": ch["nonce"], "signature": phone.approval.sign(&msg),
                          "message_version": 1, "request_id": "rq-a1", "choices": ["0.1"]})))
        .await;
    assert_eq!(v1.body["error"]["code"], "unsupported_version");
    let decide = r.call_later(
        "POST",
        "/approvals/q1/decision",
        &token,
        json!({"decision": "allow", "nonce": ch["nonce"], "signature": phone.approval.sign(&msg),
               "message_version": 3, "request_id": "rq-a1", "choices": ["0.1"]}),
    );
    let req = r.req().await;
    assert!(matches!(req.op, Op::Decide { allow: true, ref choices, .. } if choices == &["0.1"]));
    r.ok(req.id, json!({})).await;
    assert_eq!(decide.await.unwrap().body["outcome"], "allow");

    let ch = r
        .call("POST", "/approvals/p1/challenge", t, None)
        .await
        .body;
    let plan_label = label_sha256(&["Approve".into()]);
    let msg = answer_message(
        ch["hub_id"].as_str().unwrap(),
        &device,
        &AnswerFields {
            target: "approval:p1",
            occurrence: "p1",
            option: "allow",
            label: &plan_label,
            request_id: "rq-p1",
        },
        ch["nonce"].as_str().unwrap(),
        ch["expires_at"].as_i64().unwrap(),
    );
    let decide = r.call_later(
        "POST",
        "/approvals/p1/decision",
        &token,
        json!({"decision": "allow", "nonce": ch["nonce"], "signature": phone.approval.sign(&msg),
               "message_version": 3, "request_id": "rq-p1"}),
    );
    let req = r.req().await;
    assert!(matches!(req.op, Op::Decide { allow: true, .. }));
    r.ok(req.id, json!({})).await;
    assert_eq!(decide.await.unwrap().body["outcome"], "allow");
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
async fn an_agents_skills_come_from_its_runner_masked() {
    let mut r = rig().await;
    let (_, token) = r.pair(&Phone::new()).await;
    let call = r.call_later(
        "GET",
        "/agents/skills?machine_id=mac&agent=mono1/claude",
        &token,
        Value::Null,
    );
    let req = r.req().await;
    assert_eq!(
        req.op,
        Op::Skills {
            agent: "mono1/claude".into()
        }
    );
    r.ok(
        req.id,
        json!({"skills": [
            {"name": "/commit", "description": "Atomic commits", "source": "user"},
            {"name": "/deploy", "description": "Uses ghp_abcdefghijklmnopqrstuvwxyz0123456789", "source": "repo"}
        ]}),
    )
    .await;
    let reply = call.await.unwrap();
    assert_eq!(reply.status, StatusCode::OK, "{}", reply.body);
    let skills = reply.body["skills"].as_array().unwrap();
    assert_eq!(skills[0]["name"], "/commit");
    assert!(
        !skills[1]["description"]
            .as_str()
            .unwrap()
            .contains("ghp_abc")
    );
}

#[tokio::test]
async fn activity_comes_from_a_runner_that_advertises_it_masked() {
    let mut r = rig_advertising("", vec![crate::proto::CAP_ACTIVITY.into()]).await;
    let (_, token) = r.pair(&Phone::new()).await;
    let call = r.call_later("GET", "/agents?activity=true", &token, Value::Null);
    for _ in 0..2 {
        let req = r.req().await;
        let reply = match req.op {
            Op::Agents => agents_reply(),
            Op::Activity => json!([
                {"name": "mono1/claude", "session_id": null,
                 "line": "Pushed with ghp_abcdefghijklmnopqrstuvwxyz0123456789", "at": "2026-11-07T09:00:00Z"},
                {"name": "mono1/w1:p2", "session_id": null, "line": null, "at": null},
                {"name": "sb-task", "session_id": "s-task", "line": "Ran cargo test", "at": null},
            ]),
            other => panic!("unexpected {other:?}"),
        };
        r.ok(req.id, reply).await;
    }
    let reply = call.await.unwrap();
    assert_eq!(reply.status, StatusCode::OK, "{}", reply.body);
    assert_eq!(reply.body["activity_machines"], json!(["mac"]));
    let activity = reply.body["activity"].as_array().unwrap();
    assert_eq!(activity.len(), 3, "one entry per agent, sessions' included");
    let line = activity[0]["line"].as_str().unwrap();
    assert!(
        line.starts_with("Pushed with") && !line.contains("ghp_abc"),
        "{line}"
    );
    assert_eq!(activity[0]["machine_id"], "mac");
    assert!(activity[1].get("line").is_none(), "no line yet");
    assert_eq!(activity[2]["session_id"], "s-task");

    // Without asking, the list is as before.
    let call = r.call_later("GET", "/agents", &token, Value::Null);
    let req = r.req().await;
    assert!(matches!(req.op, Op::Agents));
    r.ok(req.id, agents_reply()).await;
    let plain = call.await.unwrap();
    assert!(plain.body.get("activity").is_none() && plain.body.get("activity_machines").is_none());
}

#[tokio::test]
async fn an_older_runner_is_never_asked_for_activity() {
    let mut r = rig().await;
    let (_, token) = r.pair(&Phone::new()).await;
    let start = Instant::now();
    let call = r.call_later("GET", "/agents?activity=true", &token, Value::Null);
    let req = r.req().await;
    assert!(matches!(req.op, Op::Agents), "{:?}", req.op);
    r.ok(req.id, agents_reply()).await;
    let reply = call.await.unwrap();
    assert!(
        start.elapsed() < Duration::from_secs(1),
        "{:?}",
        start.elapsed()
    );
    assert_eq!(reply.status, StatusCode::OK, "{}", reply.body);
    assert_eq!(reply.body["activity"], json!([]));
    assert_eq!(reply.body["activity_machines"], json!([]));
    assert!(r.runner.try_recv().is_err(), "nothing else was sent");
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
            before: Some(4096),
            wait_ms: None,
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
        "/agents/transcript?machine_id=mac&agent=mono1/w1:p2&after=10&wait_ms=20000",
        &token,
        Value::Null,
    );
    let req = r.req().await;
    assert_eq!(
        req.op,
        Op::Transcript {
            agent: "mono1/w1:p2".into(),
            after: Some(10),
            before: None,
            wait_ms: Some(20_000),
        },
        "the wait reaches the runner"
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

fn dialog_reply(occurrence: &str) -> Value {
    json!({
        "occurrence": occurrence,
        "dialog": {
            "kind": "permission",
            "prompt": "Would you like to run the following command?\n$ curl -H 'x: ghp_0123456789abcdefghijABCDEFGHIJ'",
            "options": [
                {"id": "o1", "label": "Yes, proceed (y)", "label_sha256": label_sha256(&["Yes, proceed (y)".into()]),
                 "is_deny": false, "offered": true},
                {"id": "o3", "label": "No", "label_sha256": label_sha256(&["No".into()]),
                 "is_deny": true, "offered": true},
            ],
            "selected": "o1",
            "occurrence": occurrence,
            "answerable": true,
        }
    })
}

#[tokio::test]
async fn an_agents_dialog_is_read_masked_and_answered_by_its_options() {
    let mut r = rig().await;
    let phone = Phone::new();
    let (device, token) = r.pair(&phone).await;
    let read = r.call_later(
        "GET",
        "/agents/dialog?machine_id=mac&agent=work%2Fcx",
        &token,
        Value::Null,
    );
    let req = r.req().await;
    assert_eq!(
        req.op,
        Op::Dialog {
            agent: "work/cx".into()
        }
    );
    r.ok(req.id, dialog_reply("occ1")).await;
    let d = read.await.unwrap().body;
    assert_eq!(d["dialog"]["occurrence_id"], "occ1");
    assert!(
        !d["dialog"]["prompt"].as_str().unwrap().contains("ghp_"),
        "{d}"
    );
    assert_eq!(d["dialog"]["options"][1]["is_deny"], true);

    // The deny needs only the token.
    let deny = r.call_later(
        "POST",
        "/agents/dialog/answer",
        &token,
        json!({"machine_id": "mac", "agent": "work/cx", "occurrence_id": "occ1", "option_id": "o3",
               "label_sha256": label_sha256(&["No".into()])}),
    );
    let req = r.req().await;
    assert!(matches!(req.op, Op::Answer { deny_only: true, ref option, .. } if option == "o3"));
    r.ok(req.id, json!({"answered": true})).await;
    assert_eq!(deny.await.unwrap().body["outcome"], "answered");

    // Any other option needs the approval key's signature over the answer message.
    let ch = r
        .call(
            "POST",
            "/agents/dialog/challenge",
            Some(&token),
            Some(json!({"machine_id": "mac", "agent": "work/cx", "occurrence_id": "occ2"})),
        )
        .await
        .body;
    assert_eq!(
        (ch["message_version"].as_u64(), ch["target"].as_str()),
        (Some(3), Some("agent:mac/work/cx"))
    );
    let label = label_sha256(&["Yes, proceed (y)".into()]);
    let msg = answer_message(
        ch["hub_id"].as_str().unwrap(),
        &device,
        &AnswerFields {
            target: "agent:mac/work/cx",
            occurrence: "occ2",
            option: "o1",
            label: &label,
            request_id: "rq-d1",
        },
        ch["nonce"].as_str().unwrap(),
        ch["expires_at"].as_i64().unwrap(),
    );
    let body = |sig: String, option: &str| {
        json!({"machine_id": "mac", "agent": "work/cx", "occurrence_id": "occ2", "option_id": option,
               "label_sha256": label, "request_id": "rq-d1", "nonce": ch["nonce"], "signature": sig,
               "message_version": 3})
    };
    let forged = r
        .call(
            "POST",
            "/agents/dialog/answer",
            Some(&token),
            Some(body(phone.approval.sign(&msg), "o3")),
        )
        .await;
    assert_eq!(forged.body["error"]["code"], "bad_signature");
    let stolen = r
        .call(
            "POST",
            "/agents/dialog/answer",
            Some(&token),
            Some(body(phone.auth.sign(&msg), "o1")),
        )
        .await;
    assert_eq!(stolen.body["error"]["code"], "bad_signature");
    let yes = r.call_later(
        "POST",
        "/agents/dialog/answer",
        &token,
        body(phone.approval.sign(&msg), "o1"),
    );
    let req = r.req().await;
    assert!(
        matches!(req.op, Op::Answer { deny_only: false, ref occurrence, .. } if occurrence == "occ2")
    );
    r.err(req.id, ErrorCode::DialogChanged, "the dialog changed")
        .await;
    let changed = yes.await.unwrap();
    assert_eq!(
        (changed.status, changed.body["error"]["code"].as_str()),
        (StatusCode::CONFLICT, Some("dialog_changed"))
    );
    // A used nonce can't be signed again; the recorded outcome is under the request id.
    let again = r
        .call(
            "POST",
            "/agents/dialog/answer",
            Some(&token),
            Some(body(phone.approval.sign(&msg), "o1")),
        )
        .await;
    assert_eq!(again.body["error"]["code"], "invalid_challenge");
    let op = r.call("GET", "/operations/rq-d1", Some(&token), None).await;
    assert_eq!(op.body["state"], "failed", "{}", op.body);
}

#[tokio::test]
async fn an_attention_opens_only_while_its_agent_shows_that_occurrence() {
    let mut r = rig().await;
    let (_device, token) = r.pair(&Phone::new()).await;
    let now = r.svc.now();
    let att = |id: &str, occurrence: &str, expires_at: i64| crate::store::AttentionRecord {
        id: id.into(),
        machine_id: "mac".into(),
        herdr_session: Some("work".into()),
        agent: "work/cx".into(),
        terminal_id: "term_1".into(),
        session_id: None,
        occurrence: occurrence.into(),
        created_at: now,
        expires_at,
        resolved_at: None,
        resolution: None,
    };
    let store = r.svc.store().clone();
    store
        .insert_attention(att("at1", "occ1", now + 60_000))
        .await
        .unwrap();
    store
        .insert_attention(att("at2", "occ0", now + 60_000))
        .await
        .unwrap();
    store
        .insert_attention(att("at3", "occ9", now - 1))
        .await
        .unwrap();

    let open = r.call_later("GET", "/attention/at1", &token, Value::Null);
    let req = r.req().await;
    r.ok(req.id, dialog_reply("occ1")).await;
    let open = open.await.unwrap().body;
    assert_eq!(
        (open["state"].as_str(), open["agent"].as_str()),
        (Some("open"), Some("work/cx"))
    );
    assert_eq!(open["dialog"]["options"][0]["id"], "o1");

    // The agent moved on, or a new one took the pane: the attention is gone, for good.
    let gone = r.call_later("GET", "/attention/at2", &token, Value::Null);
    let req = r.req().await;
    r.ok(req.id, dialog_reply("occ1")).await;
    assert_eq!(gone.await.unwrap().body["state"], "gone");
    let again = r.call("GET", "/attention/at2", Some(&token), None).await;
    assert_eq!(again.body["state"], "gone");
    assert!(again.body.get("dialog").is_none());

    let expired = r.call("GET", "/attention/at3", Some(&token), None).await;
    assert_eq!(expired.body["state"], "expired");
    let missing = r.call("GET", "/attention/nope", Some(&token), None).await;
    assert_eq!(missing.status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn an_android_device_registers_its_fcm_token() {
    let r = rig().await;
    let (device, token) = r.pair(&Phone::new()).await;
    let t = Some(token.as_str());
    let put = |body: Value| r.call("PUT", "/devices/self/push", t, Some(body));
    let fcm = "dXk1:APA91bH-x_Yz";
    let ok = put(json!({"token": fcm, "environment": "sandbox", "provider": "fcm"})).await;
    assert_eq!(ok.status, StatusCode::NO_CONTENT);
    let d = r.devices.device(&device).await.unwrap().unwrap();
    assert_eq!(
        (d.push_token.as_deref(), d.push_provider, d.push_env),
        (
            Some(fcm),
            Some(PushProvider::Fcm),
            Some(PushEnv::Production)
        ),
        "kept as sent, and FCM has no sandbox"
    );
    for bad in [
        json!({"token": "has space", "environment": "production", "provider": "fcm"}),
        json!({"token": fcm, "environment": "production"}),
        json!({"token": fcm, "environment": "production", "provider": "gcm"}),
    ] {
        assert!(put(bad.clone()).await.status.is_client_error(), "{bad}");
    }
    let apple = put(json!({"token": "AB12", "environment": "sandbox"})).await;
    assert_eq!(apple.status, StatusCode::NO_CONTENT);
    let d = r.devices.device(&device).await.unwrap().unwrap();
    assert_eq!(d.push_provider, Some(PushProvider::Apns));
}

#[tokio::test]
async fn a_runner_that_doesnt_host_core_agents_is_never_sent_one() {
    let mut old = rig().await;
    let err = old.svc.read_agent("mac", "hc/fix login").await.unwrap_err();
    assert!(err.to_string().contains("Herdr app"), "{err}");
    assert!(old.runner.try_recv().is_err(), "nothing went to the runner");

    let mut new = rig_advertising("", vec![crate::proto::CAP_HC.into()]).await;
    let svc = new.svc.clone();
    let read = tokio::spawn(async move { svc.read_agent("mac", "hc/fix login").await });
    let frame = tokio::time::timeout(Duration::from_secs(2), new.runner.recv())
        .await
        .unwrap()
        .unwrap();
    assert!(
        matches!(&frame, Frame::Req(r) if r.op.agent() == Some("hc/fix login")),
        "{frame:?}"
    );
    read.abort();
}

#[tokio::test]
async fn a_group_shows_its_tabs_and_goes_back_a_question_unsigned() {
    let mut r = rig().await;
    let phone = Phone::new();
    let (_, token) = r.pair(&phone).await;
    let read = r.call_later(
        "GET",
        "/agents/dialog?machine_id=mac&agent=work%2Fcl",
        &token,
        Value::Null,
    );
    let req = r.req().await;
    let mut reply = dialog_reply("occ2");
    reply["dialog"]["options"][0]["provisional"] = json!(true);
    reply["dialog"]["back"] = json!(true);
    reply["dialog"]["steps"] = json!([
        {"label": "Size", "answered": true, "current": false},
        {"label": "Drink ghp_0123456789abcdefghijABCDEFGHIJ", "answered": false, "current": true},
    ]);
    r.ok(req.id, reply).await;
    let d = read.await.unwrap().body;
    assert_eq!(d["dialog"]["can_go_back"], true, "{d}");
    assert_eq!(d["dialog"]["options"][0]["provisional"], true);
    assert_eq!(d["dialog"]["options"][1]["provisional"], false);
    assert_eq!(d["dialog"]["steps"][0]["label"], "Size");
    assert_eq!(d["dialog"]["steps"][1]["current"], true);
    assert!(!d["dialog"]["steps"].to_string().contains("ghp_"), "{d}");

    let back = r.call_later(
        "POST",
        "/agents/dialog/back",
        &token,
        json!({"machine_id": "mac", "agent": "work/cl", "occurrence_id": "occ2"}),
    );
    let req = r.req().await;
    assert_eq!(
        req.op,
        Op::Back {
            agent: "work/cl".into(),
            occurrence: "occ2".into()
        }
    );
    r.ok(
        req.id,
        json!({"moved": true, "dialog": dialog_reply("occ1")["dialog"]}),
    )
    .await;
    let moved = back.await.unwrap();
    assert_eq!(moved.status, StatusCode::OK, "{}", moved.body);
    assert_eq!(moved.body["outcome"], "moved");
    // The question now showing comes back with it, masked like any dialog read.
    assert_eq!(moved.body["dialog"]["occurrence_id"], "occ1");
    assert!(!moved.body.to_string().contains("ghp_"), "{}", moved.body);

    // An unsigned pick of one question returns the next one the same way.
    let pick = r.call_later(
        "POST",
        "/agents/dialog/answer",
        &token,
        json!({"machine_id": "mac", "agent": "work/cl", "occurrence_id": "occ1", "option_id": "o1",
               "label_sha256": label_sha256(&["Yes, proceed (y)".into()])}),
    );
    let req = r.req().await;
    assert!(matches!(
        req.op,
        Op::Answer {
            deny_only: true,
            ..
        }
    ));
    r.ok(
        req.id,
        json!({"answered": true, "dialog": dialog_reply("occ3")["dialog"]}),
    )
    .await;
    let picked = pick.await.unwrap().body;
    assert_eq!(picked["next"]["occurrence_id"], "occ3", "{picked}");
    assert!(!picked.to_string().contains("ghp_"), "{picked}");

    // A stale tap is told to read the dialog again.
    let stale = r.call_later(
        "POST",
        "/agents/dialog/back",
        &token,
        json!({"machine_id": "mac", "agent": "work/cl", "occurrence_id": "occ2"}),
    );
    let req = r.req().await;
    r.err(req.id, ErrorCode::DialogChanged, "the dialog changed")
        .await;
    assert_eq!(stale.await.unwrap().status, StatusCode::CONFLICT);
}

#[tokio::test]
async fn new_session_says_where_it_opens_and_takes_a_choice() {
    let mut r = rig_advertising("", vec![crate::proto::CAP_START_BACKEND.into()]).await;
    let (_, token) = r.pair(&Phone::new()).await;
    let call = r.call_later("GET", "/machines/mac/start-options", &token, Value::Null);
    let req = r.req().await;
    assert_eq!(req.op, Op::StartOptions);
    r.ok(
        req.id,
        json!({"default": "herdr", "backends": [
            {"backend": "hc", "available": false, "reason": "not_answering"},
            {"backend": "herdr", "available": true}]}),
    )
    .await;
    let o = call.await.unwrap();
    assert_eq!(o.status, StatusCode::OK, "{}", o.body);
    assert_eq!(
        o.body,
        json!({"machine_id": "mac", "default": "herdr", "backends": [
            {"backend": "hc", "available": false, "reason": "not_answering"},
            {"backend": "herdr", "available": true}]})
    );

    let bad = r
        .call(
            "POST",
            "/sessions",
            Some(&token),
            Some(
                json!({"project_id": "switchboard", "prompt": "x", "request_id": "rq-x",
                        "backend": "tmux"}),
            ),
        )
        .await;
    assert_eq!(bad.status, StatusCode::BAD_REQUEST, "{}", bad.body);

    let body = json!({"project_id": "switchboard", "prompt": "fix it", "request_id": "rq-hc",
                      "backend": "hc"});
    let call = r.call_later("POST", "/sessions", &token, body);
    let req = r.req().await;
    let Op::StartSession { backend, .. } = &req.op else {
        panic!("{:?}", req.op);
    };
    assert_eq!(*backend, Some(crate::proto::StartBackend::Hc));
    r.err(
        req.id,
        ErrorCode::BackendUnavailable,
        "the Herdr app isn't answering on this machine",
    )
    .await;
    let failed = call.await.unwrap();
    assert_eq!(failed.status, StatusCode::CONFLICT, "{}", failed.body);
    assert_eq!(failed.body["error"]["code"], "backend_unavailable");
}

#[tokio::test]
async fn an_older_runner_offers_no_choice_and_refuses_one() {
    let r = rig().await;
    let (_, token) = r.pair(&Phone::new()).await;
    let o = r
        .call("GET", "/machines/mac/start-options", Some(&token), None)
        .await;
    assert_eq!(o.status, StatusCode::OK, "{}", o.body);
    assert!(o.body.get("default").is_none());
    assert!(
        o.body["backends"]
            .as_array()
            .unwrap()
            .iter()
            .all(|b| b["available"] == false && b["reason"] == "runner_too_old"),
        "{}",
        o.body
    );
    let refused = r
        .call(
            "POST",
            "/sessions",
            Some(&token),
            Some(
                json!({"project_id": "switchboard", "prompt": "x", "request_id": "rq-old",
                        "backend": "herdr"}),
            ),
        )
        .await;
    assert_eq!(refused.status, StatusCode::CONFLICT, "{}", refused.body);
    assert_eq!(refused.body["error"]["code"], "backend_unavailable");
    let missing = r
        .call("GET", "/machines/nowhere/start-options", Some(&token), None)
        .await;
    assert_eq!(missing.status, StatusCode::BAD_REQUEST, "{}", missing.body);
}

#[tokio::test]
async fn a_herdr_app_session_lists_under_the_space_its_agent_is_in() {
    let mut r = rig().await;
    let (_, token) = r.pair(&Phone::new()).await;
    let sid = r.start(&token, "rq-space").await;
    let call = r.call_later("GET", "/agents", &token, Value::Null);
    let req = r.req().await;
    assert!(matches!(req.op, Op::Agents));
    r.ok(
        req.id,
        json!([{"name": "sb-x", "pane_id": "t4", "kind": "claude", "agent_status": "idle",
                "session_id": sid, "herdr_session": "hc", "space": "kraft"}]),
    )
    .await;
    assert_eq!(call.await.unwrap().status, StatusCode::OK);
    let s = r
        .call("GET", &format!("/sessions/{sid}"), Some(&token), None)
        .await;
    assert_eq!(s.body["space"], "kraft", "{}", s.body);
}

#[tokio::test]
async fn a_phone_registers_its_card_and_reads_the_masked_board() {
    let mut r = rig().await;
    let (device, token) = r.pair(&Phone::new()).await;
    let t = Some(token.as_str());
    let key = p256::SecretKey::from_slice(&[0x11; 32]).unwrap();
    let live_key = base64::Engine::encode(
        &base64::engine::general_purpose::URL_SAFE_NO_PAD,
        key.public_key().to_sec1_bytes(),
    );
    let put = |body: Value| r.call("PUT", "/devices/self/live", t, Some(body));
    for bad in [
        json!({"live_key": "not a key"}),
        json!({"live_key": "AAAA"}),
        json!({"activity_token": "zz"}),
    ] {
        assert!(put(bad.clone()).await.status.is_client_error(), "{bad}");
    }
    let ok = put(
        json!({"live_key": live_key, "start_token": "AB", "activity_token": "CD",
                        "activity_started_at": 5}),
    )
    .await;
    assert_eq!(ok.status, StatusCode::NO_CONTENT);
    let reg = r
        .svc
        .store()
        .live_registration(&device)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        (
            reg.start_token.as_deref(),
            reg.activity_token.as_deref(),
            reg.activity_started_at
        ),
        (Some("ab"), Some("cd"), Some(5))
    );
    assert_eq!(r.svc.store().live_targets().await.unwrap().len(), 1);
    assert!(!reg.always, "while working is the default");
    let always = put(json!({"live_key": live_key, "mode": "always"})).await;
    assert_eq!(always.status, StatusCode::NO_CONTENT);
    assert!(
        r.svc
            .store()
            .live_registration(&device)
            .await
            .unwrap()
            .unwrap()
            .always
    );
    assert!(
        put(json!({"live_key": live_key, "mode": "sometimes"}))
            .await
            .status
            .is_client_error()
    );

    let call = r.call_later("GET", "/live", &token, Value::Null);
    let req = r.req().await;
    assert!(matches!(req.op, Op::Agents), "{:?}", req.op);
    let mut agents = agents_reply();
    agents[1]["name"] = json!("ghp_abcdefghijklmnopqrstuvwxyz0123456789");
    agents[1]["agent_status"] = json!("working");
    r.ok(req.id, agents).await;
    let board = call.await.unwrap();
    assert_eq!(board.status, StatusCode::OK, "{}", board.body);
    assert_eq!(
        board.body["waiting"][0]["name"], "claude",
        "named as the app does"
    );
    assert_eq!(board.body["more"], 0);
    let running = board.body["running"].to_string();
    assert!(running.contains("sb-task"), "{running}");
    assert!(
        !running.contains("ghp_abcdef"),
        "names are masked: {running}"
    );
    assert_eq!(board.body["idle"], 0);

    let off = r.call("DELETE", "/devices/self/live", t, None).await;
    assert_eq!(off.status, StatusCode::NO_CONTENT);
    assert!(r.svc.store().live_targets().await.unwrap().is_empty());
}

#[tokio::test]
async fn a_changed_files_diff_comes_from_the_runner_masked() {
    let mut r = rig_advertising("", vec![crate::proto::CAP_DIFF.into()]).await;
    let (_, token) = r.pair(&Phone::new()).await;
    let sid = r.start(&token, "d1").await;
    let call = r.call_later(
        "GET",
        &format!("/sessions/{sid}/diff?path=src/a.rs&from=2"),
        &token,
        Value::Null,
    );
    let req = r.req().await;
    match &req.op {
        Op::Diff {
            session_id,
            path,
            from,
        } => assert_eq!(
            (session_id.as_str(), path.as_str(), *from),
            (sid.as_str(), "src/a.rs", 2)
        ),
        other => panic!("expected a diff, got {other:?}"),
    }
    r.ok(
        req.id,
        json!({"path": "src/a.rs", "binary": false, "size": null, "added": 1, "deleted": 0, "hunk_count": 3, "next": null,
               "hunks": [{"header": "@@ -1 +1,2 @@", "old_start": 1, "new_start": 1, "cut": false, "lines": [
                   {"kind": " ", "old": 1, "new": 1, "text": "fn main() {}"},
                   {"kind": "+", "old": null, "new": 2, "text": "let token = \"ghp_abcdefghijklmnopqrstuvwxyz0123456789\";"}]}]}),
    )
    .await;
    let got = call.await.unwrap();
    assert_eq!(got.status, StatusCode::OK, "{}", got.body);
    let added = got.body["hunks"][0]["lines"][1]["text"].as_str().unwrap();
    assert!(!added.contains("ghp_"), "{added}");
    assert_eq!(got.body["hunks"][0]["lines"][1]["new"], 2);
    assert_eq!(got.body["withheld"], false);

    // A key file never reaches the runner.
    let key = r
        .call(
            "GET",
            &format!("/sessions/{sid}/diff?path=deploy/server.pem"),
            Some(&token),
            None,
        )
        .await;
    assert_eq!(
        (key.status, key.body["withheld"].as_bool()),
        (StatusCode::OK, Some(true))
    );
    assert!(r.runner.try_recv().is_err());
}

#[tokio::test]
async fn an_older_runner_is_never_asked_for_a_diff() {
    let mut r = rig().await;
    let (_, token) = r.pair(&Phone::new()).await;
    let sid = r.start(&token, "d2").await;
    let got = r
        .call(
            "GET",
            &format!("/sessions/{sid}/diff?path=src/a.rs"),
            Some(&token),
            None,
        )
        .await;
    assert_eq!(got.status, StatusCode::BAD_REQUEST, "{}", got.body);
    assert!(r.runner.try_recv().is_err());
}

#[tokio::test]
async fn ship_buttons_are_kept_on_the_hub_per_scope() {
    let r = rig().await;
    let (_, token) = r.pair(&Phone::new()).await;
    let t = Some(token.as_str());
    let empty = r.call("GET", "/ship-buttons", t, None).await;
    assert_eq!(empty.body, json!({"all": [], "projects": []}));
    let put = |body: Value| r.call("PUT", "/ship-buttons", t, Some(body));
    let ok = put(json!({"scope": "*", "buttons": [
        {"skill": "/pr", "label": "Raise PR"},
        {"skill": "commit", "label": "Commit only"},
        {"skill": "pr", "label": "Twice"},
        {"skill": "$fix", "label": "Codex fix"},
        {"skill": "triage-reviews", "label": "Triage reviews", "after_pr": true},
    ]}))
    .await;
    assert_eq!(ok.status, StatusCode::NO_CONTENT, "{}", ok.body);
    assert_eq!(
        put(json!({"scope": "switchboard", "buttons": [{"skill": "ship", "label": "Ship"}]}))
            .await
            .status,
        StatusCode::NO_CONTENT
    );
    let got = r.call("GET", "/ship-buttons", t, None).await;
    assert_eq!(
        got.body["all"],
        json!([{"skill": "/pr", "label": "Raise PR", "after_pr": false},
               {"skill": "/commit", "label": "Commit only", "after_pr": false},
               {"skill": "$fix", "label": "Codex fix", "after_pr": false},
               {"skill": "/triage-reviews", "label": "Triage reviews", "after_pr": true}]),
        "a bare name becomes a slash command, a repeated skill is kept once"
    );
    assert_eq!(got.body["projects"][0]["project"], "switchboard");
    for bad in [
        json!({"scope": "nope", "buttons": []}),
        json!({"scope": "*", "buttons": [{"skill": "pr; rm -rf /", "label": "x"}]}),
        json!({"scope": "*", "buttons": [{"skill": "/", "label": "x"}]}),
        json!({"scope": "*", "buttons": [{"skill": "pr", "label": ""}]}),
        json!({"scope": "*", "buttons": [{"skill": "pr", "label": "two\nlines"}]}),
    ] {
        assert!(put(bad.clone()).await.status.is_client_error(), "{bad}");
    }
    assert_eq!(
        put(json!({"scope": "switchboard", "buttons": []}))
            .await
            .status,
        StatusCode::NO_CONTENT
    );
    assert_eq!(
        r.call("GET", "/ship-buttons", t, None).await.body["projects"],
        json!([])
    );
}

#[tokio::test]
async fn while_you_were_away_lists_what_each_agent_did_masked() {
    let mut r = rig().await;
    let (_, token) = r.pair(&Phone::new()).await;
    let t = Some(token.as_str());
    let sid = r.start(&token, "aw1").await;
    r.approval(
        &sid,
        "a1",
        "Bash",
        json!({"command": "deploy --token=supersecretvalue"}),
    )
    .await;
    let denied = r
        .call(
            "POST",
            "/approvals/a1/decision",
            t,
            Some(json!({"decision": "deny", "request_id": "d1"})),
        )
        .await;
    assert!(denied.status.is_success(), "{}", denied.body);
    let _ = r.req().await;
    // Two listings: an agent Switchboard didn't start goes from working to idle.
    for status in ["working", "idle"] {
        let call = r.call_later("GET", "/agents", &token, Value::Null);
        let req = r.req().await;
        let mut agents = agents_reply();
        agents[1]["agent_status"] = json!(status);
        r.ok(req.id, agents).await;
        assert_eq!(call.await.unwrap().status, StatusCode::OK);
    }
    // A session's turn, from the runner's own status events.
    for (status, revision) in [("working", 5), ("done", 6)] {
        r.event(Event::AgentStatus {
            session_id: sid.clone(),
            agent_status: status.into(),
            revision,
        })
        .await;
    }
    let got = r.call("GET", "/activity?since=0", t, None).await;
    assert_eq!(got.status, StatusCode::OK, "{}", got.body);
    let body = got.body.to_string();
    assert!(!body.contains("supersecretvalue"), "{body}");
    let agents = got.body["agents"].as_array().unwrap();
    let task = agents
        .iter()
        .find(|a| a["agent"] == "mono1/w1:p2")
        .expect("the listed agent");
    let kinds: Vec<_> = task["facts"]
        .as_array()
        .unwrap()
        .iter()
        .map(|f| f["kind"].as_str().unwrap())
        .collect();
    assert_eq!(
        kinds,
        ["finished"],
        "a first sighting records nothing, then the turn's end"
    );
    assert_eq!(task["state"], "idle");
    let session = agents
        .iter()
        .find(|a| a["session_id"] == sid.as_str())
        .expect("the session");
    let kinds: Vec<_> = session["facts"]
        .as_array()
        .unwrap()
        .iter()
        .map(|f| f["kind"].as_str().unwrap())
        .collect();
    for k in ["started", "asked", "denied", "finished"] {
        assert!(kinds.contains(&k), "{k} in {kinds:?}");
    }
    assert_eq!(session["state"], "idle");
    assert_eq!(got.body["summary_on"], false);
    let later = r
        .call(
            "GET",
            &format!("/activity?since={}", got.body["now"].as_i64().unwrap() + 1),
            t,
            None,
        )
        .await;
    assert_eq!(later.body["agents"], json!([]));
}

#[tokio::test]
async fn the_summary_is_off_until_the_owner_turns_it_on_and_runs_on_the_runner() {
    let mut r = rig_advertising("", vec![crate::proto::CAP_SUMMARY.into()]).await;
    let (_, token) = r.pair(&Phone::new()).await;
    let t = Some(token.as_str());
    assert_eq!(
        r.call("GET", "/settings/away", t, None).await.body,
        json!({"summary": false, "model": "haiku"})
    );
    let off = r
        .call("POST", "/activity/summary", t, Some(json!({"since": 0})))
        .await;
    assert_eq!(off.status, StatusCode::BAD_REQUEST, "{}", off.body);
    let bad = r
        .call(
            "PUT",
            "/settings/away",
            t,
            Some(json!({"summary": true, "model": "haiku; rm -rf /"})),
        )
        .await;
    assert!(bad.status.is_client_error());
    let on = r
        .call(
            "PUT",
            "/settings/away",
            t,
            Some(json!({"summary": true, "model": "haiku"})),
        )
        .await;
    assert_eq!(on.status, StatusCode::NO_CONTENT);
    let _sid = r.start(&token, "aw2").await;
    let call = r.call_later("POST", "/activity/summary", &token, json!({"since": 0}));
    let req = r.req().await;
    match &req.op {
        Op::Summarize { prompt, model } => {
            assert_eq!(model, "haiku");
            assert!(prompt.contains("Started as"), "{prompt}");
        }
        other => panic!("expected a summary, got {other:?}"),
    }
    r.ok(
        req.id,
        json!({"summary": "One task started. Its key was sk-ant-abcdefghijklmnopqrstuv."}),
    )
    .await;
    let got = call.await.unwrap();
    assert_eq!(got.status, StatusCode::OK, "{}", got.body);
    assert!(
        !got.body["summary"].as_str().unwrap().contains("sk-ant"),
        "the summary is masked too"
    );
}

#[tokio::test]
async fn a_sessions_card_name_is_its_label_not_its_agents_internal_name() {
    let mut r = rig().await;
    let (_, token) = r.pair(&Phone::new()).await;
    let sid = r.start(&token, "lbl").await;
    let label = r
        .call("GET", &format!("/sessions/{sid}"), Some(&token), None)
        .await
        .body["label"]
        .as_str()
        .unwrap()
        .to_owned();
    let call = r.call_later("GET", "/live", &token, Value::Null);
    let req = r.req().await;
    assert!(matches!(req.op, Op::Agents), "{:?}", req.op);
    r.ok(
        req.id,
        json!([{"name": "sb-dfafaaa9", "pane_id": "w9:p1", "kind": "claude", "agent_status": "working",
                "session_id": sid, "herdr_session": "switchboard-1a2b"}]),
    )
    .await;
    let board = call.await.unwrap();
    assert_eq!(board.body["running"], json!([label]), "{}", board.body);
}

// Renames, and a start into a space the owner already has.

fn managing() -> Vec<String> {
    vec![
        crate::proto::CAP_HC.into(),
        crate::proto::CAP_START_BACKEND.into(),
        crate::proto::CAP_MANAGE.into(),
    ]
}

#[tokio::test]
async fn the_lists_say_what_can_be_renamed_and_a_start_takes_a_space() {
    let mut r = rig_advertising("", managing()).await;
    let (_, token) = r.pair(&Phone::new()).await;
    let sid = r.start(&token, "rq-m1").await;
    let call = r.call_later("GET", "/agents", &token, Value::Null);
    let req = r.req().await;
    r.ok(
        req.id,
        json!([
            {"name": "sb-x", "pane_id": "t4", "kind": "claude", "agent_status": "idle",
             "session_id": sid, "herdr_session": "hc", "space": "kraft", "space_id": "sp1"},
            {"name": "hc/2", "pane_id": "t5", "kind": "claude", "agent_status": "idle",
             "herdr_session": "hc", "space": "kraft", "space_id": "sp1", "can_rename": true},
            {"name": "hc/review", "pane_id": "t6", "kind": "claude", "agent_status": "idle",
             "herdr_session": "hc", "space": "core"},
        ]),
    )
    .await;
    let agents = call.await.unwrap().body;
    let a = &agents["agents"];
    assert_eq!(a[0]["space_id"], "sp1", "{agents}");
    assert_eq!(a[0]["can_rename"], true);
    assert!(
        a[1].get("space_id").is_none(),
        "outside every space: {agents}"
    );
    assert_eq!(a[1]["can_rename"], false);
    let s = r
        .call("GET", &format!("/sessions/{sid}"), Some(&token), None)
        .await
        .body;
    assert_eq!(s["space_id"], "sp1", "{s}");
    assert_eq!(s["can_rename"], true);

    let start = json!({"project_id": "switchboard", "prompt": "another task in that space",
                       "request_id": "rq-m2", "space_id": "sp1"});
    let call = r.call_later("POST", "/sessions", &token, start.clone());
    let req = r.req().await;
    let Op::StartSession {
        session_id,
        backend,
        space,
        ..
    } = &req.op
    else {
        panic!("{:?}", req.op);
    };
    assert_eq!(space.as_deref(), Some("sp1"));
    assert_eq!(*backend, Some(crate::proto::StartBackend::Hc));
    r.ok(
        req.id,
        json!({"session_id": session_id, "workspace_id": "sp1", "pane_id": "t7",
               "herdr_session": "hc"}),
    )
    .await;
    assert_eq!(call.await.unwrap().status, StatusCode::OK);

    // A Herdr terminal has no such space; nothing reaches the runner.
    let mut in_terminal = start.clone();
    in_terminal["backend"] = json!("herdr");
    in_terminal["request_id"] = json!("rq-m3");
    let reply = r
        .call("POST", "/sessions", Some(&token), Some(in_terminal))
        .await;
    assert_eq!(reply.status, StatusCode::BAD_REQUEST, "{}", reply.body);
    assert!(r.runner.try_recv().is_err());
}

#[tokio::test]
async fn sessions_agents_and_spaces_are_renamed_through_the_api() {
    let mut r = rig_advertising("", managing()).await;
    let (_, token) = r.pair(&Phone::new()).await;
    let sid = r.start(&token, "rq-n1").await;

    let unsigned = r
        .call(
            "POST",
            &format!("/sessions/{sid}/rename"),
            None,
            Some(json!({"name": "x"})),
        )
        .await;
    assert_eq!(unsigned.status, StatusCode::UNAUTHORIZED);

    let call = r.call_later(
        "POST",
        &format!("/sessions/{sid}/rename"),
        &token,
        json!({"name": "login fix"}),
    );
    let req = r.req().await;
    assert!(matches!(req.op, Op::Agents), "the clash check lists agents");
    r.ok(req.id, json!([])).await;
    let reply = call.await.unwrap();
    assert_eq!(reply.status, StatusCode::OK, "{}", reply.body);
    assert_eq!(reply.body["name"], "login fix");
    let s = r
        .call("GET", &format!("/sessions/{sid}"), Some(&token), None)
        .await
        .body;
    assert_eq!(s["label"], "login fix", "{s}");
    let bad = r
        .call(
            "POST",
            &format!("/sessions/{sid}/rename"),
            Some(&token),
            Some(json!({"name": "  "})),
        )
        .await;
    assert_eq!(bad.status, StatusCode::BAD_REQUEST);

    let call = r.call_later(
        "POST",
        "/agents/rename",
        &token,
        json!({"machine_id": "mac", "agent": "hc/2", "name": "reviewer", "request_id": "rq-n2"}),
    );
    let req = r.req().await;
    assert_eq!(
        req.op,
        Op::RenameAgent {
            agent: "hc/2".into(),
            name: "reviewer".into()
        }
    );
    r.ok(req.id, json!({"name": "hc/reviewer"})).await;
    let reply = call.await.unwrap();
    assert_eq!(reply.status, StatusCode::OK, "{}", reply.body);
    assert_eq!(reply.body["name"], "hc/reviewer");

    let call = r.call_later(
        "POST",
        "/agents/rename",
        &token,
        json!({"machine_id": "mac", "agent": "hc/reviewer", "name": "tester"}),
    );
    let req = r.req().await;
    r.err(
        req.id,
        ErrorCode::NameTaken,
        "another agent is already called tester",
    )
    .await;
    let reply = call.await.unwrap();
    assert_eq!(reply.status, StatusCode::CONFLICT);
    assert_eq!(reply.body["error"]["code"], "name_taken", "{}", reply.body);

    let call = r.call_later(
        "POST",
        "/spaces/rename",
        &token,
        json!({"machine_id": "mac", "herdr_session": "hc", "space_id": "sp1",
               "name": "sk-ant-abcdefghijklmnopqrstuv"}),
    );
    let req = r.req().await;
    let Op::RenameSpace { space_id, name, .. } = &req.op else {
        panic!("{:?}", req.op);
    };
    assert_eq!(space_id, "sp1");
    assert!(
        !name.contains("abcdefgh"),
        "masked before it leaves the hub"
    );
    let masked = name.clone();
    r.ok(req.id, json!({ "name": masked })).await;
    let reply = call.await.unwrap();
    assert_eq!(reply.status, StatusCode::OK, "{}", reply.body);
    assert!(!reply.body.to_string().contains("abcdefgh"));
}

#[tokio::test]
async fn an_older_runner_is_never_sent_a_rename() {
    let mut r = rig().await;
    let (_, token) = r.pair(&Phone::new()).await;
    for (path, body) in [
        (
            "/agents/rename",
            json!({"machine_id": "mac", "agent": "main/old", "name": "new"}),
        ),
        (
            "/spaces/rename",
            json!({"machine_id": "mac", "herdr_session": "main", "space_id": "w2", "name": "x"}),
        ),
        (
            "/sessions",
            json!({"project_id": "switchboard", "prompt": "do it", "request_id": "rq-o1",
                   "space_id": "sp1"}),
        ),
    ] {
        let reply = r.call("POST", path, Some(&token), Some(body)).await;
        assert_eq!(reply.status, StatusCode::CONFLICT, "{path}: {}", reply.body);
    }
    assert!(r.runner.try_recv().is_err(), "nothing went to the runner");
    let offline = r
        .call(
            "POST",
            "/agents/rename",
            Some(&token),
            Some(json!({"machine_id": "nowhere", "agent": "a", "name": "b"})),
        )
        .await;
    assert_eq!(offline.status, StatusCode::BAD_REQUEST, "unknown machine");
}
