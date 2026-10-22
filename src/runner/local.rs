//! The runner built into the hub, for the machine the hub runs on. The same frames as the
//! WebSocket link, masked the same way, passed in memory: no token, no socket, no second process.

use std::sync::Arc;
use std::time::Duration;

use tokio::time::Instant;

use super::link::{Handler, mask};
use crate::hub::service::SessionService;
use crate::proto::{Event, Frame, Hello, Outcome, PROTOCOL_VERSION, Resp};

const EXPIRE_TICK: Duration = Duration::from_secs(1);

/// Connects `handler` to the hub as `machine` until the hub shuts down.
pub async fn run<H: Handler>(
    svc: Arc<SessionService>,
    machine: String,
    herdr_version: Option<String>,
    handler: Arc<H>,
    mut events: tokio::sync::mpsc::Receiver<Event>,
) {
    let hello = Hello {
        machine_id: machine.clone(),
        protocol_version: PROTOCOL_VERSION,
        runner_version: env!("CARGO_PKG_VERSION").into(),
        herdr_version,
        sessions: handler.owned().await,
    };
    let (generation, mut outbound) = svc.runner_connected(&machine, hello).await;
    tracing::info!(machine, "built-in runner connected");
    let mut shutdown = svc.shutdown_signal();
    let stopped = async move {
        let _ = shutdown.wait_for(|stop| *stop).await;
    };
    tokio::pin!(stopped);
    let mut expire = tokio::time::interval(EXPIRE_TICK);
    loop {
        tokio::select! {
            frame = outbound.recv() => match frame {
                Some(Frame::Req(req)) => {
                    let (svc, machine, handler) = (svc.clone(), machine.clone(), handler.clone());
                    tokio::spawn(async move {
                        let outcome = match handler.handle(req.op).await {
                            Ok(v) => Outcome::Ok(v),
                            Err(e) => Outcome::Err(e),
                        };
                        deliver(&svc, &machine, generation, Frame::Resp(Resp { id: req.id, outcome })).await;
                    });
                }
                Some(other) => tracing::warn!(?other, "unexpected frame for the built-in runner"),
                None => break,
            },
            Some(event) = events.recv() => deliver(&svc, &machine, generation, Frame::Event(event)).await,
            _ = expire.tick() => {
                svc.runners.expire(&machine, Instant::now());
            }
            _ = &mut stopped => break,
        }
    }
    svc.runner_disconnected(&machine, generation).await;
}

async fn deliver(svc: &Arc<SessionService>, machine: &str, generation: u64, frame: Frame) {
    match mask(&frame) {
        Ok(frame) => svc.runner_frame(machine, generation, frame).await,
        Err(e) => tracing::warn!(machine, error = %e, "a masked runner frame no longer decodes"),
    }
}
