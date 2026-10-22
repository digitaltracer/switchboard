//! Outbound WebSocket to the hub. Requests in, responses and events out; reconnects with jitter.
//! Responses for a dropped connection are discarded: the hub has already reported them as
//! `outcome_unknown`, and nothing is replayed on the next connection. Every frame sent leaves
//! the machine, so secrets in it are masked first.

use std::future::Future;
use std::sync::Arc;

use anyhow::{Context, bail};
use futures_util::{SinkExt, StreamExt};
use serde_json::Value;
use tokio::sync::mpsc;
use tokio::time::Instant;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use url::Url;

use super::sessions::Sessions;
use crate::proto::conn::{Backoff, PING_INTERVAL, QUEUE_CAPACITY};
use crate::proto::{
    Event, Frame, Hello, Op, Outcome, OwnedSession, PROTOCOL_VERSION, ProtoError, Resp, decode,
};
use crate::redact::redact;

pub struct LinkConfig {
    pub url: Url,
    pub token: String,
    pub machine_id: String,
    pub herdr_version: Option<String>,
}

pub trait Handler: Send + Sync + 'static {
    fn handle(&self, op: Op) -> impl Future<Output = Result<Value, ProtoError>> + Send;
    fn owned(&self) -> impl Future<Output = Vec<OwnedSession>> + Send;
}

impl Handler for Sessions {
    fn handle(&self, op: Op) -> impl Future<Output = Result<Value, ProtoError>> + Send {
        Sessions::handle(self, op)
    }

    fn owned(&self) -> impl Future<Output = Vec<OwnedSession>> + Send {
        Sessions::owned(self)
    }
}

pub async fn run<H: Handler>(cfg: LinkConfig, handler: Arc<H>, mut events: mpsc::Receiver<Event>) {
    let mut backoff = Backoff::default();
    loop {
        match connect_once(&cfg, &handler, &mut events, &mut backoff).await {
            Ok(()) => tracing::info!("hub closed the connection"),
            Err(e) => tracing::warn!(error = %format!("{e:#}"), "hub connection failed"),
        }
        tokio::time::sleep(backoff.next_delay()).await;
    }
}

async fn connect_once<H: Handler>(
    cfg: &LinkConfig,
    handler: &Arc<H>,
    events: &mut mpsc::Receiver<Event>,
    backoff: &mut Backoff,
) -> anyhow::Result<()> {
    let mut request = cfg.url.as_str().into_client_request()?;
    request.headers_mut().insert(
        "authorization",
        format!("Bearer {}", cfg.token)
            .parse()
            .context("token is not a valid header value")?,
    );
    let (ws, _) = tokio_tungstenite::connect_async(request).await?;
    backoff.reset();
    let (mut sink, mut stream) = ws.split();
    let hello = Frame::Hello(Hello {
        machine_id: cfg.machine_id.clone(),
        protocol_version: PROTOCOL_VERSION,
        runner_version: env!("CARGO_PKG_VERSION").into(),
        herdr_version: cfg.herdr_version.clone(),
        sessions: handler.owned().await,
    });
    sink.send(Message::text(masked(&hello))).await?;
    tracing::info!(url = %cfg.url, "connected to hub");

    let (out_tx, mut out_rx) = mpsc::channel::<Frame>(QUEUE_CAPACITY);
    let mut ping = tokio::time::interval_at(Instant::now() + PING_INTERVAL, PING_INTERVAL);
    let mut last_seen = Instant::now();
    loop {
        tokio::select! {
            msg = stream.next() => {
                let Some(msg) = msg else { return Ok(()) };
                last_seen = Instant::now();
                match msg? {
                    Message::Text(text) => match decode(&text) {
                        Ok(Frame::Req(req)) => {
                            let (handler, out) = (handler.clone(), out_tx.clone());
                            tokio::spawn(async move {
                                let outcome = match handler.handle(req.op).await {
                                    Ok(v) => Outcome::Ok(v),
                                    Err(e) => Outcome::Err(e),
                                };
                                let _ = out.send(Frame::Resp(Resp { id: req.id, outcome })).await;
                            });
                        }
                        Ok(other) => tracing::warn!(?other, "unexpected frame from hub"),
                        Err(e) => tracing::warn!(error = %e, "undecodable frame from hub"),
                    },
                    Message::Close(_) => return Ok(()),
                    _ => {}
                }
            }
            Some(frame) = out_rx.recv() => sink.send(Message::text(masked(&frame))).await?,
            Some(event) = events.recv() => sink.send(Message::text(masked(&Frame::Event(event)))).await?,
            _ = ping.tick() => {
                if last_seen.elapsed() > PING_INTERVAL * 2 {
                    bail!("hub silent for {:?}", last_seen.elapsed());
                }
                sink.send(Message::Ping(Vec::new().into())).await?;
            }
        }
    }
}

/// The frame with secrets masked, as the in-process link hands it to the hub.
pub(crate) fn mask(frame: &Frame) -> Result<Frame, serde_json::Error> {
    serde_json::from_value(redact(
        &serde_json::to_value(frame).expect("frames serialize"),
    ))
}

/// The frame as JSON text with secrets masked.
fn masked(frame: &Frame) -> String {
    redact(&serde_json::to_value(frame).expect("frames serialize")).to_string()
}

#[cfg(test)]
#[path = "link_tests.rs"]
mod tests;
