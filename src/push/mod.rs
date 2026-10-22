//! Push notifications from the durable outbox. Each delivery row is retried with backoff until
//! it is sent, fails for good or is dropped; nothing is lost to a hub restart or a slow client.
//! Notification text is generic and the payload carries ids only, so a lock screen shows
//! nothing from an agent; the app fetches the real state from the hub.

pub mod apns;
pub mod simctl;

use std::future::Future;
use std::sync::Arc;
use std::time::Duration;

use serde_json::{Value, json};
use tokio::task::JoinHandle;

use crate::config::{PushConfig, PushSenderKind};
use crate::hub::service::Clock;
use crate::redact::redact;
use crate::store::{
    DeliveryUpdate, DeviceState, DueDelivery, OutboxKind, PushEnv, Store, StoreError,
};

pub const TICK: Duration = Duration::from_secs(1);
/// A notification older than this is dropped instead of sent: an approval is long denied by then.
pub const STALE_AFTER: Duration = Duration::from_secs(30 * 60);
pub const MAX_ATTEMPTS: i64 = 8;
const BATCH: u32 = 50;

/// One notification for one device.
#[derive(Debug, Clone, PartialEq)]
pub struct Push {
    pub device_id: String,
    pub kind: OutboxKind,
    pub session_id: Option<String>,
    pub approval_id: Option<String>,
    pub token: Option<String>,
    pub env: PushEnv,
    /// The APNs payload: `aps` plus ids.
    pub payload: Value,
    /// Replaces an earlier notification with the same id on the phone.
    pub collapse_id: String,
    /// Unix seconds after which APNs stops trying.
    pub expires_at: i64,
}

#[derive(Debug, Clone, PartialEq)]
pub enum SendOutcome {
    Sent,
    /// Worth trying again later: a timeout, 429 or 5xx.
    Retry(String),
    /// The device token is no longer valid; it is forgotten.
    Unregistered(String),
    Failed(String),
}

pub trait Sender: Send + Sync + 'static {
    /// Whether a device without a push token can be sent to (only the simulator can).
    fn needs_token(&self) -> bool;
    fn send(&self, push: &Push) -> impl Future<Output = SendOutcome> + Send;
}

/// The generic text for each kind of notification.
pub fn text(kind: OutboxKind) -> (&'static str, &'static str) {
    match kind {
        OutboxKind::Approval => (
            "Approval needed",
            "An agent is waiting for you to approve or deny.",
        ),
        OutboxKind::Finished => ("Session finished", "An agent finished its turn."),
        OutboxKind::DevicePaired => (
            "New device paired",
            "A new device was paired with your hub. Revoke it if that wasn't you.",
        ),
    }
}

/// The whole notification: generic text for its kind plus ids. A relay builds it the same way,
/// so a hub can't put its own text on someone's lock screen.
pub fn payload_for(kind: OutboxKind, session_id: Option<&str>, approval_id: Option<&str>) -> Value {
    let (title, body) = text(kind);
    let thread = session_id.unwrap_or("switchboard");
    // Ids only; masked anyway, like everything that leaves the machine.
    redact(&json!({
        "aps": { "alert": { "title": title, "body": body }, "sound": "default", "thread-id": thread },
        "kind": kind.as_str(),
        "session_id": session_id,
        "approval_id": approval_id,
    }))
}

pub fn payload(d: &DueDelivery) -> Value {
    payload_for(d.kind, d.session_id.as_deref(), d.approval_id.as_deref())
}

fn backoff(attempts: i64) -> Duration {
    Duration::from_secs((2u64 << attempts.clamp(0, 8)).min(300))
}

pub struct Dispatcher<S> {
    store: Store,
    clock: Clock,
    sender: Arc<S>,
}

impl<S: Sender> Dispatcher<S> {
    pub fn new(store: Store, clock: Clock, sender: S) -> Self {
        Self {
            store,
            clock,
            sender: Arc::new(sender),
        }
    }

    /// Works through the due deliveries once; returns how many were attempted or settled.
    pub async fn tick(&self) -> Result<usize, StoreError> {
        let now = self.clock.now_ms();
        let due = self.store.due_deliveries(now, BATCH).await?;
        for d in &due {
            let update = self.attempt(d, now).await?;
            self.store
                .update_delivery(d.outbox_id, &d.device_id, update, self.clock.now_ms())
                .await?;
        }
        Ok(due.len())
    }

    async fn attempt(&self, d: &DueDelivery, now: i64) -> Result<DeliveryUpdate, StoreError> {
        let drop = |why: &str| Ok(DeliveryUpdate::Dropped(why.to_owned()));
        if d.device_state != DeviceState::Active {
            return drop("device not active");
        }
        if now - d.created_at > STALE_AFTER.as_millis() as i64 {
            return drop("stale");
        }
        // Every notification re-checks the hub: a decided approval needs no push.
        if d.kind == OutboxKind::Approval && !d.approval_open {
            return drop("already decided");
        }
        if self.sender.needs_token() && d.push_token.is_none() {
            return drop("no push token");
        }
        let push = Push {
            device_id: d.device_id.clone(),
            kind: d.kind,
            session_id: d.session_id.clone(),
            approval_id: d.approval_id.clone(),
            token: d.push_token.clone(),
            env: d.push_env.unwrap_or(PushEnv::Production),
            payload: payload(d),
            collapse_id: d
                .approval_id
                .clone()
                .unwrap_or_else(|| d.dedupe_key.clone()),
            expires_at: (d.created_at + STALE_AFTER.as_millis() as i64) / 1000,
        };
        Ok(match self.sender.send(&push).await {
            SendOutcome::Sent => DeliveryUpdate::Sent,
            SendOutcome::Retry(e) if d.attempts + 1 < MAX_ATTEMPTS => DeliveryUpdate::Retry {
                next_attempt_at: now + backoff(d.attempts).as_millis() as i64,
                error: e,
            },
            SendOutcome::Retry(e) | SendOutcome::Failed(e) => DeliveryUpdate::Failed(e),
            SendOutcome::Unregistered(e) => {
                if let Some(t) = &d.push_token {
                    self.store.clear_push_token(&d.device_id, t).await?;
                }
                DeliveryUpdate::Failed(e)
            }
        })
    }

    pub fn spawn(self) -> JoinHandle<()> {
        tokio::spawn(async move {
            let mut tick = tokio::time::interval(TICK);
            loop {
                tick.tick().await;
                if let Err(e) = self.tick().await {
                    tracing::warn!(error = %e, "push dispatch failed");
                }
            }
        })
    }
}

/// Logs what would be sent; for a hub with `sender = "log"`.
pub struct LogSender;

impl Sender for LogSender {
    fn needs_token(&self) -> bool {
        false
    }

    async fn send(&self, push: &Push) -> SendOutcome {
        tracing::info!(device = %push.device_id, payload = %push.payload, "push (log only)");
        SendOutcome::Sent
    }
}

/// The sender `[push]` names; the hub and the relay both build theirs here.
pub enum AnySender {
    Apns(Box<apns::Apns>),
    Simctl(simctl::Simctl),
    Log(LogSender),
}

impl AnySender {
    pub fn from_config(cfg: &PushConfig) -> anyhow::Result<Self> {
        Ok(match cfg.sender {
            PushSenderKind::Apns => {
                let a = cfg
                    .apns
                    .as_ref()
                    .expect("validated: apns needs [push.apns]");
                AnySender::Apns(Box::new(apns::Apns::new(a, &cfg.bundle_id)?))
            }
            PushSenderKind::Simctl => {
                AnySender::Simctl(simctl::Simctl::new(&cfg.simulator, &cfg.bundle_id))
            }
            PushSenderKind::Log => AnySender::Log(LogSender),
        })
    }
}

impl Sender for AnySender {
    fn needs_token(&self) -> bool {
        match self {
            AnySender::Apns(s) => s.needs_token(),
            AnySender::Simctl(s) => s.needs_token(),
            AnySender::Log(s) => s.needs_token(),
        }
    }

    async fn send(&self, push: &Push) -> SendOutcome {
        match self {
            AnySender::Apns(s) => s.send(push).await,
            AnySender::Simctl(s) => s.send(push).await,
            AnySender::Log(s) => s.send(push).await,
        }
    }
}

/// Starts the configured sender over the hub's outbox.
pub fn start(cfg: &PushConfig, store: Store, clock: Clock) -> anyhow::Result<JoinHandle<()>> {
    Ok(Dispatcher::new(store, clock, AnySender::from_config(cfg)?).spawn())
}

#[cfg(test)]
mod tests;
