use std::collections::VecDeque;
use std::sync::Mutex;

use p256::ecdsa::signature::Verifier;
use p256::ecdsa::{Signature, SigningKey};
use serde_json::json;

use super::apns::{Apns, Hosts, classify};
use super::*;
use crate::auth::device::Devices;
use crate::auth::device::tests::{Phone, paired};
use crate::store::{ApprovalRecord, Decision, Lifecycle, OutboxKind, SessionRecord};

const T0: i64 = 1_700_000_000_000;

/// Records what it sends and answers from a script, then `Sent`.
#[derive(Default)]
struct Fake {
    sent: Mutex<Vec<Push>>,
    script: Mutex<VecDeque<SendOutcome>>,
    needs_token: bool,
}

impl Sender for Arc<Fake> {
    fn needs_token(&self) -> bool {
        self.needs_token
    }

    async fn send(&self, push: &Push) -> SendOutcome {
        self.sent.lock().unwrap().push(push.clone());
        self.script
            .lock()
            .unwrap()
            .pop_front()
            .unwrap_or(SendOutcome::Sent)
    }
}

struct Rig {
    store: Store,
    clock: Clock,
    devices: Devices,
    fake: Arc<Fake>,
}

impl Rig {
    async fn new(store: Store) -> Rig {
        let clock = Clock::manual(T0);
        Rig {
            devices: Devices::new(store.clone(), clock.clone()),
            store,
            clock,
            fake: Arc::default(),
        }
    }

    fn dispatcher(&self) -> Dispatcher<Arc<Fake>> {
        Dispatcher::new(self.store.clone(), self.clock.clone(), self.fake.clone())
    }

    async fn session(&self, id: &str) {
        self.store
            .insert_session(SessionRecord {
                id: id.into(),
                project_id: "p".into(),
                machine_id: "mac".into(),
                agent: "claude".into(),
                prompt_sha256: "x".into(),
                branch: None,
                worktree_path: None,
                workspace_id: None,
                pane_id: None,
                lifecycle: Lifecycle::Running,
                agent_status: "working".into(),
                revision: 0,
                created_at: T0,
                updated_at: T0,
                agent_name: Some(format!("sb-{id}")),
                muted: false,
                hook_failed: false,
                error: None,
                start_commit: None,
                herdr_session: None,
            })
            .await
            .unwrap();
    }

    async fn approval(&self, session: &str, id: &str) {
        self.store
            .create_approval(ApprovalRecord {
                id: id.into(),
                session_id: session.into(),
                tool_name: "Bash".into(),
                tool_input: json!({"command": "curl -H 'Authorization: Bearer abcdefghijklmnop' x"}),
                created_at: self.clock.now_ms(),
                deadline: self.clock.now_ms() + 275_000,
                outcome: None,
                reason: None,
                decided_at: None,
                delivery: None,
                number: None,
            })
            .await
            .unwrap();
    }

    async fn device(&self, token: Option<&str>) -> String {
        let id = paired(&self.devices, &Phone::new()).await;
        if let Some(t) = token {
            self.store
                .set_push_token(&id, Some(t), Some(PushEnv::Sandbox), T0)
                .await
                .unwrap();
        }
        id
    }

    fn sent(&self) -> Vec<Push> {
        self.fake.sent.lock().unwrap().clone()
    }

    async fn state(&self, outbox_id: i64, device: &str) -> String {
        self.store
            .delivery(outbox_id, device)
            .await
            .unwrap()
            .unwrap()
            .0
    }

    async fn last_outbox(&self) -> i64 {
        self.store
            .outbox_after(0, 100)
            .await
            .unwrap()
            .last()
            .unwrap()
            .id
    }
}

#[tokio::test]
async fn an_approval_is_pushed_once_to_every_device_with_generic_text_and_ids_only() {
    let r = Rig::new(Store::open_in_memory().await.unwrap()).await;
    let (d1, d2) = (r.device(Some("aa11")).await, r.device(Some("bb22")).await);
    r.session("s1").await;
    r.approval("s1", "a1").await;
    let d = r.dispatcher();
    // The second device's pairing announcement to the first, plus the approval to both.
    assert_eq!(d.tick().await.unwrap(), 3);
    assert_eq!(d.tick().await.unwrap(), 0, "nothing is sent twice");
    let approvals: Vec<_> = r
        .sent()
        .into_iter()
        .filter(|p| p.payload["kind"] == "approval")
        .collect();
    assert_eq!(approvals.len(), 2);
    let mut to: Vec<_> = approvals.iter().map(|p| p.device_id.clone()).collect();
    to.sort();
    let mut want = vec![d1.clone(), d2];
    want.sort();
    assert_eq!(to, want);
    let p = &approvals[0];
    assert_eq!(p.payload["aps"]["alert"]["title"], "Approval needed");
    assert_eq!(
        (
            p.payload["session_id"].as_str(),
            p.payload["approval_id"].as_str()
        ),
        (Some("s1"), Some("a1"))
    );
    assert_eq!(p.collapse_id, "a1");
    let text = p.payload.to_string();
    assert!(
        !text.contains("curl") && !text.contains("abcdefghijklmnop"),
        "{text}"
    );
    let paired_note = r
        .sent()
        .into_iter()
        .find(|p| p.payload["kind"] == "device_paired")
        .unwrap();
    assert_eq!(
        paired_note.device_id, d1,
        "the new device isn't told about itself"
    );
}

#[tokio::test]
async fn failed_sends_retry_with_backoff_then_give_up() {
    let r = Rig::new(Store::open_in_memory().await.unwrap()).await;
    let dev = r.device(Some("aa11")).await;
    r.session("s1").await;
    r.approval("s1", "a1").await;
    let id = r.last_outbox().await;
    r.fake.script.lock().unwrap().extend([
        SendOutcome::Retry("APNs 503".into()),
        SendOutcome::Retry("APNs 503".into()),
    ]);
    let d = r.dispatcher();
    d.tick().await.unwrap();
    assert_eq!(
        r.store.delivery(id, &dev).await.unwrap().unwrap(),
        ("pending".into(), 1, Some("APNs 503".into()))
    );
    assert_eq!(d.tick().await.unwrap(), 0, "not due yet");
    r.clock.advance(backoff(0));
    d.tick().await.unwrap();
    assert_eq!(r.state(id, &dev).await, "pending");
    r.clock.advance(backoff(1));
    d.tick().await.unwrap();
    assert_eq!(r.state(id, &dev).await, "sent");

    r.approval("s1", "a2").await;
    let id = r.last_outbox().await;
    for _ in 0..MAX_ATTEMPTS {
        r.fake
            .script
            .lock()
            .unwrap()
            .push_back(SendOutcome::Retry("down".into()));
    }
    for i in 0..MAX_ATTEMPTS {
        d.tick().await.unwrap();
        r.clock.advance(backoff(i));
    }
    assert_eq!(
        r.store.delivery(id, &dev).await.unwrap().unwrap().0,
        "failed"
    );
}

#[tokio::test]
async fn a_pending_delivery_survives_a_hub_restart() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("hub.db");
    let r = Rig::new(Store::open(&path).await.unwrap()).await;
    let dev = r.device(Some("aa11")).await;
    r.session("s1").await;
    r.approval("s1", "a1").await;
    let id = r.last_outbox().await;
    r.fake
        .script
        .lock()
        .unwrap()
        .push_back(SendOutcome::Retry("network".into()));
    r.dispatcher().tick().await.unwrap();
    drop(r);

    let r = Rig::new(Store::open(&path).await.unwrap()).await;
    r.clock.advance(backoff(0));
    r.dispatcher().tick().await.unwrap();
    assert_eq!(r.state(id, &dev).await, "sent");
    assert_eq!(r.sent().len(), 1);
}

#[tokio::test]
async fn decided_stale_revoked_and_tokenless_deliveries_are_dropped() {
    let r = Rig::new(Store::open_in_memory().await.unwrap()).await;
    r.session("s1").await;
    let with_token = r.device(Some("aa11")).await;
    let no_token = r.device(None).await;
    r.approval("s1", "decided").await;
    let decided = r.last_outbox().await;
    r.store
        .decide_approval("decided", Decision::Deny, Some("denied"), T0)
        .await
        .unwrap();
    r.approval("s1", "old").await;
    let old = r.last_outbox().await;
    r.clock.advance(STALE_AFTER + Duration::from_secs(1));
    r.approval("s1", "fresh").await;
    let fresh = r.last_outbox().await;
    let revoked = r.device(Some("cc33")).await;
    r.approval("s1", "late").await;
    let late = r.last_outbox().await;
    r.devices.revoke(&revoked, "admin").await.unwrap();

    let tokened = Arc::new(Fake {
        needs_token: true,
        ..Default::default()
    });
    Dispatcher::new(r.store.clone(), r.clock.clone(), tokened.clone())
        .tick()
        .await
        .unwrap();
    assert_eq!(r.state(decided, &with_token).await, "dropped");
    assert_eq!(r.state(old, &with_token).await, "dropped");
    assert_eq!(r.state(fresh, &with_token).await, "sent");
    assert_eq!(r.state(fresh, &no_token).await, "dropped");
    assert_eq!(r.state(late, &revoked).await, "dropped");
    assert_eq!(r.state(late, &with_token).await, "sent");
    let reasons = r.store.delivery(fresh, &no_token).await.unwrap().unwrap().2;
    assert_eq!(reasons.as_deref(), Some("no push token"));
}

#[tokio::test]
async fn an_unregistered_token_is_forgotten() {
    let r = Rig::new(Store::open_in_memory().await.unwrap()).await;
    let dev = r.device(Some("aa11")).await;
    r.session("s1").await;
    r.approval("s1", "a1").await;
    r.fake
        .script
        .lock()
        .unwrap()
        .push_back(SendOutcome::Unregistered("APNs 410".into()));
    r.dispatcher().tick().await.unwrap();
    assert_eq!(
        r.store.device(&dev).await.unwrap().unwrap().push_token,
        None
    );
    assert_eq!(r.state(r.last_outbox().await, &dev).await, "failed");
}

fn key() -> SigningKey {
    loop {
        let mut b = [0u8; 32];
        getrandom::fill(&mut b).unwrap();
        if let Ok(k) = SigningKey::from_slice(&b) {
            return k;
        }
    }
}

fn apns(key: SigningKey) -> Apns {
    Apns::with_key(
        key,
        "ABCDE12345",
        "TEAM123456",
        "dev.example.switchboard",
        Hosts::default(),
    )
    .unwrap()
}

#[test]
fn the_provider_token_is_an_es256_jwt_signed_with_the_p8_key() {
    use base64::Engine;
    use base64::engine::general_purpose::URL_SAFE_NO_PAD as B64;
    let k = key();
    let verifying = *k.verifying_key();
    let a = apns(k);
    let jwt = a.make_jwt(1_700_000_000);
    let parts: Vec<&str> = jwt.split('.').collect();
    assert_eq!(parts.len(), 3);
    let header: serde_json::Value = serde_json::from_slice(&B64.decode(parts[0]).unwrap()).unwrap();
    let claims: serde_json::Value = serde_json::from_slice(&B64.decode(parts[1]).unwrap()).unwrap();
    assert_eq!(header, json!({"alg": "ES256", "kid": "ABCDE12345"}));
    assert_eq!(claims, json!({"iss": "TEAM123456", "iat": 1_700_000_000}));
    let sig = Signature::from_slice(&B64.decode(parts[2]).unwrap()).unwrap();
    verifying
        .verify(format!("{}.{}", parts[0], parts[1]).as_bytes(), &sig)
        .unwrap();
}

#[test]
fn the_p8_key_file_is_read_as_pkcs8_pem() {
    use p256::pkcs8::{EncodePrivateKey, LineEnding};
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("AuthKey_ABCDE12345.p8");
    std::fs::write(
        &path,
        key().to_pkcs8_pem(LineEnding::LF).unwrap().as_bytes(),
    )
    .unwrap();
    let cfg = crate::config::ApnsConfig {
        key_file: path.clone(),
        key_id: "ABCDE12345".into(),
        team_id: "TEAM123456".into(),
    };
    Apns::new(&cfg, "dev.example.switchboard").unwrap();
    std::fs::write(&path, "not a key").unwrap();
    let err = Apns::new(&cfg, "dev.example.switchboard")
        .err()
        .unwrap()
        .to_string();
    assert!(
        err.contains("not a P-256 PKCS#8 key") && !err.contains("not a key\""),
        "{err}"
    );
}

#[test]
fn requests_go_to_the_devices_environment_with_apns_headers() {
    let a = apns(key());
    let mut push = Push {
        device_id: "d1".into(),
        token: Some("aa11".into()),
        kind: OutboxKind::Approval,
        session_id: None,
        approval_id: None,
        env: PushEnv::Sandbox,
        payload: json!({"aps": {}}),
        collapse_id: "a1".into(),
        expires_at: 42,
    };
    let req = a.request(&push, "jwt").unwrap();
    assert_eq!(
        req.uri().to_string(),
        "https://api.sandbox.push.apple.com/3/device/aa11"
    );
    let h = |k: &str| req.headers()[k].to_str().unwrap().to_owned();
    assert_eq!(h("authorization"), "bearer jwt");
    assert_eq!(h("apns-topic"), "dev.example.switchboard");
    assert_eq!(
        (h("apns-push-type"), h("apns-priority")),
        ("alert".into(), "10".into())
    );
    assert_eq!(
        (h("apns-collapse-id"), h("apns-expiration")),
        ("a1".into(), "42".into())
    );
    push.env = PushEnv::Production;
    assert_eq!(
        a.request(&push, "jwt").unwrap().uri().host(),
        Some("api.push.apple.com")
    );
    push.token = None;
    assert!(a.request(&push, "jwt").is_err());
}

#[test]
fn apns_answers_are_classified() {
    let body = |r: &str| json!({ "reason": r }).to_string().into_bytes();
    assert_eq!(classify(200, b""), (SendOutcome::Sent, false));
    assert!(matches!(
        classify(410, &body("Unregistered")).0,
        SendOutcome::Unregistered(_)
    ));
    assert!(matches!(
        classify(400, &body("BadDeviceToken")).0,
        SendOutcome::Unregistered(_)
    ));
    assert!(matches!(
        classify(403, &body("ExpiredProviderToken")),
        (SendOutcome::Retry(_), true)
    ));
    assert!(matches!(
        classify(429, &body("TooManyRequests")).0,
        SendOutcome::Retry(_)
    ));
    assert!(matches!(classify(503, b"").0, SendOutcome::Retry(_)));
    assert!(matches!(
        classify(400, &body("PayloadTooLarge")).0,
        SendOutcome::Failed(_)
    ));
}

#[test]
fn simctl_reads_the_payload_from_stdin() {
    let s = simctl::Simctl::new("booted", "dev.example.switchboard");
    assert_eq!(
        s.args(),
        ["simctl", "push", "booted", "dev.example.switchboard", "-"]
    );
}

/// Reaches Apple's sandbox over TLS and HTTP/2 with a made-up key; Apple refuses the token.
/// Run with `SWITCHBOARD_LIVE_APNS=1 cargo test --lib apns_sandbox_is_reachable`.
#[tokio::test]
async fn apns_sandbox_is_reachable() {
    if std::env::var("SWITCHBOARD_LIVE_APNS").is_err() {
        return;
    }
    let a = apns(key());
    let push = Push {
        device_id: "d1".into(),
        token: Some("00".repeat(32)),
        kind: OutboxKind::Approval,
        session_id: None,
        approval_id: None,
        env: PushEnv::Sandbox,
        payload: json!({"aps": {"alert": "test"}}),
        collapse_id: "t".into(),
        expires_at: 0,
    };
    let outcome = a.send(&push).await;
    // A refused provider token proves the TLS and HTTP/2 path; 403 is retried.
    assert!(
        matches!(&outcome, SendOutcome::Retry(e) if e.contains("403")),
        "{outcome:?}"
    );
}

/// Checks a real APNs key against Apple's sandbox with a made-up device token: Apple checks the
/// provider token first, so `BadDeviceToken` means the key, key id and team are accepted.
/// Run with `SWITCHBOARD_LIVE_APNS_CONFIG=<hub.toml> cargo test --lib apns_key_is_accepted`.
#[tokio::test]
async fn apns_key_is_accepted() {
    let Ok(path) = std::env::var("SWITCHBOARD_LIVE_APNS_CONFIG") else {
        return;
    };
    let text = std::fs::read_to_string(path).unwrap();
    let push: crate::config::PushConfig = toml::from_str::<toml::Table>(&text)
        .unwrap()
        .get("push")
        .expect("a [push] section")
        .clone()
        .try_into()
        .unwrap();
    let a = Apns::new(push.apns.as_ref().expect("[push.apns]"), &push.bundle_id).unwrap();
    for env in [PushEnv::Sandbox, PushEnv::Production] {
        let outcome = a
            .send(&Push {
                device_id: "d1".into(),
                token: Some("00".repeat(32)),
                kind: OutboxKind::Approval,
                session_id: None,
                approval_id: None,
                env,
                payload: json!({"aps": {"alert": "test"}}),
                collapse_id: "t".into(),
                expires_at: 0,
            })
            .await;
        println!("{env:?}: {outcome:?}");
        assert!(
            matches!(&outcome, SendOutcome::Unregistered(e) if e.contains("BadDeviceToken")),
            "{env:?}: Apple refused the provider token: {outcome:?}"
        );
    }
}
