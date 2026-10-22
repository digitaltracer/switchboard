//! The relay server: `GET /hub` for hubs (WebSocket), `POST /h/{relay_id}` for phones, and
//! `GET /health`. Mounted behind nginx at a path such as `/relay/`.

use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::{DefaultBodyLimit, Path, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use futures_util::{SinkExt, StreamExt};
use serde_json::json;
use tokio::net::TcpListener;
use tokio::sync::{mpsc, oneshot};
use tokio::time::Instant;

use super::proto::{HubFrame, MAX_FRAME_BYTES, PushOutcome, RelayFrame, RelayPush};
use super::seal::{RequestEnvelope, ResponseEnvelope, VERSION, registration_message, relay_id};
use crate::auth::device::verify;
use crate::config::RelayConfig;
use crate::push::{AnySender, Push, SendOutcome, Sender, payload_for};
use crate::runner::random_id;
use crate::store::{OutboxKind, PushEnv};

/// How long a phone waits for its hub; long-polls take up to 25 s.
pub const REQUEST_TIMEOUT: Duration = Duration::from_secs(45);
const HELLO_TIMEOUT: Duration = Duration::from_secs(10);
const PING: Duration = Duration::from_secs(20);
/// Requests a hub may have in flight at once.
pub const MAX_IN_FLIGHT: usize = 64;
/// Pushes a hub may send per minute, across reconnects.
pub const PUSHES_PER_MINUTE: u32 = 120;
/// Pushes one device may receive per minute, whichever hub asks.
pub const PUSHES_PER_DEVICE_PER_MINUTE: u32 = 30;
/// Pushes the relay sends per minute in all, to protect the APNs key.
pub const PUSHES_PER_MINUTE_TOTAL: u32 = 3000;
/// Hub connections at once, counting ones still in the handshake.
pub const MAX_HUB_CONNECTIONS: usize = 4096;
const MAX_BODY: usize = 2 << 20;
/// An uncompressed SEC1 P-256 key.
const EPK_BYTES: usize = 65;

struct Slot {
    generation: u64,
    tx: mpsc::Sender<RelayFrame>,
    pending: HashMap<u64, oneshot::Sender<Result<ResponseEnvelope, String>>>,
    /// Closes the connection when a newer one for the same hub replaces it.
    replaced: Option<oneshot::Sender<()>>,
}

/// Fixed one-minute windows per key.
#[derive(Default)]
struct Budgets(HashMap<String, (Instant, u32)>);

impl Budgets {
    fn take(&mut self, key: &str, limit: u32) -> bool {
        let now = Instant::now();
        let window = Duration::from_secs(60);
        if self.0.len() > 10_000 {
            self.0
                .retain(|_, (start, _)| now.duration_since(*start) < window);
        }
        let entry = self.0.entry(key.to_owned()).or_insert((now, 0));
        if now.duration_since(entry.0) >= window {
            *entry = (now, 0);
        }
        entry.1 += 1;
        entry.1 <= limit
    }
}

pub struct Relay {
    hubs: Mutex<HashMap<String, Slot>>,
    budgets: Mutex<Budgets>,
    connections: AtomicUsize,
    next: AtomicU64,
    push: Option<AnySender>,
}

impl Relay {
    pub fn new(push: Option<AnySender>) -> Arc<Self> {
        Arc::new(Self {
            hubs: Mutex::default(),
            budgets: Mutex::default(),
            connections: AtomicUsize::new(0),
            next: AtomicU64::new(1),
            push,
        })
    }

    /// Spends one push from the hub's, the device's and the relay's budgets.
    fn push_allowed(&self, rid: &str, token: &str) -> bool {
        let mut b = self.budgets.lock().unwrap();
        b.take(&format!("hub:{rid}"), PUSHES_PER_MINUTE)
            && b.take(&format!("device:{token}"), PUSHES_PER_DEVICE_PER_MINUTE)
            && b.take("total", PUSHES_PER_MINUTE_TOTAL)
    }

    pub fn connected(&self, relay_id: &str) -> bool {
        self.hubs.lock().unwrap().contains_key(relay_id)
    }
}

pub fn routes(relay: Arc<Relay>) -> Router {
    Router::new()
        .route("/hub", get(hub_upgrade))
        .route("/h/{relay_id}", post(phone_request))
        .route("/health", get(|| async { "ok" }))
        .layer(DefaultBodyLimit::max(MAX_BODY))
        .with_state(relay)
}

pub async fn run(cfg: RelayConfig) -> anyhow::Result<()> {
    let _ = rustls::crypto::ring::default_provider().install_default();
    let push = cfg.push.as_ref().map(AnySender::from_config).transpose()?;
    let relay = Relay::new(push);
    let listener = TcpListener::bind(cfg.listen).await?;
    tracing::info!(addr = %cfg.listen, push = cfg.push.is_some(), "relay listening");
    axum::serve(listener, routes(relay))
        .with_graceful_shutdown(async {
            let mut term =
                tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
                    .expect("SIGTERM handler");
            tokio::select! {
                _ = tokio::signal::ctrl_c() => {}
                _ = term.recv() => {}
            }
        })
        .await?;
    Ok(())
}

/// Serves on `listener`, for tests.
pub fn serve(
    relay: Arc<Relay>,
    listener: TcpListener,
) -> (SocketAddr, tokio::task::JoinHandle<()>) {
    let addr = listener.local_addr().expect("bound");
    let task = tokio::spawn(async move {
        let _ = axum::serve(listener, routes(relay)).await;
    });
    (addr, task)
}

fn error(status: StatusCode, code: &str, message: &str) -> Response {
    (
        status,
        Json(json!({ "error": { "code": code, "message": message } })),
    )
        .into_response()
}

fn valid_relay_id(id: &str) -> bool {
    id.len() == 32
        && id
            .bytes()
            .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
}

/// Drops junk before it costs the hub's home connection anything; the hub still checks it all.
fn plausible_envelope(env: &RequestEnvelope) -> bool {
    use base64::Engine;
    let b64 = base64::engine::general_purpose::URL_SAFE_NO_PAD;
    env.v == VERSION
        && b64
            .decode(&env.epk)
            .is_ok_and(|k| k.len() == EPK_BYTES && p256::PublicKey::from_sec1_bytes(&k).is_ok())
        && env.ct.len() <= MAX_BODY
        && env
            .ct
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
}

/// Ids and collapse keys such as `finished:<id>:<revision>`; never free text.
fn valid_id(id: Option<&str>) -> bool {
    id.is_none_or(|s| {
        s.len() <= 64
            && s.bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"-_:.".contains(&b))
    })
}

/// Frees a phone's request slot however its handler ends, including when the phone hangs up.
struct PendingGuard<'a> {
    relay: &'a Relay,
    rid: &'a str,
    id: u64,
}

impl Drop for PendingGuard<'_> {
    fn drop(&mut self) {
        if let Some(slot) = self.relay.hubs.lock().unwrap().get_mut(self.rid) {
            slot.pending.remove(&self.id);
        }
    }
}

/// Counts a hub connection for as long as it lives.
struct ConnectionGuard(Arc<Relay>);

impl Drop for ConnectionGuard {
    fn drop(&mut self) {
        self.0.connections.fetch_sub(1, Ordering::Relaxed);
    }
}

// Phones.

async fn phone_request(
    State(relay): State<Arc<Relay>>,
    Path(rid): Path<String>,
    Json(env): Json<RequestEnvelope>,
) -> Response {
    if !valid_relay_id(&rid) {
        return error(StatusCode::BAD_REQUEST, "bad_relay_id", "not a relay id");
    }
    if !plausible_envelope(&env) {
        return error(
            StatusCode::BAD_REQUEST,
            "bad_envelope",
            "not a request envelope",
        );
    }
    let id = relay.next.fetch_add(1, Ordering::Relaxed);
    let (tx, rx) = oneshot::channel();
    let sent = {
        let mut hubs = relay.hubs.lock().unwrap();
        match hubs.get_mut(&rid) {
            None => None,
            Some(slot) if slot.pending.len() >= MAX_IN_FLIGHT => Some(false),
            Some(slot) => {
                let ok = slot
                    .tx
                    .try_send(RelayFrame::Request { id, envelope: env })
                    .is_ok();
                if ok {
                    slot.pending.insert(id, tx);
                }
                Some(ok)
            }
        }
    };
    match sent {
        None => {
            return error(
                StatusCode::SERVICE_UNAVAILABLE,
                "hub_offline",
                "the hub isn't connected to the relay",
            );
        }
        Some(false) => {
            return error(
                StatusCode::TOO_MANY_REQUESTS,
                "busy",
                "too many requests to this hub",
            );
        }
        Some(true) => {}
    }
    let _guard = PendingGuard {
        relay: &relay,
        rid: &rid,
        id,
    };
    match tokio::time::timeout(REQUEST_TIMEOUT, rx).await {
        Ok(Ok(Ok(envelope))) => Json(envelope).into_response(),
        Ok(Ok(Err(reason))) => error(StatusCode::BAD_REQUEST, "rejected", &reason),
        Ok(Err(_)) => error(
            StatusCode::SERVICE_UNAVAILABLE,
            "hub_offline",
            "the hub disconnected",
        ),
        Err(_) => error(
            StatusCode::GATEWAY_TIMEOUT,
            "hub_timeout",
            "the hub didn't answer in time",
        ),
    }
}

// Hubs.

async fn hub_upgrade(State(relay): State<Arc<Relay>>, ws: WebSocketUpgrade) -> Response {
    if relay.connections.fetch_add(1, Ordering::Relaxed) >= MAX_HUB_CONNECTIONS {
        relay.connections.fetch_sub(1, Ordering::Relaxed);
        return error(
            StatusCode::SERVICE_UNAVAILABLE,
            "full",
            "too many hubs connected",
        );
    }
    let guard = ConnectionGuard(relay.clone());
    ws.max_message_size(MAX_FRAME_BYTES)
        .on_upgrade(move |socket| async move {
            let _guard = guard;
            hub_connection(relay, socket).await
        })
}

async fn recv(stream: &mut futures_util::stream::SplitStream<WebSocket>) -> Option<HubFrame> {
    loop {
        match stream.next().await? {
            Ok(Message::Text(t)) => return serde_json::from_str(&t).ok(),
            Ok(Message::Close(_)) | Err(_) => return None,
            Ok(_) => {}
        }
    }
}

async fn send(
    sink: &mut futures_util::stream::SplitSink<WebSocket, Message>,
    f: &RelayFrame,
) -> bool {
    let text = serde_json::to_string(f).expect("serializes");
    sink.send(Message::Text(text.into())).await.is_ok()
}

/// Checks the hub's keys and signature; returns its relay id.
async fn register(
    sink: &mut futures_util::stream::SplitSink<WebSocket, Message>,
    stream: &mut futures_util::stream::SplitStream<WebSocket>,
) -> Option<String> {
    let Ok(Some(HubFrame::Hello {
        version,
        kx_key,
        sign_key,
    })) = tokio::time::timeout(HELLO_TIMEOUT, recv(stream)).await
    else {
        return None;
    };
    use base64::Engine;
    let b64 = base64::engine::general_purpose::URL_SAFE_NO_PAD;
    let (Ok(kx), Ok(sign)) = (b64.decode(&kx_key), b64.decode(&sign_key)) else {
        return None;
    };
    if version != VERSION || p256::PublicKey::from_sec1_bytes(&kx).is_err() {
        return None;
    }
    let nonce = random_id();
    if !send(
        sink,
        &RelayFrame::Challenge {
            nonce: nonce.clone(),
        },
    )
    .await
    {
        return None;
    }
    let Ok(Some(HubFrame::Auth { signature })) =
        tokio::time::timeout(HELLO_TIMEOUT, recv(stream)).await
    else {
        return None;
    };
    verify(
        &sign_key,
        &registration_message(&nonce, &kx_key, &sign_key),
        &signature,
    )
    .then(|| relay_id(&kx, &sign))
}

async fn hub_connection(relay: Arc<Relay>, socket: WebSocket) {
    let (mut sink, mut stream) = socket.split();
    let Some(rid) = register(&mut sink, &mut stream).await else {
        let _ = sink.send(Message::Close(None)).await;
        return;
    };
    // Ready goes out before the slot exists, so no phone request can arrive ahead of it.
    if !send(
        &mut sink,
        &RelayFrame::Ready {
            relay_id: rid.clone(),
        },
    )
    .await
    {
        return;
    }
    let generation = relay.next.fetch_add(1, Ordering::Relaxed);
    let (tx, mut rx) = mpsc::channel(256);
    let (replaced_tx, mut replaced) = oneshot::channel();
    // A newer connection for the same hub replaces and closes the older one; its waiting phones
    // get an error.
    let old = relay.hubs.lock().unwrap().insert(
        rid.clone(),
        Slot {
            generation,
            tx: tx.clone(),
            pending: HashMap::new(),
            replaced: Some(replaced_tx),
        },
    );
    if let Some(close) = old.and_then(|mut s| s.replaced.take()) {
        tracing::warn!(relay_id = %rid, "a second connection for this hub replaced the first");
        let _ = close.send(());
    }
    tracing::info!(relay_id = %rid, "hub connected");
    let mut ping = tokio::time::interval_at(Instant::now() + PING, PING);
    let mut last_seen = Instant::now();
    loop {
        tokio::select! {
            msg = stream.next() => {
                let Some(Ok(msg)) = msg else { break };
                last_seen = Instant::now();
                let Message::Text(text) = msg else {
                    if matches!(msg, Message::Close(_)) { break }
                    continue;
                };
                match serde_json::from_str::<HubFrame>(&text) {
                    Ok(HubFrame::Response { id, envelope }) => complete(&relay, &rid, generation, id, Ok(envelope)),
                    Ok(HubFrame::Reject { id, reason }) => complete(&relay, &rid, generation, id, Err(reason)),
                    Ok(HubFrame::Push { id, push }) => push_for(&relay, &rid, generation, tx.clone(), id, push),
                    _ => tracing::debug!(relay_id = %rid, "ignoring an unexpected frame"),
                }
            }
            frame = rx.recv() => {
                let Some(frame) = frame else { break };
                if !send(&mut sink, &frame).await { break }
            }
            _ = ping.tick() => {
                if last_seen.elapsed() > PING * 3 { break }
                if sink.send(Message::Ping(Vec::new().into())).await.is_err() { break }
            }
            _ = &mut replaced => {
                let _ = sink.send(Message::Close(None)).await;
                break;
            }
        }
    }
    let mut hubs = relay.hubs.lock().unwrap();
    if hubs.get(&rid).is_some_and(|s| s.generation == generation) {
        hubs.remove(&rid);
    }
    tracing::info!(relay_id = %rid, "hub disconnected");
}

fn complete(
    relay: &Relay,
    rid: &str,
    generation: u64,
    id: u64,
    result: Result<ResponseEnvelope, String>,
) {
    let waiter = relay
        .hubs
        .lock()
        .unwrap()
        .get_mut(rid)
        .filter(|s| s.generation == generation)
        .and_then(|s| s.pending.remove(&id));
    if let Some(w) = waiter {
        let _ = w.send(result);
    }
}

/// Sends a hub's push with the relay's own key, within the hub's per-minute budget.
fn push_for(
    relay: &Arc<Relay>,
    rid: &str,
    generation: u64,
    tx: mpsc::Sender<RelayFrame>,
    id: u64,
    p: RelayPush,
) {
    let reply = move |outcome, error: Option<String>| RelayFrame::PushResult { id, outcome, error };
    let current = relay
        .hubs
        .lock()
        .unwrap()
        .get(rid)
        .is_some_and(|s| s.generation == generation);
    let allowed = current && relay.push_allowed(rid, &p.token);
    let kind = match p.kind.as_str() {
        "approval" => Some(OutboxKind::Approval),
        "finished" => Some(OutboxKind::Finished),
        "device_paired" => Some(OutboxKind::DevicePaired),
        _ => None,
    };
    let env = match p.env.as_str() {
        "sandbox" => Some(PushEnv::Sandbox),
        "production" => Some(PushEnv::Production),
        _ => None,
    };
    let hex = p.token.len() <= 200 && p.token.bytes().all(|b| b.is_ascii_hexdigit());
    // The simulator sender needs no token; Apple's does.
    let has_token = !p.token.is_empty();
    let token_ok = move |s: &AnySender| hex && (has_token || !s.needs_token());
    let relay = relay.clone();
    tokio::spawn(async move {
        let frame = match (&relay.push, allowed, kind, env) {
            (None, ..) => reply(
                PushOutcome::Failed,
                Some("this relay doesn't send pushes".into()),
            ),
            (_, false, ..) => reply(PushOutcome::Retry, Some("push rate limit".into())),
            (Some(sender), true, Some(kind), Some(env))
                if token_ok(sender)
                    && valid_id(Some(&p.collapse_id))
                    && valid_id(p.session_id.as_deref())
                    && valid_id(p.approval_id.as_deref()) =>
            {
                let push = Push {
                    device_id: String::new(),
                    kind,
                    session_id: p.session_id.clone(),
                    approval_id: p.approval_id.clone(),
                    token: (!p.token.is_empty()).then(|| p.token.clone()),
                    env,
                    payload: payload_for(kind, p.session_id.as_deref(), p.approval_id.as_deref()),
                    collapse_id: p.collapse_id.clone(),
                    expires_at: p.expires_at,
                };
                match sender.send(&push).await {
                    SendOutcome::Sent => reply(PushOutcome::Sent, None),
                    SendOutcome::Retry(e) => reply(PushOutcome::Retry, Some(e)),
                    SendOutcome::Unregistered(e) => reply(PushOutcome::Unregistered, Some(e)),
                    SendOutcome::Failed(e) => reply(PushOutcome::Failed, Some(e)),
                }
            }
            _ => reply(PushOutcome::Failed, Some("malformed push".into())),
        };
        let _ = tx.send(frame).await;
    });
}
