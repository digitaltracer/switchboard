//! The runner WebSocket endpoint: token auth at the upgrade, hello, then frames both ways.

use std::sync::Arc;
use std::time::Duration;

use axum::Router;
use axum::extract::State;
use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::http::{HeaderMap, StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use futures_util::{SinkExt, StreamExt};
use tokio::time::Instant;

use super::service::SessionService;
use crate::proto::auth::{authenticate, bearer};
use crate::proto::conn::PING_INTERVAL;
use crate::proto::{MAX_FRAME_BYTES, accept_hello, decode, encode};

pub const RUNNER_PATH: &str = "/runner";
const HELLO_TIMEOUT: Duration = Duration::from_secs(10);
const EXPIRE_TICK: Duration = Duration::from_secs(1);

pub fn routes(svc: Arc<SessionService>) -> Router {
    Router::new()
        .route(RUNNER_PATH, get(upgrade))
        .with_state(svc)
}

async fn upgrade(
    State(svc): State<Arc<SessionService>>,
    headers: HeaderMap,
    ws: WebSocketUpgrade,
) -> Response {
    let token = headers
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(bearer);
    let machines = svc
        .config()
        .machines
        .iter()
        .map(|m| (m.id.as_str(), m.token_hash.as_str()));
    let Some(machine) = token.and_then(|t| authenticate(t, machines)) else {
        return (StatusCode::UNAUTHORIZED, "runner token required").into_response();
    };
    let machine = machine.to_owned();
    ws.max_message_size(MAX_FRAME_BYTES)
        .on_upgrade(move |socket| connection(svc, machine, socket))
}

async fn connection(svc: Arc<SessionService>, machine: String, socket: WebSocket) {
    let (mut sink, mut stream) = socket.split();
    let hello = match tokio::time::timeout(HELLO_TIMEOUT, stream.next()).await {
        Ok(Some(Ok(Message::Text(text)))) => {
            accept_hello(&text, &machine).map_err(|e| e.to_string())
        }
        Ok(other) => Err(format!("expected a hello frame, got {other:?}")),
        Err(_) => Err("no hello within the timeout".into()),
    };
    let hello = match hello {
        Ok(hello) => hello,
        Err(e) => {
            tracing::warn!(machine, error = %e, "rejecting runner connection");
            let _ = sink.send(Message::Close(None)).await;
            return;
        }
    };
    let (generation, mut outbound) = svc.runner_connected(&machine, hello).await;
    tracing::info!(machine, generation, "runner connected");

    let mut shutdown = svc.shutdown_signal();
    let stopped = async move {
        let _ = shutdown.wait_for(|stop| *stop).await;
    };
    tokio::pin!(stopped);
    let mut ping = tokio::time::interval_at(Instant::now() + PING_INTERVAL, PING_INTERVAL);
    let mut expire = tokio::time::interval(EXPIRE_TICK);
    let mut last_seen = Instant::now();
    loop {
        tokio::select! {
            msg = stream.next() => {
                let Some(Ok(msg)) = msg else { break };
                last_seen = Instant::now();
                match msg {
                    Message::Text(text) => match decode(&text) {
                        Ok(frame) => svc.runner_frame(&machine, generation, frame).await,
                        Err(e) => tracing::warn!(machine, error = %e, "undecodable runner frame"),
                    },
                    Message::Close(_) => break,
                    _ => {}
                }
            }
            frame = outbound.recv() => {
                // A closed queue means a newer connection replaced this one.
                let Some(frame) = frame else { break };
                if sink.send(Message::Text(encode(&frame).into())).await.is_err() {
                    break;
                }
            }
            _ = ping.tick() => {
                if last_seen.elapsed() > PING_INTERVAL * 2 {
                    tracing::warn!(machine, "runner silent; dropping the connection");
                    break;
                }
                if sink.send(Message::Ping(Vec::new().into())).await.is_err() {
                    break;
                }
            }
            _ = expire.tick() => {
                svc.runners.expire(&machine, Instant::now());
            }
            _ = &mut stopped => break,
        }
    }
    let _ = sink.send(Message::Close(None)).await;
    svc.runner_disconnected(&machine, generation).await;
    tracing::info!(machine, generation, "runner disconnected");
}
