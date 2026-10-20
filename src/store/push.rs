//! Push deliveries: one row per notification and device, retried until sent, failed or dropped.

use tokio_rusqlite::rusqlite::{self, params};

use super::{DeviceState, OutboxKind, PushEnv, Result, Store};

/// A delivery that is due, with what sending it needs.
#[derive(Debug, Clone, PartialEq)]
pub struct DueDelivery {
    pub outbox_id: i64,
    pub device_id: String,
    pub kind: OutboxKind,
    pub dedupe_key: String,
    pub session_id: Option<String>,
    pub approval_id: Option<String>,
    pub created_at: i64,
    pub attempts: i64,
    pub device_state: DeviceState,
    pub push_token: Option<String>,
    pub push_env: Option<PushEnv>,
    /// For an approval: whether it is still undecided.
    pub approval_open: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub enum DeliveryUpdate {
    Sent,
    Retry { next_attempt_at: i64, error: String },
    Failed(String),
    Dropped(String),
}

impl Store {
    /// Pending deliveries whose next attempt is due, oldest first.
    pub async fn due_deliveries(&self, now: i64, limit: u32) -> Result<Vec<DueDelivery>> {
        self.call(move |c| {
            let mut stmt = c.prepare(
                "SELECT d.outbox_id, d.device_id, o.kind, o.dedupe_key, o.session_id, o.approval_id,
                 o.created_at, d.attempts, v.state, v.push_token, v.push_env,
                 (o.approval_id IS NOT NULL AND EXISTS
                    (SELECT 1 FROM approvals a WHERE a.id = o.approval_id AND a.outcome IS NULL))
                 FROM outbox_deliveries d
                 JOIN outbox o ON o.id = d.outbox_id
                 JOIN devices v ON v.id = d.device_id
                 WHERE d.state = 'pending' AND d.next_attempt_at <= ?1
                 ORDER BY d.next_attempt_at, d.outbox_id LIMIT ?2",
            )?;
            let rows = stmt.query_map(params![now, limit], |r| {
                Ok((
                    r.get::<_, i64>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, String>(2)?,
                    r.get::<_, String>(3)?,
                    r.get::<_, Option<String>>(4)?,
                    r.get::<_, Option<String>>(5)?,
                    r.get::<_, i64>(6)?,
                    r.get::<_, i64>(7)?,
                    r.get::<_, String>(8)?,
                    r.get::<_, Option<String>>(9)?,
                    r.get::<_, Option<String>>(10)?,
                    r.get::<_, bool>(11)?,
                ))
            })?;
            let raws = rows.collect::<rusqlite::Result<Vec<_>>>()?;
            raws.into_iter()
                .map(
                    |(
                        outbox_id,
                        device_id,
                        kind,
                        dedupe_key,
                        session_id,
                        approval_id,
                        created_at,
                        attempts,
                        state,
                        push_token,
                        push_env,
                        approval_open,
                    )| {
                        Ok(DueDelivery {
                            outbox_id,
                            device_id,
                            kind: OutboxKind::parse(kind)?,
                            dedupe_key,
                            session_id,
                            approval_id,
                            created_at,
                            attempts,
                            device_state: DeviceState::parse(state)?,
                            push_token,
                            push_env: push_env.map(PushEnv::parse).transpose()?,
                            approval_open,
                        })
                    },
                )
                .collect()
        })
        .await
    }

    /// Settles one attempt; a delivery that is no longer pending keeps its state.
    pub async fn update_delivery(
        &self,
        outbox_id: i64,
        device_id: &str,
        update: DeliveryUpdate,
        now: i64,
    ) -> Result<bool> {
        let device_id = device_id.to_owned();
        self.call(move |c| {
            let n = match update {
                DeliveryUpdate::Sent => c.execute(
                    "UPDATE outbox_deliveries SET state = 'sent', attempts = attempts + 1, sent_at = ?3,
                     last_error = NULL WHERE outbox_id = ?1 AND device_id = ?2 AND state = 'pending'",
                    params![outbox_id, device_id, now],
                )?,
                DeliveryUpdate::Retry { next_attempt_at, error } => c.execute(
                    "UPDATE outbox_deliveries SET attempts = attempts + 1, next_attempt_at = ?3,
                     last_error = ?4 WHERE outbox_id = ?1 AND device_id = ?2 AND state = 'pending'",
                    params![outbox_id, device_id, next_attempt_at, error],
                )?,
                DeliveryUpdate::Failed(error) => c.execute(
                    "UPDATE outbox_deliveries SET state = 'failed', attempts = attempts + 1,
                     last_error = ?3 WHERE outbox_id = ?1 AND device_id = ?2 AND state = 'pending'",
                    params![outbox_id, device_id, error],
                )?,
                DeliveryUpdate::Dropped(why) => c.execute(
                    "UPDATE outbox_deliveries SET state = 'dropped', last_error = ?3
                     WHERE outbox_id = ?1 AND device_id = ?2 AND state = 'pending'",
                    params![outbox_id, device_id, why],
                )?,
            };
            Ok(n == 1)
        })
        .await
    }

    /// Forgets a token APNs says is no longer valid; the phone sends a new one when it registers.
    pub async fn clear_push_token(&self, device_id: &str, token: &str) -> Result<bool> {
        let (device_id, token) = (device_id.to_owned(), token.to_owned());
        self.call(move |c| {
            Ok(c.execute(
                "UPDATE devices SET push_token = NULL WHERE id = ?1 AND push_token = ?2",
                params![device_id, token],
            )? == 1)
        })
        .await
    }

    /// A delivery's state and attempts, for tests and the admin.
    pub async fn delivery(
        &self,
        outbox_id: i64,
        device_id: &str,
    ) -> Result<Option<(String, i64, Option<String>)>> {
        let device_id = device_id.to_owned();
        self.call(move |c| {
            use tokio_rusqlite::rusqlite::OptionalExtension;
            Ok(c.query_row(
                "SELECT state, attempts, last_error FROM outbox_deliveries WHERE outbox_id = ?1 AND device_id = ?2",
                params![outbox_id, device_id],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .optional()?)
        })
        .await
    }
}
