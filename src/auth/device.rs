//! Paired app devices. A phone pairs with a single-use code from `switchboard admin pair-device`,
//! and the admin confirms the key fingerprint the phone shows. Each device holds two P-256 keys:
//! an auth key that signs token challenges, and an approval key, behind Face ID or the passcode,
//! that signs approvals. App and bot identities never mix: device tokens work only on the app API.

use std::collections::VecDeque;
use std::sync::Mutex;
use std::time::Duration;

use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD as B64;
use p256::ecdsa::signature::Verifier;
use p256::ecdsa::{Signature, VerifyingKey};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

use crate::hub::service::Clock;
use crate::proto::auth::hash_token;
use crate::runner::random_id;
use crate::store::{
    ApprovalProof, Challenge, ChallengePurpose, Claim, DeviceRecord, DeviceState, NewDevice, Store,
    StoreError,
};

pub const CODE_TTL: Duration = Duration::from_secs(10 * 60);
pub const CHALLENGE_TTL: Duration = Duration::from_secs(60);
pub const TOKEN_TTL: Duration = Duration::from_secs(15 * 60);
/// The signed message format; anything else is refused.
pub const MESSAGE_VERSION: u32 = 1;
/// Failed pairing claims allowed per window, across all callers.
pub const PAIR_ATTEMPTS: usize = 5;
pub const PAIR_WINDOW: Duration = Duration::from_secs(10 * 60);
const MAX_NAME_CHARS: usize = 64;

#[derive(Debug, thiserror::Error)]
pub enum AuthError {
    #[error("the pairing code is unknown, used or expired")]
    InvalidCode,
    #[error("too many pairing attempts; ask for a new code later")]
    RateLimited,
    #[error("bad device key: {0}")]
    BadKey(String),
    #[error("bad request: {0}")]
    BadRequest(String),
    #[error("unknown or inactive device")]
    UnknownDevice,
    #[error("the challenge is unknown, used or expired")]
    InvalidChallenge,
    #[error("the signature does not verify")]
    BadSignature,
    #[error("signed message version {0} is not supported")]
    UnsupportedVersion(u32),
    #[error("missing, expired or revoked token")]
    InvalidToken,
    #[error("store: {0}")]
    Store(#[from] StoreError),
}

impl AuthError {
    pub fn code(&self) -> &'static str {
        match self {
            AuthError::InvalidCode => "invalid_code",
            AuthError::RateLimited => "rate_limited",
            AuthError::BadKey(_) => "bad_key",
            AuthError::BadRequest(_) => "bad_request",
            AuthError::UnknownDevice => "unknown_device",
            AuthError::InvalidChallenge => "invalid_challenge",
            AuthError::BadSignature => "bad_signature",
            AuthError::UnsupportedVersion(_) => "unsupported_version",
            AuthError::InvalidToken => "invalid_token",
            AuthError::Store(_) => "internal",
        }
    }
}

// Canonical messages: ASCII lines joined by `\n`, no trailing newline. Every field is an id,
// a decision or a number, none of which can hold a newline.

pub fn token_message(hub_id: &str, device_id: &str, nonce: &str, expires_at: i64) -> String {
    format!(
        "switchboard-token-v{MESSAGE_VERSION}\nhub={hub_id}\ndevice={device_id}\nnonce={nonce}\nexpires={expires_at}"
    )
}

pub fn approval_message(
    hub_id: &str,
    device_id: &str,
    approval_id: &str,
    decision: &str,
    nonce: &str,
    expires_at: i64,
) -> String {
    format!(
        "switchboard-approval-v{MESSAGE_VERSION}\nhub={hub_id}\ndevice={device_id}\napproval={approval_id}\ndecision={decision}\nnonce={nonce}\nexpires={expires_at}"
    )
}

/// What the phone and the admin terminal both show: SHA-256 over the hub and both keys, as six
/// groups of four hex digits.
pub fn fingerprint(hub_id: &str, auth_key: &str, approval_key: &str) -> String {
    let digest = Sha256::digest(format!("{hub_id}\n{auth_key}\n{approval_key}").as_bytes());
    hex::encode(&digest[..12])
        .as_bytes()
        .chunks(4)
        .map(|c| std::str::from_utf8(c).expect("hex is ascii"))
        .collect::<Vec<_>>()
        .join("-")
}

/// A P-256 public key in SEC1 form, base64url without padding.
pub fn parse_key(b64: &str) -> Result<VerifyingKey, AuthError> {
    let bytes = B64
        .decode(b64)
        .map_err(|e| AuthError::BadKey(format!("not base64url: {e}")))?;
    VerifyingKey::from_sec1_bytes(&bytes).map_err(|_| AuthError::BadKey("not a P-256 key".into()))
}

/// Checks a DER ECDSA P-256 signature over SHA-256 of `message`, as the Secure Enclave makes it.
pub fn verify(key_b64: &str, message: &str, signature_b64: &str) -> bool {
    let Ok(key) = parse_key(key_b64) else {
        return false;
    };
    let Ok(der) = B64.decode(signature_b64) else {
        return false;
    };
    let Ok(sig) = Signature::from_der(&der) else {
        return false;
    };
    key.verify(message.as_bytes(), &sig).is_ok()
}

/// Failed pairing claims in a sliding window.
#[derive(Default)]
pub struct PairLimiter {
    failures: Mutex<VecDeque<i64>>,
}

impl PairLimiter {
    fn allowed(&self, now: i64) -> bool {
        let mut f = self.failures.lock().unwrap();
        let since = now - PAIR_WINDOW.as_millis() as i64;
        while f.front().is_some_and(|t| *t <= since) {
            f.pop_front();
        }
        f.len() < PAIR_ATTEMPTS
    }

    fn failed(&self, now: i64) {
        self.failures.lock().unwrap().push_back(now);
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct PairingCode {
    pub code: String,
    pub code_hash: String,
    pub expires_at: i64,
    pub hub_id: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Claimed {
    pub device_id: String,
    pub fingerprint: String,
    pub hub_id: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct IssuedToken {
    pub access_token: String,
    pub expires_at: i64,
}

pub struct ClaimRequest<'a> {
    pub code: &'a str,
    pub name: &'a str,
    pub auth_key: &'a str,
    pub approval_key: &'a str,
    pub client_build: Option<i64>,
}

/// Device pairing, tokens and signed approvals over the hub's store.
pub struct Devices {
    store: Store,
    clock: Clock,
    limiter: PairLimiter,
}

impl Devices {
    pub fn new(store: Store, clock: Clock) -> Self {
        Self {
            store,
            clock,
            limiter: PairLimiter::default(),
        }
    }

    fn now(&self) -> i64 {
        self.clock.now_ms()
    }

    async fn audit(&self, actor: &str, kind: &str, detail: Value) -> Result<(), AuthError> {
        self.store
            .append_audit(self.now(), actor, None, kind, &detail)
            .await?;
        Ok(())
    }

    pub async fn hub_id(&self) -> Result<String, AuthError> {
        Ok(self.store.hub_id().await?)
    }

    /// A fresh single-use code, valid for ten minutes. Only its hash is stored.
    pub async fn issue_code(&self) -> Result<PairingCode, AuthError> {
        let code = format!("{}{}", random_id(), random_id());
        let code_hash = hash_token(&code);
        let now = self.now();
        let expires_at = now + CODE_TTL.as_millis() as i64;
        self.store
            .create_pairing_code(&code_hash, now, expires_at)
            .await?;
        self.audit(
            "admin",
            "pairing_code_issued",
            json!({ "expires_at": expires_at }),
        )
        .await?;
        Ok(PairingCode {
            code,
            code_hash,
            expires_at,
            hub_id: self.hub_id().await?,
        })
    }

    /// A phone presents the code and its two public keys; the device waits for the admin to
    /// confirm its fingerprint.
    pub async fn claim(&self, req: ClaimRequest<'_>) -> Result<Claimed, AuthError> {
        let now = self.now();
        if !self.limiter.allowed(now) {
            self.audit("app", "pairing_rate_limited", json!({})).await?;
            return Err(AuthError::RateLimited);
        }
        let name = req.name.trim();
        if name.is_empty()
            || name.chars().count() > MAX_NAME_CHARS
            || name.chars().any(char::is_control)
        {
            return Err(AuthError::BadRequest(format!(
                "the device name must be 1-{MAX_NAME_CHARS} printable characters"
            )));
        }
        parse_key(req.auth_key)?;
        parse_key(req.approval_key)?;
        if req.auth_key == req.approval_key {
            return Err(AuthError::BadKey("the two keys must differ".into()));
        }
        let hub_id = self.hub_id().await?;
        let device_id = random_id();
        let fingerprint = fingerprint(&hub_id, req.auth_key, req.approval_key);
        let device = NewDevice {
            id: device_id.clone(),
            name: name.to_owned(),
            auth_key: req.auth_key.to_owned(),
            approval_key: req.approval_key.to_owned(),
            fingerprint: fingerprint.clone(),
            client_build: req.client_build,
        };
        match self
            .store
            .claim_pairing_code(&hash_token(req.code), device, now)
            .await?
        {
            Claim::Claimed => {
                self.audit(
                    "app",
                    "pairing_claimed",
                    json!({ "device_id": device_id, "name": name, "fingerprint": fingerprint }),
                )
                .await?;
                Ok(Claimed {
                    device_id,
                    fingerprint,
                    hub_id,
                })
            }
            Claim::Invalid => {
                self.limiter.failed(now);
                self.audit(
                    "app",
                    "pairing_refused",
                    json!({ "reason": "invalid_code" }),
                )
                .await?;
                Err(AuthError::InvalidCode)
            }
        }
    }

    /// The device that claimed `code_hash`, for the admin command waiting on it.
    pub async fn claim_of(&self, code_hash: &str) -> Result<Option<DeviceRecord>, AuthError> {
        Ok(self.store.pairing_claim(code_hash).await?)
    }

    /// The admin's answer to "does the phone show this fingerprint?".
    pub async fn confirm(&self, device_id: &str, accept: bool) -> Result<bool, AuthError> {
        let settled = self
            .store
            .settle_pending_device(device_id, accept, self.now())
            .await?;
        if settled {
            let kind = if accept {
                "device_paired"
            } else {
                "pairing_rejected"
            };
            self.audit("admin", kind, json!({ "device_id": device_id }))
                .await?;
        }
        Ok(settled)
    }

    pub async fn device(&self, device_id: &str) -> Result<Option<DeviceRecord>, AuthError> {
        Ok(self.store.device(device_id).await?)
    }

    pub async fn list(&self) -> Result<Vec<DeviceRecord>, AuthError> {
        Ok(self.store.devices().await?)
    }

    async fn active(&self, device_id: &str) -> Result<DeviceRecord, AuthError> {
        self.store
            .device(device_id)
            .await?
            .filter(|d| d.state == DeviceState::Active)
            .ok_or(AuthError::UnknownDevice)
    }

    async fn challenge(
        &self,
        device_id: &str,
        purpose: ChallengePurpose,
        approval_id: Option<&str>,
    ) -> Result<Challenge, AuthError> {
        let now = self.now();
        let ch = Challenge {
            nonce: random_id(),
            device_id: device_id.to_owned(),
            purpose,
            approval_id: approval_id.map(str::to_owned),
            created_at: now,
            expires_at: now + CHALLENGE_TTL.as_millis() as i64,
            used_at: None,
        };
        self.store.create_challenge(ch.clone()).await?;
        Ok(ch)
    }

    pub async fn token_challenge(&self, device_id: &str) -> Result<Challenge, AuthError> {
        self.active(device_id).await?;
        self.challenge(device_id, ChallengePurpose::Token, None)
            .await
    }

    /// A short-lived access token for a signature over a token challenge.
    pub async fn issue_token(
        &self,
        device_id: &str,
        nonce: &str,
        signature: &str,
        version: u32,
    ) -> Result<IssuedToken, AuthError> {
        if version != MESSAGE_VERSION {
            return Err(AuthError::UnsupportedVersion(version));
        }
        let device = self.active(device_id).await?;
        let ch = self
            .store
            .challenge(nonce)
            .await?
            .filter(|c| c.device_id == device_id && c.purpose == ChallengePurpose::Token)
            .ok_or(AuthError::InvalidChallenge)?;
        let message = token_message(&self.hub_id().await?, device_id, nonce, ch.expires_at);
        if !verify(&device.auth_key, &message, signature) {
            self.audit(
                &format!("app:{device_id}"),
                "token_refused",
                json!({ "reason": "bad_signature" }),
            )
            .await?;
            return Err(AuthError::BadSignature);
        }
        let token = format!("{}{}", random_id(), random_id());
        let now = self.now();
        let expires_at = now + TOKEN_TTL.as_millis() as i64;
        if !self
            .store
            .redeem_token_challenge(nonce, device_id, &hash_token(&token), now, expires_at)
            .await?
        {
            return Err(AuthError::InvalidChallenge);
        }
        Ok(IssuedToken {
            access_token: token,
            expires_at,
        })
    }

    /// The active device behind a bearer token. Revoking a device fails its next request.
    pub async fn authenticate(&self, token: &str) -> Result<DeviceRecord, AuthError> {
        self.store
            .device_for_token(&hash_token(token), self.now())
            .await?
            .ok_or(AuthError::InvalidToken)
    }

    pub async fn approval_challenge(
        &self,
        device_id: &str,
        approval_id: &str,
    ) -> Result<Challenge, AuthError> {
        self.active(device_id).await?;
        self.challenge(device_id, ChallengePurpose::Approval, Some(approval_id))
            .await
    }

    /// Checks a signed approve. The returned proof's nonce is used only when the decision is
    /// recorded, in the same transaction.
    pub async fn verify_approval(
        &self,
        device: &DeviceRecord,
        approval_id: &str,
        nonce: &str,
        signature: &str,
        version: u32,
    ) -> Result<ApprovalProof, AuthError> {
        if version != MESSAGE_VERSION {
            return Err(AuthError::UnsupportedVersion(version));
        }
        let ch = self
            .store
            .challenge(nonce)
            .await?
            .filter(|c| {
                c.device_id == device.id
                    && c.purpose == ChallengePurpose::Approval
                    && c.approval_id.as_deref() == Some(approval_id)
                    && c.used_at.is_none()
                    && c.expires_at > self.now()
            })
            .ok_or(AuthError::InvalidChallenge)?;
        let message = approval_message(
            &self.hub_id().await?,
            &device.id,
            approval_id,
            "allow",
            nonce,
            ch.expires_at,
        );
        if !verify(&device.approval_key, &message, signature) {
            self.audit(
                &format!("app:{}", device.id),
                "approval_signature_refused",
                json!({ "approval_id": approval_id }),
            )
            .await?;
            return Err(AuthError::BadSignature);
        }
        Ok(ApprovalProof {
            device_id: device.id.clone(),
            nonce: nonce.to_owned(),
        })
    }

    /// Revokes a device; `by` is the actor for the audit log.
    pub async fn revoke(&self, device_id: &str, by: &str) -> Result<bool, AuthError> {
        let revoked = self.store.revoke_device(device_id, self.now()).await?;
        if revoked.is_some() {
            self.audit(by, "device_revoked", json!({ "device_id": device_id }))
                .await?;
        }
        Ok(revoked.is_some())
    }
}

#[cfg(test)]
#[path = "device_tests.rs"]
pub(crate) mod tests;
