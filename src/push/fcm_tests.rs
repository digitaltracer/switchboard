use std::collections::VecDeque;

use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::routing::post;
use ring::signature::{RSA_PKCS1_2048_8192_SHA256, UnparsedPublicKey};
use serde_json::json;
use tokio::net::TcpListener;

use super::*;
use crate::store::{OutboxKind, PushEnv, PushProvider};

/// A throwaway RSA key made for the test, so no key file is ever committed. `None` without
/// `openssl`.
pub(crate) fn test_key() -> Option<String> {
    let out = std::process::Command::new("openssl")
        .args([
            "genpkey",
            "-algorithm",
            "RSA",
            "-pkeyopt",
            "rsa_keygen_bits:2048",
        ])
        .output()
        .ok()?;
    out.status
        .success()
        .then(|| String::from_utf8(out.stdout).unwrap())
}

#[derive(Default)]
pub(crate) struct Mock {
    pub assertions: Mutex<Vec<String>>,
    pub sends: Mutex<Vec<(String, Value)>>,
    /// Answers to sends, then 200.
    pub script: Mutex<VecDeque<(u16, Value)>>,
}

async fn token(State(m): State<Arc<Mock>>, body: String) -> (StatusCode, axum::Json<Value>) {
    let assertion = body
        .split('&')
        .find_map(|kv| kv.strip_prefix("assertion="))
        .unwrap_or_default()
        .to_owned();
    let n = {
        let mut a = m.assertions.lock().unwrap();
        a.push(assertion);
        a.len()
    };
    (
        StatusCode::OK,
        axum::Json(json!({ "access_token": format!("at{n}"), "expires_in": 3600 })),
    )
}

async fn send(
    State(m): State<Arc<Mock>>,
    headers: HeaderMap,
    axum::Json(body): axum::Json<Value>,
) -> (StatusCode, axum::Json<Value>) {
    let auth = headers["authorization"].to_str().unwrap().to_owned();
    m.sends.lock().unwrap().push((auth, body));
    let (status, reply) = m
        .script
        .lock()
        .unwrap()
        .pop_front()
        .unwrap_or((200, json!({ "name": "projects/p1/messages/1" })));
    (StatusCode::from_u16(status).unwrap(), axum::Json(reply))
}

/// An FCM sender pointed at a local stand-in for Google's token and send endpoints.
pub(crate) async fn mock(pem: &str) -> (Fcm, Arc<Mock>) {
    let m = Arc::new(Mock::default());
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let app = axum::Router::new()
        .route("/token", post(token))
        .route("/v1/projects/p1/messages:send", post(send))
        .with_state(m.clone());
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    let key = json!({
        "type": "service_account", "project_id": "p1", "client_email": "sb@p1.iam.example",
        "private_key": pem, "token_uri": format!("http://{addr}/token"),
    });
    let fcm = Fcm::from_json(&key.to_string(), &format!("http://{addr}")).unwrap();
    (fcm, m)
}

pub(crate) fn android(kind: OutboxKind) -> Push {
    Push {
        device_id: "d1".into(),
        kind,
        session_id: None,
        approval_id: None,
        attention_id: Some("att1".into()),
        token: Some("fcm-token:APA91b_x".into()),
        env: PushEnv::Production,
        provider: PushProvider::Fcm,
        payload: json!({}),
        collapse_id: "waiting:att1".into(),
        expires_at: 4_000_000_000,
    }
}

fn verify(pem: &str, jwt: &str) -> Value {
    let der: String = pem.lines().filter(|l| !l.starts_with("-----")).collect();
    let key = RsaKeyPair::from_pkcs8(&STANDARD.decode(der).unwrap()).unwrap();
    let (input, sig) = jwt.rsplit_once('.').unwrap();
    UnparsedPublicKey::new(&RSA_PKCS1_2048_8192_SHA256, key.public().as_ref())
        .verify(input.as_bytes(), &B64.decode(sig).unwrap())
        .expect("the assertion is signed with the service-account key");
    let (header, claims) = input.split_once('.').unwrap();
    let header: Value = serde_json::from_slice(&B64.decode(header).unwrap()).unwrap();
    assert_eq!(header["alg"], "RS256");
    serde_json::from_slice(&B64.decode(claims).unwrap()).unwrap()
}

#[tokio::test]
async fn a_push_is_sent_with_a_signed_token_and_generic_text_only() {
    let Some(pem) = test_key() else {
        eprintln!("skipped: no openssl to make a test key");
        return;
    };
    let (fcm, m) = mock(&pem).await;
    assert_eq!(
        fcm.send(&android(OutboxKind::Waiting)).await,
        SendOutcome::Sent
    );
    assert_eq!(
        fcm.send(&android(OutboxKind::Approval)).await,
        SendOutcome::Sent
    );
    let assertions = m.assertions.lock().unwrap().clone();
    assert_eq!(assertions.len(), 1, "the access token is reused");
    let claims = verify(&pem, &assertions[0]);
    assert_eq!(claims["iss"], "sb@p1.iam.example");
    assert_eq!(claims["scope"], SCOPE);
    assert!(claims["aud"].as_str().unwrap().ends_with("/token"));

    let sends = m.sends.lock().unwrap().clone();
    let (auth, body) = &sends[0];
    assert_eq!(auth, "Bearer at1");
    let msg = &body["message"];
    assert_eq!(msg["token"], "fcm-token:APA91b_x");
    assert_eq!(
        msg["notification"],
        json!({ "title": "Agent waiting", "body": "An agent is waiting for your answer." })
    );
    assert_eq!(
        msg["data"],
        json!({ "kind": "waiting", "attention_id": "att1" })
    );
    assert_eq!(msg["android"]["collapse_key"], "waiting:att1");
    assert_eq!(msg["android"]["priority"], "high");
    assert!(msg["android"]["ttl"].as_str().unwrap().ends_with('s'));
}

#[tokio::test]
async fn a_refused_access_token_is_renewed_once_and_a_gone_device_unregistered() {
    let Some(pem) = test_key() else {
        eprintln!("skipped: no openssl to make a test key");
        return;
    };
    let (fcm, m) = mock(&pem).await;
    m.script.lock().unwrap().extend([
        (401, json!({ "error": { "status": "UNAUTHENTICATED" } })),
        (200, json!({})),
        (
            404,
            json!({ "error": { "status": "NOT_FOUND",
            "details": [{ "errorCode": "UNREGISTERED" }] } }),
        ),
        (503, json!({})),
        (400, json!({ "error": { "status": "INVALID_ARGUMENT" } })),
    ]);
    let push = android(OutboxKind::Finished);
    assert_eq!(fcm.send(&push).await, SendOutcome::Sent);
    assert_eq!(m.assertions.lock().unwrap().len(), 2);
    assert!(matches!(
        fcm.send(&push).await,
        SendOutcome::Unregistered(_)
    ));
    assert!(matches!(fcm.send(&push).await, SendOutcome::Retry(_)));
    assert!(matches!(fcm.send(&push).await, SendOutcome::Failed(_)));
    let no_token = Push {
        token: None,
        ..push
    };
    assert!(matches!(fcm.send(&no_token).await, SendOutcome::Failed(_)));
}

#[test]
fn tokens_and_keys_are_checked() {
    assert!(valid_token("dXk1:APA91bH-x_y.z"));
    for bad in ["", "has space", "new\nline", &"a".repeat(MAX_TOKEN + 1)] {
        assert!(!valid_token(bad), "{bad:?}");
    }
    assert!(Fcm::from_json("{}", SEND_BASE).is_err());
    let not_rsa = json!({ "project_id": "p", "client_email": "e", "token_uri": "https://t",
        "private_key": "-----BEGIN PRIVATE KEY-----\nAAAA\n-----END PRIVATE KEY-----\n" });
    let err = Fcm::from_json(&not_rsa.to_string(), SEND_BASE)
        .err()
        .unwrap()
        .to_string();
    assert!(err.contains("not an RSA") && !err.contains("AAAA"), "{err}");
}

#[test]
fn a_key_readable_by_others_or_with_a_plain_token_uri_is_refused() {
    use std::os::unix::fs::PermissionsExt;
    let Some(pem) = test_key() else {
        return;
    };
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("fcm.json");
    let key = |uri: &str| {
        json!({ "project_id": "p1", "client_email": "e", "private_key": pem, "token_uri": uri })
            .to_string()
    };
    std::fs::write(&path, key("https://oauth2.googleapis.com/token")).unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();
    let cfg = FcmConfig {
        key_file: path.clone(),
    };
    let err = Fcm::new(&cfg).err().unwrap().to_string();
    assert!(err.contains("chmod 600"), "{err}");
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
    let _ = rustls::crypto::ring::default_provider().install_default();
    assert!(Fcm::new(&cfg).is_ok());
    std::fs::write(&path, key("http://oauth2.googleapis.com/token")).unwrap();
    assert!(Fcm::new(&cfg).err().unwrap().to_string().contains("https"));
}
