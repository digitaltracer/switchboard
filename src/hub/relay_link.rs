//! The hub's link to a relay. The hub dials out, proves its relay id with its signing key, then
//! answers each phone request the relay forwards: it decrypts it, runs it against its own app
//! API in-process and encrypts the answer. Only the app API is reachable this way, never the
//! runner endpoint. Pushes go out through the same link, using the relay's APNs key.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use axum::Router;
use axum::body::Body;
use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD as B64;
use futures_util::{SinkExt, StreamExt};
use tokio::sync::{mpsc, oneshot, watch};
use tokio::task::JoinHandle;
use tokio::time::Instant;
use tokio_tungstenite::tungstenite::Message;
use tower::ServiceExt;

use super::service::Clock;
use crate::proto::conn::Backoff;
use crate::push::{Push, SendOutcome, Sender};
use crate::relay::proto::{HubFrame, PushOutcome, RelayFrame, RelayPush};
use crate::relay::seal::{
    HubKeys, PlainResponse, ReplayGuard, RequestEnvelope, VERSION, registration_message,
};

const PING: Duration = Duration::from_secs(20);
const HANDSHAKE: Duration = Duration::from_secs(10);
const PUSH_TIMEOUT: Duration = Duration::from_secs(40);
/// A send that takes longer means the connection is dead.
const SEND_TIMEOUT: Duration = Duration::from_secs(30);
/// The backoff resets only once a connection has stayed up this long.
const STABLE: Duration = Duration::from_secs(60);
/// Largest answer a phone gets; session output is capped well below this.
const MAX_RESPONSE: usize = 4 << 20;
/// Headers a relayed request may not set; the hub decides them itself.
const DROPPED: &[&str] = &[
    "host",
    "connection",
    "content-length",
    "transfer-encoding",
    "upgrade",
];

/// Where a push's outcome goes when the relay reports it.
type PushWaiter = oneshot::Sender<(PushOutcome, Option<String>)>;

pub struct RelayLink {
    url: String,
    keys: HubKeys,
    app: Router,
    guard: ReplayGuard,
    clock: Clock,
    out: Mutex<Option<mpsc::Sender<HubFrame>>>,
    pushes: Mutex<HashMap<u64, PushWaiter>>,
    next: AtomicU64,
    ready: watch::Sender<bool>,
}

impl RelayLink {
    pub fn new(url: String, keys: HubKeys, app: Router, clock: Clock) -> Arc<Self> {
        Arc::new(Self {
            url,
            keys,
            app,
            guard: ReplayGuard::default(),
            clock,
            out: Mutex::new(None),
            pushes: Mutex::default(),
            next: AtomicU64::new(1),
            ready: watch::channel(false).0,
        })
    }

    pub fn relay_id(&self) -> String {
        self.keys.relay_id()
    }

    /// True while registered with the relay.
    pub fn ready(&self) -> watch::Receiver<bool> {
        self.ready.subscribe()
    }

    /// Stays connected, reconnecting with backoff, until the task is aborted.
    pub fn spawn(self: &Arc<Self>) -> JoinHandle<()> {
        let link = self.clone();
        tokio::spawn(async move {
            let mut backoff = Backoff::default();
            loop {
                match link.connect_once(&mut backoff).await {
                    Ok(()) => tracing::info!("relay connection closed"),
                    Err(e) => tracing::warn!(error = %e, "relay connection failed"),
                }
                link.disconnected();
                tokio::time::sleep(backoff.next_delay()).await;
            }
        })
    }

    fn disconnected(&self) {
        *self.out.lock().unwrap() = None;
        self.ready.send_replace(false);
        // Pushes waiting on this connection are retried by the dispatcher.
        self.pushes.lock().unwrap().clear();
    }

    async fn connect_once(self: &Arc<Self>, backoff: &mut Backoff) -> anyhow::Result<()> {
        let (ws, _) = tokio::time::timeout(
            HANDSHAKE,
            tokio_tungstenite::connect_async(self.url.as_str()),
        )
        .await
        .map_err(|_| anyhow::anyhow!("connecting to the relay timed out"))??;
        let (mut sink, mut stream) = ws.split();
        let (kx, sign) = (
            B64.encode(self.keys.kx_public()),
            B64.encode(self.keys.sign_public()),
        );
        let send = |f: &HubFrame| Message::text(serde_json::to_string(f).expect("serializes"));
        sink.send(send(&HubFrame::Hello {
            version: VERSION,
            kx_key: kx.clone(),
            sign_key: sign.clone(),
        }))
        .await?;
        let nonce = match next_frame(&mut stream).await? {
            RelayFrame::Challenge { nonce } => nonce,
            other => anyhow::bail!("expected a challenge, got {other:?}"),
        };
        let signature = self.keys.sign(&registration_message(&nonce, &kx, &sign));
        sink.send(send(&HubFrame::Auth { signature })).await?;
        match next_frame(&mut stream).await? {
            RelayFrame::Ready { relay_id } if relay_id == self.relay_id() => {}
            other => anyhow::bail!("the relay didn't accept this hub: {other:?}"),
        }
        tracing::info!(relay = %self.url, relay_id = %self.relay_id(), "connected to the relay");
        let up = Instant::now();

        let (tx, mut rx) = mpsc::channel::<HubFrame>(256);
        *self.out.lock().unwrap() = Some(tx.clone());
        self.ready.send_replace(true);
        let mut ping = tokio::time::interval_at(Instant::now() + PING, PING);
        let mut last_seen = Instant::now();
        loop {
            tokio::select! {
                msg = stream.next() => {
                    let Some(msg) = msg else { return Ok(()) };
                    last_seen = Instant::now();
                    match msg? {
                        Message::Text(text) => match serde_json::from_str::<RelayFrame>(&text) {
                            Ok(RelayFrame::Request { id, envelope }) => {
                                let (link, out) = (self.clone(), tx.clone());
                                tokio::spawn(async move {
                                    let _ = out.send(link.answer(id, envelope).await).await;
                                });
                            }
                            Ok(RelayFrame::PushResult { id, outcome, error }) => {
                                if let Some(w) = self.pushes.lock().unwrap().remove(&id) {
                                    let _ = w.send((outcome, error));
                                }
                            }
                            Ok(other) => tracing::debug!(?other, "unexpected frame from the relay"),
                            Err(e) => tracing::warn!(error = %e, "undecodable frame from the relay"),
                        },
                        Message::Close(_) => return Ok(()),
                        _ => {}
                    }
                }
                Some(frame) = rx.recv() => timed(sink.send(send(&frame))).await?,
                _ = ping.tick() => {
                    if last_seen.elapsed() > PING * 3 {
                        anyhow::bail!("relay silent for {:?}", last_seen.elapsed());
                    }
                    if up.elapsed() >= STABLE {
                        backoff.reset();
                    }
                    timed(sink.send(Message::Ping(Vec::new().into()))).await?;
                }
            }
        }
    }

    /// One phone request: decrypt, run against the app API, encrypt the answer.
    pub async fn answer(&self, id: u64, envelope: RequestEnvelope) -> HubFrame {
        let (req, reply) = match self.keys.open_request(&envelope) {
            Ok(opened) => opened,
            Err(e) => {
                return HubFrame::Reject {
                    id,
                    reason: e.to_string(),
                };
            }
        };
        // A refusal goes back in the clear: sealing it would be a second answer under this key.
        if let Err(e) = self.guard.check(&reply.epk, req.ts, self.clock.now_ms()) {
            return HubFrame::Reject {
                id,
                reason: e.to_string(),
            };
        }
        let plain = self.run(&req).await;
        HubFrame::Response {
            id,
            envelope: reply.seal(&plain),
        }
    }

    async fn run(&self, req: &crate::relay::seal::PlainRequest) -> PlainResponse {
        if !req.path.starts_with("/app/v1/") {
            return api_error(
                404,
                "not_found",
                "only the app API is reachable through the relay",
            );
        }
        let Ok(body) = req.body_bytes() else {
            return api_error(400, "bad_request", "the request body isn't base64");
        };
        let mut builder = axum::http::Request::builder()
            .method(req.method.as_str())
            .uri(&req.path);
        for (name, value) in &req.headers {
            if !DROPPED.contains(&name.to_ascii_lowercase().as_str()) {
                builder = builder.header(name, value);
            }
        }
        let Ok(http) = builder.body(Body::from(body)) else {
            return api_error(400, "bad_request", "malformed request");
        };
        let Ok(resp) = self.app.clone().oneshot(http).await;
        let status = resp.status().as_u16();
        let headers = resp
            .headers()
            .iter()
            .filter_map(|(k, v)| Some((k.to_string(), v.to_str().ok()?.to_owned())))
            .collect();
        match axum::body::to_bytes(resp.into_body(), MAX_RESPONSE).await {
            Ok(bytes) => PlainResponse::new(status, headers, &bytes),
            Err(_) => api_error(500, "internal", "the answer is too large to relay"),
        }
    }

    async fn push(&self, push: &Push) -> SendOutcome {
        let Some(out) = self.out.lock().unwrap().clone() else {
            return SendOutcome::Retry("not connected to the relay".into());
        };
        let id = self.next.fetch_add(1, Ordering::Relaxed);
        let (tx, rx) = oneshot::channel();
        self.pushes.lock().unwrap().insert(id, tx);
        let frame = HubFrame::Push {
            id,
            push: RelayPush {
                token: push.token.clone().unwrap_or_default(),
                env: push.env.as_str().into(),
                kind: push.kind.as_str().into(),
                session_id: push.session_id.clone(),
                approval_id: push.approval_id.clone(),
                collapse_id: push.collapse_id.clone(),
                expires_at: push.expires_at,
            },
        };
        if out.send(frame).await.is_err() {
            self.pushes.lock().unwrap().remove(&id);
            return SendOutcome::Retry("the relay connection closed".into());
        }
        match tokio::time::timeout(PUSH_TIMEOUT, rx).await {
            Ok(Ok((outcome, error))) => {
                let e = error.unwrap_or_default();
                match outcome {
                    PushOutcome::Sent => SendOutcome::Sent,
                    PushOutcome::Retry => SendOutcome::Retry(e),
                    PushOutcome::Unregistered => SendOutcome::Unregistered(e),
                    PushOutcome::Failed => SendOutcome::Failed(e),
                }
            }
            _ => {
                self.pushes.lock().unwrap().remove(&id);
                SendOutcome::Retry("no answer from the relay".into())
            }
        }
    }
}

/// Sends the hub's pushes through the relay, which holds the app's APNs key.
pub struct RelaySender(pub Arc<RelayLink>);

impl Sender for RelaySender {
    fn needs_token(&self) -> bool {
        false
    }

    async fn send(&self, push: &Push) -> SendOutcome {
        self.0.push(push).await
    }
}

async fn timed<E: Into<anyhow::Error>>(
    send: impl std::future::Future<Output = Result<(), E>>,
) -> anyhow::Result<()> {
    tokio::time::timeout(SEND_TIMEOUT, send)
        .await
        .map_err(|_| anyhow::anyhow!("sending to the relay timed out"))?
        .map_err(Into::into)
}

async fn next_frame<S>(stream: &mut S) -> anyhow::Result<RelayFrame>
where
    S: futures_util::Stream<Item = Result<Message, tokio_tungstenite::tungstenite::Error>> + Unpin,
{
    let read = async {
        loop {
            match stream.next().await {
                Some(Ok(Message::Text(t))) => return Ok(serde_json::from_str(&t)?),
                Some(Ok(Message::Close(_))) | None => {
                    anyhow::bail!("the relay closed the connection")
                }
                Some(Err(e)) => return Err(e.into()),
                Some(Ok(_)) => {}
            }
        }
    };
    tokio::time::timeout(HANDSHAKE, read)
        .await
        .map_err(|_| anyhow::anyhow!("the relay didn't answer the handshake"))?
}

fn api_error(status: u16, code: &str, message: &str) -> PlainResponse {
    let body = serde_json::json!({ "error": { "code": code, "message": message } }).to_string();
    PlainResponse::new(
        status,
        vec![("content-type".into(), "application/json".into())],
        body.as_bytes(),
    )
}
