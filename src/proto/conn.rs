//! Connection rules shared by hub and runner: correlation ids, generations, deadlines,
//! bounded in-flight requests and reconnect backoff.

use std::collections::HashMap;
use std::hash::{BuildHasher, RandomState};
use std::time::Duration;

use serde_json::Value;
use tokio::sync::oneshot;
use tokio::time::Instant;

use super::{ErrorCode, Op, Outcome, ProtoError, Req, Resp};

pub const PING_INTERVAL: Duration = Duration::from_secs(20);
pub const MAX_IN_FLIGHT: usize = 64;
/// Capacity of each outbound frame queue.
pub const QUEUE_CAPACITY: usize = 256;

pub type Reply = Result<Value, ProtoError>;

struct InFlight {
    deadline: Instant,
    mutating: bool,
    tx: oneshot::Sender<Reply>,
}

/// The hub's side of one runner: requests waiting for their `Resp`.
/// A new connection bumps the generation, so replies sent on an older one are dropped.
pub struct Correlator {
    generation: u64,
    next_id: u64,
    in_flight: HashMap<u64, InFlight>,
}

impl Default for Correlator {
    fn default() -> Self {
        Self::new()
    }
}

impl Correlator {
    pub fn new() -> Self {
        Self {
            generation: 0,
            next_id: 1,
            in_flight: HashMap::new(),
        }
    }

    pub fn generation(&self) -> u64 {
        self.generation
    }

    pub fn in_flight(&self) -> usize {
        self.in_flight.len()
    }

    /// Starts a connection. Everything still in flight fails, mutations as `outcome_unknown`;
    /// none of it is replayed.
    pub fn connected(&mut self) -> u64 {
        self.fail_all();
        self.generation += 1;
        self.generation
    }

    pub fn disconnected(&mut self) {
        self.fail_all();
    }

    fn fail_all(&mut self) {
        for (_, f) in self.in_flight.drain() {
            let err = if f.mutating {
                ProtoError::new(
                    ErrorCode::OutcomeUnknown,
                    "runner connection lost mid-request",
                )
            } else {
                ProtoError::new(ErrorCode::RunnerDisconnected, "runner connection lost")
            };
            let _ = f.tx.send(Err(err));
        }
    }

    /// Registers a request on the current generation.
    pub fn register(
        &mut self,
        op: Op,
        timeout: Duration,
        now: Instant,
    ) -> Result<(u64, Req, oneshot::Receiver<Reply>), ProtoError> {
        if self.in_flight.len() >= MAX_IN_FLIGHT {
            return Err(ProtoError::new(
                ErrorCode::Busy,
                "too many requests in flight",
            ));
        }
        let id = self.next_id;
        self.next_id += 1;
        let (tx, rx) = oneshot::channel();
        self.in_flight.insert(
            id,
            InFlight {
                deadline: now + timeout,
                mutating: op.is_mutating(),
                tx,
            },
        );
        let req = Req {
            id,
            deadline_ms: timeout.as_millis() as u64,
            op,
        };
        Ok((self.generation, req, rx))
    }

    /// Delivers a reply. Returns false for a stale generation or an unknown id.
    pub fn complete(&mut self, generation: u64, resp: Resp) -> bool {
        if generation != self.generation {
            return false;
        }
        let Some(f) = self.in_flight.remove(&resp.id) else {
            return false;
        };
        let reply = match resp.outcome {
            Outcome::Ok(v) => Ok(v),
            Outcome::Err(e) => Err(e),
        };
        let _ = f.tx.send(reply);
        true
    }

    /// Fails requests whose deadline has passed; mutations as `outcome_unknown`.
    pub fn expire(&mut self, now: Instant) -> usize {
        let expired: Vec<u64> = self
            .in_flight
            .iter()
            .filter(|(_, f)| f.deadline <= now)
            .map(|(id, _)| *id)
            .collect();
        for id in &expired {
            let f = self.in_flight.remove(id).expect("listed above");
            let err = if f.mutating {
                ProtoError::new(
                    ErrorCode::OutcomeUnknown,
                    "runner did not answer before the deadline",
                )
            } else {
                ProtoError::new(
                    ErrorCode::DeadlineExceeded,
                    "runner did not answer before the deadline",
                )
            };
            let _ = f.tx.send(Err(err));
        }
        expired.len()
    }

    pub fn next_deadline(&self) -> Option<Instant> {
        self.in_flight.values().map(|f| f.deadline).min()
    }
}

/// Exponential backoff with full jitter for runner reconnects.
#[derive(Debug, Clone)]
pub struct Backoff {
    base: Duration,
    cap: Duration,
    attempt: u32,
}

impl Default for Backoff {
    fn default() -> Self {
        Self::new(Duration::from_millis(500), Duration::from_secs(30))
    }
}

impl Backoff {
    pub fn new(base: Duration, cap: Duration) -> Self {
        Self {
            base,
            cap,
            attempt: 0,
        }
    }

    pub fn reset(&mut self) {
        self.attempt = 0;
    }

    /// Ceiling for the current attempt, before jitter.
    pub fn ceiling(&self) -> Duration {
        let factor = 1u32.checked_shl(self.attempt.min(20)).unwrap_or(u32::MAX);
        self.base.saturating_mul(factor).min(self.cap)
    }

    /// Next delay, uniform in `[0, ceiling]`; `unit` in `[0, 1)` is the jitter source.
    pub fn next_with(&mut self, unit: f64) -> Duration {
        let d = self.ceiling().mul_f64(unit.clamp(0.0, 1.0));
        self.attempt = self.attempt.saturating_add(1);
        d
    }

    pub fn next_delay(&mut self) -> Duration {
        let unit = (RandomState::new().hash_one(Instant::now()) >> 11) as f64 / (1u64 << 53) as f64;
        self.next_with(unit)
    }
}
