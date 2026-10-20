//! Paired app devices: pairing codes, device keys, challenges and short-lived tokens.

use tokio_rusqlite::rusqlite::{self, OptionalExtension, Row, params};

use super::{OutboxKind, Result, Store, StoreError, queue};

str_enum!(DeviceState, "state" {
    Pending => "pending",
    Active => "active",
    Rejected => "rejected",
    Revoked => "revoked",
});

str_enum!(ChallengePurpose, "purpose" {
    Token => "token",
    Approval => "approval",
});

str_enum!(PushEnv, "push_env" {
    Sandbox => "sandbox",
    Production => "production",
});

#[derive(Debug, Clone, PartialEq)]
pub struct DeviceRecord {
    pub id: String,
    pub name: String,
    /// P-256 public keys, SEC1 uncompressed, base64url.
    pub auth_key: String,
    pub approval_key: String,
    pub fingerprint: String,
    pub state: DeviceState,
    pub client_build: Option<i64>,
    pub created_at: i64,
    pub confirmed_at: Option<i64>,
    pub revoked_at: Option<i64>,
    pub last_seen_at: Option<i64>,
    pub push_token: Option<String>,
    pub push_env: Option<PushEnv>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct NewDevice {
    pub id: String,
    pub name: String,
    pub auth_key: String,
    pub approval_key: String,
    pub fingerprint: String,
    pub client_build: Option<i64>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Challenge {
    pub nonce: String,
    pub device_id: String,
    pub purpose: ChallengePurpose,
    pub approval_id: Option<String>,
    pub created_at: i64,
    pub expires_at: i64,
    pub used_at: Option<i64>,
}

/// A verified signature over an approval challenge; the decision consumes the nonce.
#[derive(Debug, Clone, PartialEq)]
pub struct ApprovalProof {
    pub device_id: String,
    pub nonce: String,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Claim {
    Claimed,
    /// Unknown, expired or already used; the caller can't tell which.
    Invalid,
}

impl Store {
    pub async fn hub_id(&self) -> Result<String> {
        self.call(|c| {
            Ok(
                c.query_row("SELECT hub_id FROM hub_identity WHERE id = 1", [], |r| {
                    r.get(0)
                })?,
            )
        })
        .await
    }

    // Pairing.

    pub async fn create_pairing_code(
        &self,
        code_hash: &str,
        now: i64,
        expires_at: i64,
    ) -> Result<()> {
        let code_hash = code_hash.to_owned();
        self.call(move |c| {
            c.execute(
                "INSERT INTO pairing_codes (code_hash, created_at, expires_at) VALUES (?1, ?2, ?3)",
                params![code_hash, now, expires_at],
            )?;
            Ok(())
        })
        .await
    }

    /// Uses the code once, recording the device as pending its fingerprint check.
    pub async fn claim_pairing_code(
        &self,
        code_hash: &str,
        d: NewDevice,
        now: i64,
    ) -> Result<Claim> {
        let code_hash = code_hash.to_owned();
        self.call(move |c| {
            let tx = c.transaction()?;
            let n = tx.execute(
                "UPDATE pairing_codes SET claimed_at = ?2, device_id = ?3
                 WHERE code_hash = ?1 AND claimed_at IS NULL AND expires_at > ?2",
                params![code_hash, now, d.id],
            )?;
            if n == 0 {
                return Ok(Claim::Invalid);
            }
            tx.execute(
                "INSERT INTO devices (id, name, auth_key, approval_key, fingerprint, state,
                 client_build, created_at) VALUES (?1, ?2, ?3, ?4, ?5, 'pending', ?6, ?7)",
                params![
                    d.id,
                    d.name,
                    d.auth_key,
                    d.approval_key,
                    d.fingerprint,
                    d.client_build,
                    now
                ],
            )?;
            tx.commit()?;
            Ok(Claim::Claimed)
        })
        .await
    }

    /// The device that claimed this code, if any.
    pub async fn pairing_claim(&self, code_hash: &str) -> Result<Option<DeviceRecord>> {
        let code_hash = code_hash.to_owned();
        self.call(move |c| {
            c.query_row(
                "SELECT d.* FROM pairing_codes p JOIN devices d ON d.id = p.device_id
                 WHERE p.code_hash = ?1",
                [code_hash],
                raw_device,
            )
            .optional()?
            .map(RawDevice::finish)
            .transpose()
        })
        .await
    }

    /// Settles a pending device. Activation announces it to the devices already paired.
    pub async fn settle_pending_device(&self, id: &str, accept: bool, now: i64) -> Result<bool> {
        let id = id.to_owned();
        self.call(move |c| {
            let tx = c.transaction()?;
            let state = if accept { "active" } else { "rejected" };
            let n = tx.execute(
                "UPDATE devices SET state = ?2, confirmed_at = ?3 WHERE id = ?1 AND state = 'pending'",
                params![id, state, now],
            )?;
            if n == 1 && accept {
                queue(
                    &tx,
                    &format!("device_paired:{id}"),
                    OutboxKind::DevicePaired,
                    None,
                    None,
                    now,
                    Some(&id),
                )?;
            }
            tx.commit()?;
            Ok(n == 1)
        })
        .await
    }

    // Devices.

    pub async fn device(&self, id: &str) -> Result<Option<DeviceRecord>> {
        let id = id.to_owned();
        self.call(move |c| {
            c.query_row("SELECT * FROM devices WHERE id = ?1", [id], raw_device)
                .optional()?
                .map(RawDevice::finish)
                .transpose()
        })
        .await
    }

    pub async fn devices(&self) -> Result<Vec<DeviceRecord>> {
        self.call(|c| {
            let mut stmt = c.prepare("SELECT * FROM devices ORDER BY created_at")?;
            let rows = stmt.query_map([], raw_device)?;
            rows.map(|r| r?.finish()).collect()
        })
        .await
    }

    pub async fn active_device_count(&self) -> Result<i64> {
        self.call(|c| {
            Ok(c.query_row(
                "SELECT COUNT(*) FROM devices WHERE state = 'active'",
                [],
                |r| r.get(0),
            )?)
        })
        .await
    }

    /// Revokes a device: its tokens go at once and nothing more is sent to it.
    pub async fn revoke_device(&self, id: &str, now: i64) -> Result<Option<DeviceRecord>> {
        let id = id.to_owned();
        self.call(move |c| {
            let tx = c.transaction()?;
            let d = tx
                .query_row(
                    "UPDATE devices SET state = 'revoked', revoked_at = ?2, push_token = NULL
                     WHERE id = ?1 AND state IN ('pending', 'active') RETURNING *",
                    params![id, now],
                    raw_device,
                )
                .optional()?
                .map(RawDevice::finish)
                .transpose()?;
            tx.execute("DELETE FROM device_tokens WHERE device_id = ?1", [&id])?;
            tx.execute(
                "UPDATE outbox_deliveries SET state = 'dropped', last_error = 'device revoked'
                 WHERE device_id = ?1 AND state = 'pending'",
                [&id],
            )?;
            tx.commit()?;
            Ok(d)
        })
        .await
    }

    /// The devices a notification is queued for.
    pub async fn deliveries_of(&self, outbox_id: i64) -> Result<Vec<String>> {
        self.call(move |c| {
            let mut stmt = c.prepare(
                "SELECT device_id FROM outbox_deliveries WHERE outbox_id = ?1 ORDER BY device_id",
            )?;
            let rows = stmt.query_map([outbox_id], |r| r.get(0))?;
            Ok(rows.collect::<rusqlite::Result<_>>()?)
        })
        .await
    }

    pub async fn set_push_token(
        &self,
        device_id: &str,
        token: Option<&str>,
        env: Option<PushEnv>,
        now: i64,
    ) -> Result<bool> {
        let (device_id, token) = (device_id.to_owned(), token.map(str::to_owned));
        self.call(move |c| {
            Ok(c.execute(
                "UPDATE devices SET push_token = ?2, push_env = ?3, push_token_at = ?4
                 WHERE id = ?1 AND state = 'active'",
                params![device_id, token, env.map(PushEnv::as_str), now],
            )? == 1)
        })
        .await
    }

    // Challenges and tokens.

    pub async fn create_challenge(&self, ch: Challenge) -> Result<()> {
        self.call(move |c| {
            c.execute(
                "INSERT INTO challenges (nonce, device_id, purpose, approval_id, created_at, expires_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                params![
                    ch.nonce,
                    ch.device_id,
                    ch.purpose.as_str(),
                    ch.approval_id,
                    ch.created_at,
                    ch.expires_at
                ],
            )?;
            Ok(())
        })
        .await
    }

    pub async fn challenge(&self, nonce: &str) -> Result<Option<Challenge>> {
        let nonce = nonce.to_owned();
        self.call(move |c| {
            c.query_row(
                "SELECT * FROM challenges WHERE nonce = ?1",
                [nonce],
                raw_challenge,
            )
            .optional()?
            .map(RawChallenge::finish)
            .transpose()
        })
        .await
    }

    /// Uses a token challenge and records the new token in one transaction.
    pub async fn redeem_token_challenge(
        &self,
        nonce: &str,
        device_id: &str,
        token_hash: &str,
        now: i64,
        token_expires_at: i64,
    ) -> Result<bool> {
        let args = [nonce, device_id, token_hash].map(str::to_owned);
        self.call(move |c| {
            let tx = c.transaction()?;
            if !use_challenge(&tx, &args[0], &args[1], ChallengePurpose::Token, None, now)? {
                return Ok(false);
            }
            tx.execute(
                "INSERT INTO device_tokens (token_hash, device_id, created_at, expires_at)
                 VALUES (?1, ?2, ?3, ?4)",
                params![args[2], args[1], now, token_expires_at],
            )?;
            tx.execute(
                "DELETE FROM device_tokens WHERE device_id = ?1 AND expires_at <= ?2",
                params![args[1], now],
            )?;
            tx.commit()?;
            Ok(true)
        })
        .await
    }

    /// The active device holding this unexpired token.
    pub async fn device_for_token(
        &self,
        token_hash: &str,
        now: i64,
    ) -> Result<Option<DeviceRecord>> {
        let token_hash = token_hash.to_owned();
        self.call(move |c| {
            let d = c
                .query_row(
                    "SELECT d.* FROM device_tokens t JOIN devices d ON d.id = t.device_id
                     WHERE t.token_hash = ?1 AND t.expires_at > ?2 AND d.state = 'active'",
                    params![token_hash, now],
                    raw_device,
                )
                .optional()?
                .map(RawDevice::finish)
                .transpose()?;
            if let Some(d) = &d {
                c.execute(
                    "UPDATE devices SET last_seen_at = ?2 WHERE id = ?1",
                    params![d.id, now],
                )?;
            }
            Ok(d)
        })
        .await
    }
}

/// Marks a challenge used if it matches, is unused and unexpired, and its device is active.
pub(super) fn use_challenge(
    tx: &rusqlite::Transaction<'_>,
    nonce: &str,
    device_id: &str,
    purpose: ChallengePurpose,
    approval_id: Option<&str>,
    now: i64,
) -> Result<bool> {
    let n = tx.execute(
        "UPDATE challenges SET used_at = ?5
         WHERE nonce = ?1 AND device_id = ?2 AND purpose = ?3 AND approval_id IS ?4
         AND used_at IS NULL AND expires_at > ?5
         AND EXISTS (SELECT 1 FROM devices WHERE id = ?2 AND state = 'active')",
        params![nonce, device_id, purpose.as_str(), approval_id, now],
    )?;
    Ok(n == 1)
}

struct RawDevice(DeviceRecord, String, Option<String>);

fn raw_device(r: &Row<'_>) -> rusqlite::Result<RawDevice> {
    Ok(RawDevice(
        DeviceRecord {
            id: r.get("id")?,
            name: r.get("name")?,
            auth_key: r.get("auth_key")?,
            approval_key: r.get("approval_key")?,
            fingerprint: r.get("fingerprint")?,
            state: DeviceState::Pending,
            client_build: r.get("client_build")?,
            created_at: r.get("created_at")?,
            confirmed_at: r.get("confirmed_at")?,
            revoked_at: r.get("revoked_at")?,
            last_seen_at: r.get("last_seen_at")?,
            push_token: r.get("push_token")?,
            push_env: None,
        },
        r.get("state")?,
        r.get("push_env")?,
    ))
}

impl RawDevice {
    fn finish(self) -> Result<DeviceRecord> {
        Ok(DeviceRecord {
            state: DeviceState::parse(self.1)?,
            push_env: self.2.map(PushEnv::parse).transpose()?,
            ..self.0
        })
    }
}

struct RawChallenge(Challenge, String);

fn raw_challenge(r: &Row<'_>) -> rusqlite::Result<RawChallenge> {
    Ok(RawChallenge(
        Challenge {
            nonce: r.get("nonce")?,
            device_id: r.get("device_id")?,
            purpose: ChallengePurpose::Token,
            approval_id: r.get("approval_id")?,
            created_at: r.get("created_at")?,
            expires_at: r.get("expires_at")?,
            used_at: r.get("used_at")?,
        },
        r.get("purpose")?,
    ))
}

impl RawChallenge {
    fn finish(self) -> Result<Challenge> {
        Ok(Challenge {
            purpose: ChallengePurpose::parse(self.1)?,
            ..self.0
        })
    }
}
