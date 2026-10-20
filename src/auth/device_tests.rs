use p256::ecdsa::SigningKey;
use p256::ecdsa::signature::Signer;

use super::*;
use crate::store::OutboxKind;

const T0: i64 = 1_700_000_000_000;

/// A software stand-in for a Secure Enclave key.
pub(crate) struct Key(SigningKey);

impl Key {
    pub(crate) fn new() -> Self {
        loop {
            let mut b = [0u8; 32];
            getrandom::fill(&mut b).unwrap();
            if let Ok(k) = SigningKey::from_slice(&b) {
                return Key(k);
            }
        }
    }

    pub(crate) fn public(&self) -> String {
        B64.encode(self.0.verifying_key().to_sec1_bytes())
    }

    pub(crate) fn sign(&self, message: &str) -> String {
        let sig: Signature = self.0.sign(message.as_bytes());
        B64.encode(sig.to_der().as_bytes())
    }
}

pub(crate) struct Phone {
    pub(crate) auth: Key,
    pub(crate) approval: Key,
}

impl Phone {
    pub(crate) fn new() -> Self {
        Phone {
            auth: Key::new(),
            approval: Key::new(),
        }
    }

    fn claim<'a>(&'a self, code: &'a str, auth: &'a str, approval: &'a str) -> ClaimRequest<'a> {
        ClaimRequest {
            code,
            name: "iPhone",
            auth_key: auth,
            approval_key: approval,
            client_build: Some(1),
        }
    }
}

async fn devices() -> (Devices, Clock) {
    let clock = Clock::manual(T0);
    (
        Devices::new(Store::open_in_memory().await.unwrap(), clock.clone()),
        clock,
    )
}

/// A phone through code, claim and confirmation; returns its device id.
pub(crate) async fn paired(d: &Devices, phone: &Phone) -> String {
    let code = d.issue_code().await.unwrap();
    let (a, b) = (phone.auth.public(), phone.approval.public());
    let claimed = d.claim(phone.claim(&code.code, &a, &b)).await.unwrap();
    assert!(d.confirm(&claimed.device_id, true).await.unwrap());
    claimed.device_id
}

pub(crate) async fn token(d: &Devices, phone: &Phone, device_id: &str) -> IssuedToken {
    let ch = d.token_challenge(device_id).await.unwrap();
    let msg = token_message(
        &d.hub_id().await.unwrap(),
        device_id,
        &ch.nonce,
        ch.expires_at,
    );
    d.issue_token(
        device_id,
        &ch.nonce,
        &phone.auth.sign(&msg),
        MESSAGE_VERSION,
    )
    .await
    .unwrap()
}

#[test]
fn canonical_messages_and_fingerprints_are_stable() {
    assert_eq!(
        approval_message("h1", "d1", "a1", "allow", "n1", 42),
        "switchboard-approval-v1\nhub=h1\ndevice=d1\napproval=a1\ndecision=allow\nnonce=n1\nexpires=42"
    );
    assert_eq!(
        token_message("h1", "d1", "n1", 42),
        "switchboard-token-v1\nhub=h1\ndevice=d1\nnonce=n1\nexpires=42"
    );
    let f = fingerprint("h1", "k1", "k2");
    // The iOS app's tests pin the same vector.
    assert_eq!(f, "a553-4998-583d-047b-1245-3ce8");
    assert_eq!(f.matches('-').count(), 5);
    assert_ne!(f, fingerprint("h2", "k1", "k2"), "bound to the hub");
    assert_ne!(
        f,
        fingerprint("h1", "k2", "k1"),
        "bound to which key is which"
    );
}

#[test]
fn signatures_verify_only_for_their_key_and_message() {
    let (k, other) = (Key::new(), Key::new());
    let sig = k.sign("hello");
    assert!(verify(&k.public(), "hello", &sig));
    assert!(!verify(&k.public(), "hello!", &sig));
    assert!(!verify(&other.public(), "hello", &sig));
    assert!(!verify(&k.public(), "hello", "not-base64!"));
    assert!(!verify("bad", "hello", &sig));
}

#[tokio::test]
async fn pairing_needs_a_fresh_single_use_code_and_a_confirmed_fingerprint() {
    let (d, clock) = devices().await;
    let phone = Phone::new();
    let (a, b) = (phone.auth.public(), phone.approval.public());

    let code = d.issue_code().await.unwrap();
    let claimed = d.claim(phone.claim(&code.code, &a, &b)).await.unwrap();
    assert_eq!(claimed.fingerprint, fingerprint(&claimed.hub_id, &a, &b));
    let waiting = d.claim_of(&code.code_hash).await.unwrap().unwrap();
    assert_eq!(waiting.state, DeviceState::Pending);
    assert_eq!(waiting.fingerprint, claimed.fingerprint);

    let reused = d.claim(phone.claim(&code.code, &a, &b)).await.unwrap_err();
    assert_eq!(reused.code(), "invalid_code");
    // Unconfirmed: no token yet.
    assert_eq!(
        d.token_challenge(&claimed.device_id)
            .await
            .unwrap_err()
            .code(),
        "unknown_device"
    );
    assert!(d.confirm(&claimed.device_id, true).await.unwrap());
    assert!(
        !d.confirm(&claimed.device_id, false).await.unwrap(),
        "settled once"
    );
    token(&d, &phone, &claimed.device_id).await;

    let late = d.issue_code().await.unwrap();
    clock.advance(CODE_TTL);
    let expired = d.claim(phone.claim(&late.code, &a, &b)).await.unwrap_err();
    assert_eq!(expired.code(), "invalid_code");

    let rejected_code = d.issue_code().await.unwrap();
    let other = Phone::new();
    let (oa, ob) = (other.auth.public(), other.approval.public());
    let c = d
        .claim(other.claim(&rejected_code.code, &oa, &ob))
        .await
        .unwrap();
    assert!(d.confirm(&c.device_id, false).await.unwrap());
    assert_eq!(
        d.token_challenge(&c.device_id).await.unwrap_err().code(),
        "unknown_device"
    );

    let kinds: Vec<String> = d
        .store
        .list_audit(None, 50)
        .await
        .unwrap()
        .into_iter()
        .map(|e| e.kind)
        .collect();
    for k in [
        "pairing_code_issued",
        "pairing_claimed",
        "device_paired",
        "pairing_refused",
        "pairing_rejected",
    ] {
        assert!(kinds.iter().any(|x| x == k), "{k} missing from {kinds:?}");
    }
    let audit = serde_json::to_string(
        &d.store
            .list_audit(None, 50)
            .await
            .unwrap()
            .iter()
            .map(|e| &e.detail)
            .collect::<Vec<_>>(),
    )
    .unwrap();
    assert!(!audit.contains(&code.code), "codes are never logged");
}

#[tokio::test]
async fn bad_keys_and_names_are_refused() {
    let (d, _) = devices().await;
    let phone = Phone::new();
    let code = d.issue_code().await.unwrap();
    let a = phone.auth.public();
    let e = d
        .claim(phone.claim(&code.code, &a, "AAAA"))
        .await
        .unwrap_err();
    assert_eq!(e.code(), "bad_key");
    let e = d.claim(phone.claim(&code.code, &a, &a)).await.unwrap_err();
    assert_eq!(e.code(), "bad_key");
    let mut req = phone.claim(&code.code, &a, &a);
    req.name = "\u{7}";
    assert_eq!(d.claim(req).await.unwrap_err().code(), "bad_request");
    let b = phone.approval.public();
    d.claim(phone.claim(&code.code, &a, &b)).await.unwrap();
}

#[tokio::test]
async fn failed_claims_are_rate_limited_even_for_a_valid_code() {
    let (d, clock) = devices().await;
    let phone = Phone::new();
    let (a, b) = (phone.auth.public(), phone.approval.public());
    for _ in 0..PAIR_ATTEMPTS {
        let e = d.claim(phone.claim("guess", &a, &b)).await.unwrap_err();
        assert_eq!(e.code(), "invalid_code");
    }
    let code = d.issue_code().await.unwrap();
    let e = d.claim(phone.claim(&code.code, &a, &b)).await.unwrap_err();
    assert_eq!(e.code(), "rate_limited");
    clock.advance(PAIR_WINDOW);
    let fresh = d.issue_code().await.unwrap();
    d.claim(phone.claim(&fresh.code, &a, &b)).await.unwrap();
}

#[tokio::test]
async fn a_new_device_is_announced_to_the_devices_already_paired() {
    let (d, _) = devices().await;
    let first = Phone::new();
    let first_id = paired(&d, &first).await;
    let second_id = paired(&d, &Phone::new()).await;
    let queued = d.store.outbox_after(0, 10).await.unwrap();
    let announced: Vec<_> = queued
        .iter()
        .filter(|e| e.kind == OutboxKind::DevicePaired)
        .collect();
    assert_eq!(announced.len(), 2);
    let to = d.store.deliveries_of(announced[1].id).await.unwrap();
    assert_eq!(
        to,
        [first_id],
        "the new device {second_id} isn't told about itself"
    );
    assert!(
        d.store
            .deliveries_of(announced[0].id)
            .await
            .unwrap()
            .is_empty()
    );
}

#[tokio::test]
async fn tokens_need_a_signature_from_the_devices_auth_key_over_a_fresh_challenge() {
    let (d, clock) = devices().await;
    let phone = Phone::new();
    let id = paired(&d, &phone).await;
    let hub = d.hub_id().await.unwrap();

    let ch = d.token_challenge(&id).await.unwrap();
    let msg = token_message(&hub, &id, &ch.nonce, ch.expires_at);
    let wrong_key = phone.approval.sign(&msg);
    assert_eq!(
        d.issue_token(&id, &ch.nonce, &wrong_key, 1)
            .await
            .unwrap_err()
            .code(),
        "bad_signature"
    );
    let good = phone.auth.sign(&msg);
    assert_eq!(
        d.issue_token(&id, &ch.nonce, &good, 0)
            .await
            .unwrap_err()
            .code(),
        "unsupported_version"
    );
    let t = d.issue_token(&id, &ch.nonce, &good, 1).await.unwrap();
    assert_eq!(
        d.issue_token(&id, &ch.nonce, &good, 1)
            .await
            .unwrap_err()
            .code(),
        "invalid_challenge",
        "a nonce is used once"
    );
    assert_eq!(d.authenticate(&t.access_token).await.unwrap().id, id);
    assert_eq!(
        d.authenticate("nope").await.unwrap_err().code(),
        "invalid_token"
    );

    let stale = d.token_challenge(&id).await.unwrap();
    clock.advance(CHALLENGE_TTL);
    let msg = token_message(&hub, &id, &stale.nonce, stale.expires_at);
    assert_eq!(
        d.issue_token(&id, &stale.nonce, &phone.auth.sign(&msg), 1)
            .await
            .unwrap_err()
            .code(),
        "invalid_challenge",
        "an expired challenge"
    );

    // Another device's challenge can't be redeemed with this one's id.
    let other = Phone::new();
    let other_id = paired(&d, &other).await;
    let ch = d.token_challenge(&other_id).await.unwrap();
    let msg = token_message(&hub, &id, &ch.nonce, ch.expires_at);
    assert_eq!(
        d.issue_token(&id, &ch.nonce, &phone.auth.sign(&msg), 1)
            .await
            .unwrap_err()
            .code(),
        "invalid_challenge"
    );

    clock.advance(TOKEN_TTL);
    assert_eq!(
        d.authenticate(&t.access_token).await.unwrap_err().code(),
        "invalid_token",
        "tokens are short-lived"
    );
}

#[tokio::test]
async fn revoking_a_device_rejects_its_next_request() {
    let (d, _) = devices().await;
    let phone = Phone::new();
    let id = paired(&d, &phone).await;
    let t = token(&d, &phone, &id).await;
    let ch = d.token_challenge(&id).await.unwrap();
    assert!(d.revoke(&id, "admin").await.unwrap());
    assert!(!d.revoke(&id, "admin").await.unwrap());
    assert_eq!(
        d.authenticate(&t.access_token).await.unwrap_err().code(),
        "invalid_token"
    );
    let msg = token_message(&d.hub_id().await.unwrap(), &id, &ch.nonce, ch.expires_at);
    assert_eq!(
        d.issue_token(&id, &ch.nonce, &phone.auth.sign(&msg), 1)
            .await
            .unwrap_err()
            .code(),
        "unknown_device"
    );
    // Lost-device recovery: pair the replacement, then revoke the old one from it or the admin.
    let replacement = Phone::new();
    let new_id = paired(&d, &replacement).await;
    token(&d, &replacement, &new_id).await;
}

#[test]
fn a_cryptokit_signature_verifies() {
    // Made once by Apple's CryptoKit, as the phone signs; the fixture also pins the message format.
    let v: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string("tests/fixtures/app-v1/cryptokit-signature.json").unwrap(),
    )
    .unwrap();
    let message = v["message"].as_str().unwrap();
    assert_eq!(
        message,
        approval_message("h1", "d1", "a1", "allow", "n1", 42)
    );
    let (key, sig) = (
        v["public_key"].as_str().unwrap(),
        v["signature"].as_str().unwrap(),
    );
    assert!(verify(key, message, sig));
    assert!(!verify(key, &message.replace("allow", "deny"), sig));
}
