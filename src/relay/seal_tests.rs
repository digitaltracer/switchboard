use super::*;

fn request() -> PlainRequest {
    PlainRequest {
        ts: 1_700_000_000_000,
        method: "GET".into(),
        path: "/app/v1/sessions?after_revision=3".into(),
        headers: vec![("authorization".into(), "Bearer t".into())],
        body: String::new(),
    }
}

#[test]
fn a_request_opens_only_at_its_hub_and_the_reply_only_at_its_phone() {
    let hub = HubKeys::generate();
    let (env, pending) = phone::seal(&hub.kx_public(), &request());
    let (got, reply) = hub.open_request(&env).unwrap();
    assert_eq!(got, request());
    let resp = PlainResponse::new(
        200,
        vec![("content-type".into(), "application/json".into())],
        b"{}",
    );
    let sealed = reply.seal(&resp);
    assert_eq!(pending.open(&sealed).unwrap(), resp);

    let other = HubKeys::generate();
    assert_eq!(other.open_request(&env).err(), Some(SealError::Decrypt));
    let (_, someone_else) = phone::seal(&hub.kx_public(), &request());
    assert_eq!(someone_else.open(&sealed).err(), Some(SealError::Decrypt));
}

#[test]
fn two_answers_to_one_request_never_share_a_keystream() {
    let hub = HubKeys::generate();
    let (env, pending) = phone::seal(&hub.kx_public(), &request());
    let resp = PlainResponse::new(200, vec![], b"same answer");
    let (_, first) = hub.open_request(&env).unwrap();
    let (_, second) = hub.open_request(&env).unwrap();
    let (a, b) = (first.seal(&resp), second.seal(&resp));
    assert_ne!(a.n, b.n);
    assert_ne!(a.ct, b.ct);
    assert_eq!(pending.open(&a).unwrap(), pending.open(&b).unwrap());
}

#[test]
fn tampering_and_unknown_versions_are_refused() {
    let hub = HubKeys::generate();
    let (mut env, _) = phone::seal(&hub.kx_public(), &request());
    let mut ct = B64.decode(&env.ct).unwrap();
    ct[0] ^= 1;
    env.ct = B64.encode(ct);
    assert_eq!(hub.open_request(&env).err(), Some(SealError::Decrypt));
    env.v = 1;
    assert_eq!(hub.open_request(&env).err(), Some(SealError::Version(1)));
    env.v = VERSION;
    env.epk = "AAAA".into();
    assert!(matches!(
        hub.open_request(&env),
        Err(SealError::Malformed(_))
    ));
}

#[test]
fn replays_and_stale_requests_are_refused() {
    let g = ReplayGuard::default();
    let now = 1_700_000_000_000;
    g.check("k1", now, now).unwrap();
    assert_eq!(g.check("k1", now, now + 1), Err(SealError::Replay));
    assert_eq!(
        g.check("k2", now - MAX_SKEW_MS - 1, now),
        Err(SealError::Stale)
    );
    assert_eq!(
        g.check("k3", now + MAX_SKEW_MS + 1, now),
        Err(SealError::Stale)
    );
    // Once a key is outside any clock window it is forgotten; its timestamp would be refused anyway.
    g.check("k4", now, now).unwrap();
    assert_eq!(g.seen.lock().unwrap().len(), 2);
    g.check("k5", now + 3 * MAX_SKEW_MS, now + 3 * MAX_SKEW_MS)
        .unwrap();
    assert_eq!(g.seen.lock().unwrap().len(), 1);
}

#[test]
fn the_key_file_is_private_reused_and_gives_a_stable_relay_id() {
    use std::os::unix::fs::PermissionsExt;
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("relay-key.json");
    let first = HubKeys::load_or_create(&path).unwrap();
    assert_eq!(
        std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
        0o600
    );
    let again = HubKeys::load_or_create(&path).unwrap();
    assert_eq!(first.relay_id(), again.relay_id());
    assert_eq!(first.relay_id().len(), 32);
    assert_eq!(
        first.relay_id(),
        relay_id(&first.kx_public(), &first.sign_public())
    );
    std::fs::write(&path, "junk").unwrap();
    assert!(
        HubKeys::load_or_create(&path).is_err(),
        "a bad file is never replaced"
    );
}

/// The app's CryptoKit code sealed `swift_request`; this side opens it and seals the reply the
/// app's tests open. Both halves are deterministic, so the fixture pins the wire format.
#[test]
fn the_apps_cryptokit_envelopes_interoperate() {
    let path = "tests/fixtures/app-v1/relay-vector.json";
    let mut v: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
    let hub = HubKeys::from_secrets(
        &[0x33; 32],
        &hex::decode(v["hub_secret_hex"].as_str().unwrap()).unwrap(),
    );
    assert_eq!(
        B64.encode(hub.kx_public()),
        v["hub_kx_public"].as_str().unwrap()
    );
    let env: RequestEnvelope = serde_json::from_value(v["swift_request"].clone()).unwrap();
    let (req, reply) = hub.open_request(&env).unwrap();
    assert_eq!(
        (req.method.as_str(), req.path.as_str(), req.ts),
        ("GET", "/app/v1/meta", 42)
    );
    assert_eq!(
        req.headers,
        [("x-switchboard-build".to_owned(), "1".to_owned())]
    );
    let answer = PlainResponse::new(
        200,
        vec![("content-type".into(), "application/json".into())],
        br#"{"ok":true}"#,
    );
    let sealed = serde_json::to_value(reply.seal_with(&answer, [0x44; 12])).unwrap();
    let (sign, id) = (B64.encode(hub.sign_public()), hub.relay_id());
    if std::env::var("SWITCHBOARD_WRITE_RELAY_VECTOR").is_ok() {
        v["rust_response"] = sealed;
        v["sign_public"] = sign.into();
        v["relay_id"] = id.into();
        std::fs::write(path, serde_json::to_string_pretty(&v).unwrap() + "\n").unwrap();
        return;
    }
    assert_eq!(
        sealed, v["rust_response"],
        "the reply is deterministic for fixed keys"
    );
    assert_eq!(
        (v["sign_public"].as_str(), v["relay_id"].as_str()),
        (Some(sign.as_str()), Some(id.as_str()))
    );
}
