//! A real relay and a real hub link over local WebSockets, with the phone side in Rust.

use std::sync::Arc;
use std::time::Duration;

use base64::Engine;
use base64::engine::general_purpose::{STANDARD as B64STD, URL_SAFE_NO_PAD as B64};
use serde_json::{Value, json};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

use super::seal::{HubKeys, PlainRequest, phone};
use super::server::{Relay, serve};
use crate::auth::device::tests::Phone;
use crate::auth::device::{MESSAGE_VERSION, token_message};
use crate::config::HubConfig;
use crate::hub::relay_link::{RelayLink, RelaySender};
use crate::hub::service::{Clock, SessionService};
use crate::push::{AnySender, LogSender, Push, SendOutcome, Sender};
use crate::store::{OutboxKind, PushEnv, Store};

fn hub_config() -> HubConfig {
    HubConfig::parse(&format!(
        "public_url = \"http://localhost:1\"\nlisten = \"127.0.0.1:0\"\ndatabase = \"/d\"\n\
         [[machines]]\nid = \"mac\"\ntoken_hash = \"{}\"\n",
        "0".repeat(64)
    ))
    .unwrap()
}

struct Rig {
    relay_addr: std::net::SocketAddr,
    relay: Arc<Relay>,
    hub: crate::hub::Hub,
    link: Arc<RelayLink>,
    link_task: tokio::task::JoinHandle<()>,
    kx: Vec<u8>,
}

async fn start_link(
    addr: std::net::SocketAddr,
    keys: HubKeys,
    app: axum::Router,
) -> (Arc<RelayLink>, tokio::task::JoinHandle<()>) {
    let link = RelayLink::new(format!("ws://{addr}/hub"), keys, app, Clock::System);
    let task = link.spawn();
    let mut ready = link.ready();
    tokio::time::timeout(Duration::from_secs(5), ready.wait_for(|r| *r))
        .await
        .expect("the hub registers with the relay")
        .unwrap();
    (link, task)
}

async fn rig_with(push: Option<AnySender>) -> Rig {
    let relay = Relay::new(push);
    let (relay_addr, _) = serve(
        relay.clone(),
        TcpListener::bind("127.0.0.1:0").await.unwrap(),
    );
    let store = Store::open_in_memory().await.unwrap();
    let svc = SessionService::new(hub_config(), store, Clock::System);
    let hub = crate::hub::serve(svc, TcpListener::bind("127.0.0.1:0").await.unwrap()).unwrap();
    let keys = HubKeys::generate();
    let kx = keys.kx_public();
    let (link, link_task) = start_link(relay_addr, keys, hub.app.clone()).await;
    Rig {
        relay_addr,
        relay,
        hub,
        link,
        link_task,
        kx,
    }
}

/// A plain HTTP/1.1 POST; returns the status and the JSON body.
async fn post(addr: std::net::SocketAddr, path: &str, body: &Value) -> (u16, Value) {
    let mut s = tokio::net::TcpStream::connect(addr).await.unwrap();
    let body = body.to_string();
    let req = format!(
        "POST {path} HTTP/1.1\r\nhost: relay\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
        body.len()
    );
    s.write_all(req.as_bytes()).await.unwrap();
    let mut out = String::new();
    s.read_to_string(&mut out).await.unwrap();
    let status = out[9..12].parse().unwrap();
    let json = out
        .split("\r\n\r\n")
        .nth(1)
        .and_then(|b| serde_json::from_str(b).ok())
        .unwrap_or(Value::Null);
    (status, json)
}

impl Rig {
    /// A phone request through the relay: sealed, forwarded, answered, opened.
    async fn call(
        &self,
        method: &str,
        path: &str,
        token: Option<&str>,
        body: Option<Value>,
    ) -> (u16, Value) {
        let (env, pending) = phone::seal(&self.kx, &request(method, path, token, body));
        let (status, reply) = post(
            self.relay_addr,
            &format!("/h/{}", self.link.relay_id()),
            &serde_json::to_value(&env).unwrap(),
        )
        .await;
        assert_eq!(status, 200, "relay: {reply}");
        let resp = pending
            .open(&serde_json::from_value(reply).unwrap())
            .unwrap();
        (
            resp.status,
            serde_json::from_slice(&resp.body_bytes().unwrap()).unwrap_or(Value::Null),
        )
    }
}

fn request(method: &str, path: &str, token: Option<&str>, body: Option<Value>) -> PlainRequest {
    let mut headers = vec![("content-type".to_owned(), "application/json".to_owned())];
    if let Some(t) = token {
        headers.push(("authorization".into(), format!("Bearer {t}")));
    }
    PlainRequest {
        ts: Clock::System.now_ms(),
        method: method.into(),
        path: path.into(),
        headers,
        body: body
            .map(|b| B64STD.encode(b.to_string()))
            .unwrap_or_default(),
    }
}

#[tokio::test]
async fn a_phone_pairs_signs_in_and_reads_sessions_through_the_relay() {
    let r = rig_with(None).await;
    let (status, meta) = r.call("GET", "/app/v1/meta", None, None).await;
    assert_eq!(status, 200);
    let hub_id = meta["hub_id"].as_str().unwrap().to_owned();

    let devices = crate::auth::device::Devices::new(r.hub.service.store().clone(), Clock::System);
    let code = devices.issue_code().await.unwrap();
    let phone_keys = Phone::new();
    let (status, paired) = r
        .call(
            "POST",
            "/app/v1/pair",
            None,
            Some(json!({"code": code.code, "name": "iPhone",
            "auth_key": phone_keys.auth.public(), "approval_key": phone_keys.approval.public()})),
        )
        .await;
    assert_eq!(status, 200, "{paired}");
    let device = paired["device_id"].as_str().unwrap().to_owned();
    devices.confirm(&device, true).await.unwrap();

    let (_, ch) = r
        .call(
            "POST",
            "/app/v1/auth/challenge",
            None,
            Some(json!({"device_id": device})),
        )
        .await;
    let msg = token_message(
        &hub_id,
        &device,
        ch["nonce"].as_str().unwrap(),
        ch["expires_at"].as_i64().unwrap(),
    );
    let (status, t) = r
        .call(
            "POST",
            "/app/v1/auth/token",
            None,
            Some(json!({"device_id": device, "nonce": ch["nonce"],
            "signature": phone_keys.auth.sign(&msg), "message_version": MESSAGE_VERSION})),
        )
        .await;
    assert_eq!(status, 200, "{t}");
    let token = t["access_token"].as_str().unwrap();
    let (status, list) = r.call("GET", "/app/v1/sessions", Some(token), None).await;
    assert_eq!((status, list["sessions"].clone()), (200, json!([])));
    let (status, _) = r.call("GET", "/app/v1/sessions", None, None).await;
    assert_eq!(
        status, 401,
        "the hub's own auth still applies through the relay"
    );
}

#[tokio::test]
async fn tampered_replayed_and_misrouted_requests_are_refused() {
    let r = rig_with(None).await;
    let rid = r.link.relay_id();
    let (env, _) = phone::seal(&r.kx, &request("GET", "/app/v1/meta", None, None));
    let path = format!("/h/{rid}");

    let mut bad = env.clone();
    let mut ct = B64.decode(&bad.ct).unwrap();
    ct[3] ^= 1;
    bad.ct = B64.encode(ct);
    let (status, body) = post(r.relay_addr, &path, &serde_json::to_value(&bad).unwrap()).await;
    assert_eq!(
        (status, body["error"]["code"].as_str()),
        (400, Some("rejected"))
    );

    let (status, _) = post(r.relay_addr, &path, &serde_json::to_value(&env).unwrap()).await;
    assert_eq!(status, 200, "the untouched envelope still goes through");

    let (stale, _) = phone::seal(
        &r.kx,
        &PlainRequest {
            ts: 1,
            ..request("GET", "/app/v1/meta", None, None)
        },
    );
    let (status, body) = post(r.relay_addr, &path, &serde_json::to_value(&stale).unwrap()).await;
    assert_eq!(
        (status, body["error"]["code"].as_str()),
        (400, Some("rejected"))
    );

    // The runner endpoint and anything else outside the app API stays out of reach.
    let (status, _) = r.call("GET", "/runner", None, None).await;
    assert_eq!(status, 404);

    let other = "0".repeat(32);
    let (status, body) = post(
        r.relay_addr,
        &format!("/h/{other}"),
        &serde_json::to_value(&stale).unwrap(),
    )
    .await;
    assert_eq!(
        (status, body["error"]["code"].as_str()),
        (503, Some("hub_offline"))
    );
    let (status, _) = post(
        r.relay_addr,
        "/h/not-an-id",
        &serde_json::to_value(&stale).unwrap(),
    )
    .await;
    assert_eq!(status, 400);
}

#[tokio::test]
async fn a_replayed_envelope_gets_the_hubs_refusal_in_the_clear() {
    let r = rig_with(None).await;
    let path = format!("/h/{}", r.link.relay_id());
    let (env, pending) = phone::seal(&r.kx, &request("GET", "/app/v1/meta", None, None));
    let body = serde_json::to_value(&env).unwrap();
    let (_, first) = post(r.relay_addr, &path, &body).await;
    assert_eq!(
        pending
            .open(&serde_json::from_value(first).unwrap())
            .unwrap()
            .status,
        200
    );
    let (status, second) = post(r.relay_addr, &path, &body).await;
    assert_eq!(
        (status, second["error"]["code"].as_str()),
        (400, Some("rejected")),
        "a captured request can't be sent again, and its refusal isn't sealed"
    );
}

#[tokio::test]
async fn a_hub_reconnects_under_the_same_relay_id_and_an_offline_hub_is_reported() {
    let r = rig_with(None).await;
    let rid = r.link.relay_id();
    r.link_task.abort();
    tokio::time::timeout(Duration::from_secs(5), async {
        while r.relay.connected(&rid) {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("the relay notices the hub went away");
    let (env, _) = phone::seal(&r.kx, &request("GET", "/app/v1/meta", None, None));
    let (status, body) = post(
        r.relay_addr,
        &format!("/h/{rid}"),
        &serde_json::to_value(&env).unwrap(),
    )
    .await;
    assert_eq!(
        (status, body["error"]["code"].as_str()),
        (503, Some("hub_offline"))
    );

    // The hub's keys live in its key file; the same keys come back under the same id.
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("relay-key.json");
    let keys = HubKeys::load_or_create(&path).unwrap();
    let id = keys.relay_id();
    let kx = keys.kx_public();
    let (link, task) = start_link(r.relay_addr, keys, r.hub.app.clone()).await;
    task.abort();
    drop(link);
    let (link, _task) = start_link(
        r.relay_addr,
        HubKeys::load_or_create(&path).unwrap(),
        r.hub.app.clone(),
    )
    .await;
    assert_eq!(link.relay_id(), id);
    let rig = Rig { link, kx, ..r };
    assert_eq!(rig.call("GET", "/app/v1/meta", None, None).await.0, 200);
}

#[tokio::test]
async fn a_hub_with_someone_elses_keys_but_the_wrong_signature_is_turned_away() {
    use futures_util::{SinkExt, StreamExt};
    use tokio_tungstenite::tungstenite::Message;
    let relay = Relay::new(None);
    let (addr, _) = serve(
        relay.clone(),
        TcpListener::bind("127.0.0.1:0").await.unwrap(),
    );
    let victim = HubKeys::generate();
    let (ws, _) = tokio_tungstenite::connect_async(format!("ws://{addr}/hub"))
        .await
        .unwrap();
    let (mut sink, mut stream) = ws.split();
    let hello = json!({"type": "hello", "version": super::seal::VERSION, "kx_key": B64.encode(victim.kx_public()),
                       "sign_key": B64.encode(victim.sign_public())});
    sink.send(Message::text(hello.to_string())).await.unwrap();
    let _challenge = stream.next().await.unwrap().unwrap();
    let forged = HubKeys::generate().sign("whatever");
    sink.send(Message::text(
        json!({"type": "auth", "signature": forged}).to_string(),
    ))
    .await
    .unwrap();
    let next = tokio::time::timeout(Duration::from_secs(5), stream.next())
        .await
        .unwrap();
    assert!(
        !matches!(next, Some(Ok(Message::Text(_)))),
        "no ready for a forged signature: {next:?}"
    );
    assert!(!relay.connected(&victim.relay_id()));
}

fn push(kind: OutboxKind) -> Push {
    Push {
        device_id: "d1".into(),
        kind,
        session_id: Some("s1".into()),
        approval_id: Some("a1".into()),
        token: Some("ab12".into()),
        env: PushEnv::Sandbox,
        payload: json!({"ignored": "the relay writes its own"}),
        collapse_id: "a1".into(),
        expires_at: 0,
    }
}

#[tokio::test]
async fn pushes_go_out_through_the_relays_sender() {
    let r = rig_with(Some(AnySender::Log(LogSender))).await;
    let sender = RelaySender(r.link.clone());
    assert!(!sender.needs_token());
    assert_eq!(
        sender.send(&push(OutboxKind::Approval)).await,
        SendOutcome::Sent
    );
    let mut bad = push(OutboxKind::Finished);
    bad.token = Some("not hex!".into());
    assert!(matches!(sender.send(&bad).await, SendOutcome::Failed(_)));

    let none = rig_with(None).await;
    let outcome = RelaySender(none.link.clone())
        .send(&push(OutboxKind::Approval))
        .await;
    assert!(matches!(outcome, SendOutcome::Failed(e) if e.contains("doesn't send pushes")));
}

fn push_to(token: usize) -> Push {
    Push {
        token: Some(format!("{token:04x}")),
        ..push(OutboxKind::Finished)
    }
}

fn rate_limited(o: &SendOutcome) -> bool {
    matches!(o, SendOutcome::Retry(e) if e.contains("rate limit"))
}

#[tokio::test]
async fn pushes_wait_for_a_connection_and_are_rate_limited() {
    use super::server::{PUSHES_PER_DEVICE_PER_MINUTE, PUSHES_PER_MINUTE};
    let r = rig_with(Some(AnySender::Log(LogSender))).await;
    let sender = RelaySender(r.link.clone());
    for _ in 0..PUSHES_PER_DEVICE_PER_MINUTE {
        assert_eq!(sender.send(&push_to(0)).await, SendOutcome::Sent);
    }
    assert!(
        rate_limited(&sender.send(&push_to(0)).await),
        "one device gets a bounded number"
    );
    for t in 1..PUSHES_PER_MINUTE - PUSHES_PER_DEVICE_PER_MINUTE {
        assert_eq!(sender.send(&push_to(t as usize)).await, SendOutcome::Sent);
    }
    assert!(
        rate_limited(&sender.send(&push_to(9999)).await),
        "and so does one hub, across devices"
    );
    r.link_task.abort();
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert!(matches!(
        sender.send(&push(OutboxKind::Finished)).await,
        SendOutcome::Retry(_)
    ));
}

/// Through a deployed relay over real TLS and nginx: a throwaway hub dials its WebSocket and a
/// sealed phone request comes back answered. Run with
/// `SWITCHBOARD_LIVE_RELAY=https://adarshnb.com/relay cargo test --lib live_relay_round_trip`.
#[tokio::test]
async fn live_relay_round_trip() {
    use http_body_util::{BodyExt, Full};
    use hyper_util::client::legacy::Client;
    use hyper_util::rt::TokioExecutor;
    let Ok(base) = std::env::var("SWITCHBOARD_LIVE_RELAY") else {
        return;
    };
    let _ = rustls::crypto::ring::default_provider().install_default();
    let base = base.trim_end_matches('/').to_owned();
    let ws = format!("{}/hub", base.replacen("https://", "wss://", 1));
    let store = Store::open_in_memory().await.unwrap();
    let svc = SessionService::new(hub_config(), store, Clock::System);
    let hub = crate::hub::serve(svc, TcpListener::bind("127.0.0.1:0").await.unwrap()).unwrap();
    let keys = HubKeys::generate();
    let kx = keys.kx_public();
    let link = RelayLink::new(ws, keys, hub.app.clone(), Clock::System);
    let _task = link.spawn();
    tokio::time::timeout(Duration::from_secs(15), link.ready().wait_for(|r| *r))
        .await
        .expect("the hub registers with the deployed relay")
        .unwrap();

    let https = hyper_rustls::HttpsConnectorBuilder::new()
        .with_provider_and_webpki_roots(Arc::new(rustls::crypto::ring::default_provider()))
        .unwrap()
        .https_only()
        .enable_http2()
        .build();
    let client: Client<_, Full<bytes::Bytes>> = Client::builder(TokioExecutor::new()).build(https);
    let (env, pending) = phone::seal(&kx, &request("GET", "/app/v1/meta", None, None));
    let req = hyper::Request::post(format!("{base}/h/{}", link.relay_id()))
        .header("content-type", "application/json")
        .body(Full::new(serde_json::to_vec(&env).unwrap().into()))
        .unwrap();
    let resp = client.request(req).await.unwrap();
    let status = resp.status();
    let body = resp.into_body().collect().await.unwrap().to_bytes();
    assert_eq!(status, 200, "{}", String::from_utf8_lossy(&body));
    let answer = pending
        .open(&serde_json::from_slice(&body).unwrap())
        .unwrap();
    assert_eq!(answer.status, 200);
    let meta: Value = serde_json::from_slice(&answer.body_bytes().unwrap()).unwrap();
    assert!(meta["hub_id"].is_string(), "{meta}");
    println!(
        "live relay ok: relay id {}, hub {}",
        link.relay_id(),
        meta["hub_id"]
    );
}

#[tokio::test]
async fn a_hubs_push_budget_survives_a_reconnect() {
    let r = rig_with(Some(AnySender::Log(LogSender))).await;
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("relay-key.json");
    let (link, task) = start_link(
        r.relay_addr,
        HubKeys::load_or_create(&path).unwrap(),
        r.hub.app.clone(),
    )
    .await;
    for t in 0..super::server::PUSHES_PER_MINUTE {
        assert_eq!(
            RelaySender(link.clone()).send(&push_to(t as usize)).await,
            SendOutcome::Sent
        );
    }
    task.abort();
    let (link, _task) = start_link(
        r.relay_addr,
        HubKeys::load_or_create(&path).unwrap(),
        r.hub.app.clone(),
    )
    .await;
    assert!(rate_limited(&RelaySender(link).send(&push_to(5000)).await));
}

#[tokio::test]
async fn junk_envelopes_stop_at_the_relay() {
    let r = rig_with(None).await;
    let path = format!("/h/{}", r.link.relay_id());
    let (env, _) = phone::seal(&r.kx, &request("GET", "/app/v1/meta", None, None));
    let junk = [
        json!({"v": super::seal::VERSION, "epk": "AAAA", "ct": env.ct}),
        json!({"v": 1, "epk": env.epk, "ct": env.ct}),
        json!({"v": super::seal::VERSION, "epk": env.epk, "ct": "not base64!"}),
    ];
    for j in junk {
        let (status, body) = post(r.relay_addr, &path, &j).await;
        assert_eq!(
            (status, body["error"]["code"].as_str()),
            (400, Some("bad_envelope")),
            "{j}"
        );
    }
    let (status, _) = post(r.relay_addr, &path, &serde_json::to_value(&env).unwrap()).await;
    assert_eq!(status, 200);
}

#[tokio::test]
async fn a_second_connection_for_the_same_hub_closes_the_first() {
    let r = rig_with(None).await;
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("relay-key.json");
    let (first, _t1) = start_link(
        r.relay_addr,
        HubKeys::load_or_create(&path).unwrap(),
        r.hub.app.clone(),
    )
    .await;
    let mut first_ready = first.ready();
    let (_second, _t2) = start_link(
        r.relay_addr,
        HubKeys::load_or_create(&path).unwrap(),
        r.hub.app.clone(),
    )
    .await;
    tokio::time::timeout(Duration::from_secs(5), first_ready.wait_for(|r| !*r))
        .await
        .expect("the replaced connection is told, not left hanging")
        .unwrap();
}
