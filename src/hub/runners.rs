//! Connected runners, one slot per machine: the correlator, the outbound queue and readiness.
//! A machine is ready only after the hub has reconciled the hello of its current connection.

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::Duration;

use serde::Serialize;
use serde_json::Value;
use tokio::sync::mpsc;
use tokio::time::Instant;

use crate::proto::conn::{Correlator, QUEUE_CAPACITY};
use crate::proto::{ErrorCode, Frame, Op, ProtoError, Resp};
use crate::redact::{redact, redact_str};

/// Extra wait past a request's deadline before the caller gives up on its own.
const REPLY_GRACE: Duration = Duration::from_secs(5);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Readiness {
    Offline,
    /// Connected, hello not yet reconciled; starts wait.
    Reconciling,
    Ready,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct RunnerInfo {
    pub runner_version: String,
    pub herdr_version: Option<String>,
    pub connected_at: i64,
}

#[derive(Default)]
struct Slot {
    correlator: Correlator,
    tx: Option<mpsc::Sender<Frame>>,
    ready: bool,
    info: Option<RunnerInfo>,
}

#[derive(Default)]
pub struct Runners {
    slots: Mutex<HashMap<String, Slot>>,
}

impl Runners {
    /// A new connection for `machine`; in-flight requests on the older one fail and its queue closes.
    pub fn attach(&self, machine: &str, info: RunnerInfo) -> (u64, mpsc::Receiver<Frame>) {
        let (tx, rx) = mpsc::channel(QUEUE_CAPACITY);
        let mut slots = self.slots.lock().unwrap();
        let slot = slots.entry(machine.to_owned()).or_default();
        let generation = slot.correlator.connected();
        slot.tx = Some(tx);
        slot.ready = false;
        slot.info = Some(info);
        (generation, rx)
    }

    pub fn set_ready(&self, machine: &str, generation: u64) {
        let mut slots = self.slots.lock().unwrap();
        if let Some(slot) = slots.get_mut(machine)
            && slot.correlator.generation() == generation
        {
            slot.ready = true;
        }
    }

    /// Returns false when a newer connection already replaced this one.
    pub fn detach(&self, machine: &str, generation: u64) -> bool {
        let mut slots = self.slots.lock().unwrap();
        let Some(slot) = slots.get_mut(machine) else {
            return false;
        };
        if slot.correlator.generation() != generation || slot.tx.is_none() {
            return false;
        }
        slot.correlator.disconnected();
        slot.tx = None;
        slot.ready = false;
        slot.info = None;
        true
    }

    pub fn is_current(&self, machine: &str, generation: u64) -> bool {
        let slots = self.slots.lock().unwrap();
        slots
            .get(machine)
            .is_some_and(|s| s.tx.is_some() && s.correlator.generation() == generation)
    }

    pub fn complete(&self, machine: &str, generation: u64, resp: Resp) -> bool {
        let mut slots = self.slots.lock().unwrap();
        slots
            .get_mut(machine)
            .is_some_and(|s| s.correlator.complete(generation, resp))
    }

    pub fn expire(&self, machine: &str, now: Instant) -> usize {
        let mut slots = self.slots.lock().unwrap();
        slots
            .get_mut(machine)
            .map_or(0, |s| s.correlator.expire(now))
    }

    pub fn readiness(&self, machine: &str) -> Readiness {
        let slots = self.slots.lock().unwrap();
        match slots.get(machine) {
            Some(s) if s.tx.is_some() && s.ready => Readiness::Ready,
            Some(s) if s.tx.is_some() => Readiness::Reconciling,
            _ => Readiness::Offline,
        }
    }

    pub fn info(&self, machine: &str) -> Option<RunnerInfo> {
        self.slots
            .lock()
            .unwrap()
            .get(machine)
            .and_then(|s| s.info.clone())
    }

    /// Sends `op` on the current connection; `runner_disconnected` means it was never sent.
    pub async fn call(
        &self,
        machine: &str,
        op: Op,
        timeout: Duration,
    ) -> Result<Value, ProtoError> {
        let mutating = op.is_mutating();
        let (tx, req, rx) = {
            let mut slots = self.slots.lock().unwrap();
            let slot = slots
                .get_mut(machine)
                .filter(|s| s.tx.is_some())
                .ok_or_else(|| {
                    ProtoError::new(
                        ErrorCode::RunnerDisconnected,
                        format!("machine {machine} is offline"),
                    )
                })?;
            let (_, req, rx) = slot.correlator.register(op, timeout, Instant::now())?;
            (slot.tx.clone().expect("checked above"), req, rx)
        };
        // A closed queue means the connection is going away; detaching fails the request.
        let _ = tx.send(Frame::Req(req)).await;
        match tokio::time::timeout(timeout + REPLY_GRACE, rx).await {
            // Runner output reaches callers and the database only with secrets masked.
            Ok(Ok(reply)) => reply.map(|v| redact(&v)).map_err(|mut e| {
                e.message = redact_str(&e.message);
                e
            }),
            _ if mutating => Err(ProtoError::new(
                ErrorCode::OutcomeUnknown,
                "no reply from the runner",
            )),
            _ => Err(ProtoError::new(
                ErrorCode::DeadlineExceeded,
                "no reply from the runner",
            )),
        }
    }
}
