//! Firebase Cloud Messaging HTTP v1 for Android devices. A service-account key signs an RS256
//! JWT that is exchanged for an access token, cached until shortly before it expires. The key is
//! read once at start, must be mode 0600 outside any repo, and is never logged.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use base64::Engine;
use base64::engine::general_purpose::{STANDARD, URL_SAFE_NO_PAD as B64};
use bytes::Bytes;
use http_body_util::{BodyExt, Full};
use hyper::Request;
use hyper_rustls::HttpsConnector;
use hyper_util::client::legacy::Client;
use hyper_util::client::legacy::connect::HttpConnector;
use hyper_util::rt::TokioExecutor;
use ring::signature::{RSA_PKCS1_SHA256, RsaKeyPair};
use serde::Deserialize;
use serde_json::{Map, Value, json};

use super::{Push, SendOutcome, Sender, text};
use crate::config::FcmConfig;

pub const SEND_BASE: &str = "https://fcm.googleapis.com";
const SCOPE: &str = "https://www.googleapis.com/auth/firebase.messaging";
const TOKEN_LIFETIME: i64 = 3600;
/// An access token is renewed this long before Google says it expires.
const RENEW_EARLY: i64 = 300;
const TIMEOUT: Duration = Duration::from_secs(30);
/// FCM registration tokens are opaque; this bounds and shapes them.
pub const MAX_TOKEN: usize = 4096;

/// Whether `token` looks like an FCM registration token.
pub fn valid_token(token: &str) -> bool {
    !token.is_empty()
        && token.len() <= MAX_TOKEN
        && token
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"-_:.".contains(&b))
}

#[derive(Deserialize)]
struct ServiceAccount {
    project_id: String,
    client_email: String,
    private_key: String,
    token_uri: String,
}

pub struct Fcm {
    client: Client<HttpsConnector<HttpConnector>, Full<Bytes>>,
    key: RsaKeyPair,
    email: String,
    token_uri: String,
    send_url: String,
    access: Mutex<Option<(String, i64)>>,
}

impl Fcm {
    pub fn new(cfg: &FcmConfig) -> anyhow::Result<Self> {
        use std::os::unix::fs::PermissionsExt;
        let path = &cfg.key_file;
        let mode = std::fs::metadata(path)
            .map_err(|e| anyhow::anyhow!("reading the FCM key {}: {e}", path.display()))?
            .permissions()
            .mode();
        if mode & 0o077 != 0 {
            anyhow::bail!(
                "the FCM key {} is readable by others; chmod 600 it",
                path.display()
            );
        }
        let text = std::fs::read_to_string(path)
            .map_err(|e| anyhow::anyhow!("reading the FCM key {}: {e}", path.display()))?;
        let fcm = Self::from_json(&text, SEND_BASE)
            .map_err(|e| anyhow::anyhow!("{}: {e}", path.display()))?;
        if !fcm.token_uri.starts_with("https://") {
            anyhow::bail!("{}: token_uri must be https", path.display());
        }
        Ok(fcm)
    }

    /// From a service-account key's JSON; `send_base` is where messages go.
    pub fn from_json(json_text: &str, send_base: &str) -> anyhow::Result<Self> {
        let sa: ServiceAccount = serde_json::from_str(json_text)
            .map_err(|_| anyhow::anyhow!("not a service-account key"))?;
        let der: String = sa
            .private_key
            .lines()
            .filter(|l| !l.starts_with("-----"))
            .collect();
        let der = STANDARD
            .decode(der.trim())
            .map_err(|_| anyhow::anyhow!("the private key is not PEM"))?;
        let key = RsaKeyPair::from_pkcs8(&der)
            .map_err(|_| anyhow::anyhow!("the private key is not an RSA PKCS#8 key"))?;
        let https = hyper_rustls::HttpsConnectorBuilder::new()
            .with_provider_and_webpki_roots(Arc::new(rustls::crypto::ring::default_provider()))?
            .https_or_http()
            .enable_http1()
            .enable_http2()
            .build();
        Ok(Self {
            client: Client::builder(TokioExecutor::new()).build(https),
            key,
            email: sa.client_email,
            token_uri: sa.token_uri,
            send_url: format!("{send_base}/v1/projects/{}/messages:send", sa.project_id),
            access: Mutex::new(None),
        })
    }

    /// The assertion exchanged for an access token.
    pub fn make_jwt(&self, now: i64) -> anyhow::Result<String> {
        let header = B64.encode(json!({ "alg": "RS256", "typ": "JWT" }).to_string());
        let claims = B64.encode(
            json!({ "iss": self.email, "scope": SCOPE, "aud": self.token_uri,
                    "iat": now, "exp": now + TOKEN_LIFETIME })
            .to_string(),
        );
        let input = format!("{header}.{claims}");
        let mut sig = vec![0; self.key.public().modulus_len()];
        self.key
            .sign(
                &RSA_PKCS1_SHA256,
                &ring::rand::SystemRandom::new(),
                input.as_bytes(),
                &mut sig,
            )
            .map_err(|_| anyhow::anyhow!("signing failed"))?;
        Ok(format!("{input}.{}", B64.encode(sig)))
    }

    /// The message: the kind's generic text plus ids, as data strings.
    pub fn message(push: &Push) -> anyhow::Result<Value> {
        let token = push
            .token
            .as_deref()
            .ok_or_else(|| anyhow::anyhow!("no device token"))?;
        let (title, body) = text(push.kind);
        let mut data = Map::new();
        data.insert("kind".into(), push.kind.as_str().into());
        for (k, v) in [
            ("session_id", &push.session_id),
            ("approval_id", &push.approval_id),
            ("attention_id", &push.attention_id),
        ] {
            if let Some(v) = v {
                data.insert(k.into(), v.clone().into());
            }
        }
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_secs() as i64);
        let ttl = (push.expires_at - now).max(0);
        Ok(json!({ "message": {
            "token": token,
            "notification": { "title": title, "body": body },
            "data": data,
            "android": { "priority": "high", "collapse_key": push.collapse_id, "ttl": format!("{ttl}s") },
        }}))
    }

    async fn post(&self, req: Request<Full<Bytes>>) -> Result<(u16, Bytes), String> {
        let resp = match tokio::time::timeout(TIMEOUT, self.client.request(req)).await {
            Ok(Ok(r)) => r,
            Ok(Err(e)) => return Err(format!("FCM connection: {e}")),
            Err(_) => return Err("FCM timed out".into()),
        };
        let status = resp.status().as_u16();
        let body = resp
            .into_body()
            .collect()
            .await
            .map(|b| b.to_bytes())
            .unwrap_or_default();
        Ok((status, body))
    }

    async fn access_token(&self, now: i64, renew: bool) -> Result<String, SendOutcome> {
        if let Some((t, until)) = &*self.access.lock().unwrap()
            && !renew
            && now < *until
        {
            return Ok(t.clone());
        }
        let jwt = self
            .make_jwt(now)
            .map_err(|e| SendOutcome::Failed(e.to_string()))?;
        let form = format!(
            "grant_type=urn%3Aietf%3Aparams%3Aoauth%3Agrant-type%3Ajwt-bearer&assertion={jwt}"
        );
        let req = Request::post(&self.token_uri)
            .header("content-type", "application/x-www-form-urlencoded")
            .body(Full::new(Bytes::from(form)))
            .map_err(|e| SendOutcome::Failed(e.to_string()))?;
        let (status, body) = self.post(req).await.map_err(SendOutcome::Retry)?;
        let v: Value = serde_json::from_slice(&body).unwrap_or_default();
        match (status, v["access_token"].as_str()) {
            (200, Some(t)) => {
                let life = v["expires_in"].as_i64().unwrap_or(TOKEN_LIFETIME);
                *self.access.lock().unwrap() = Some((t.to_owned(), now + life - RENEW_EARLY));
                Ok(t.to_owned())
            }
            (429 | 500..=599, _) => Err(SendOutcome::Retry(format!("FCM token {status}"))),
            _ => Err(SendOutcome::Failed(format!("FCM token {status}"))),
        }
    }
}

/// What an FCM answer means for the delivery, and whether the access token must be renewed.
pub fn classify(status: u16, body: &[u8]) -> (SendOutcome, bool) {
    let v: Value = serde_json::from_slice(body).unwrap_or_default();
    let err = &v["error"];
    let code = err["details"]
        .as_array()
        .and_then(|d| d.iter().find_map(|x| x["errorCode"].as_str()))
        .or_else(|| err["status"].as_str())
        .unwrap_or_default()
        .to_owned();
    let what = format!("FCM {status} {code}").trim().to_owned();
    match (status, code.as_str()) {
        (200, _) => (SendOutcome::Sent, false),
        // INVALID_ARGUMENT may be the message, not the token, so the token is kept.
        (404, _) | (_, "UNREGISTERED") => (SendOutcome::Unregistered(what), false),
        (401, _) => (SendOutcome::Retry(what), true),
        (429, _) | (500..=599, _) => (SendOutcome::Retry(what), false),
        _ => (SendOutcome::Failed(what), false),
    }
}

impl Sender for Fcm {
    fn needs_token(&self) -> bool {
        true
    }

    async fn send(&self, push: &Push) -> SendOutcome {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_secs() as i64);
        let message = match Self::message(push) {
            Ok(m) => m.to_string(),
            Err(e) => return SendOutcome::Failed(e.to_string()),
        };
        for renew in [false, true] {
            let token = match self.access_token(now, renew).await {
                Ok(t) => t,
                Err(outcome) => return outcome,
            };
            let req = match Request::post(&self.send_url)
                .header("authorization", format!("Bearer {token}"))
                .header("content-type", "application/json")
                .body(Full::new(Bytes::from(message.clone())))
            {
                Ok(r) => r,
                Err(e) => return SendOutcome::Failed(e.to_string()),
            };
            let (status, body) = match self.post(req).await {
                Ok(r) => r,
                Err(e) => return SendOutcome::Retry(e),
            };
            let (outcome, renew_token) = classify(status, &body);
            if !(renew_token && !renew) {
                return outcome;
            }
        }
        unreachable!("the second pass always returns")
    }
}

#[cfg(test)]
#[path = "fcm_tests.rs"]
pub(crate) mod tests;
