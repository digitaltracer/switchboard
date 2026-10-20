//! Apple Push Notification service with a token-based key: an ES256 JWT from the `.p8` key,
//! refreshed every 40 minutes, sent over HTTP/2. The key is read once at start and never logged.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD as B64;
use bytes::Bytes;
use http_body_util::{BodyExt, Full};
use hyper::Request;
use hyper_rustls::HttpsConnector;
use hyper_util::client::legacy::Client;
use hyper_util::client::legacy::connect::HttpConnector;
use hyper_util::rt::TokioExecutor;
use p256::ecdsa::signature::Signer;
use p256::ecdsa::{Signature, SigningKey};
use p256::pkcs8::DecodePrivateKey;
use serde_json::json;

use super::{Push, SendOutcome, Sender};
use crate::config::ApnsConfig;
use crate::store::PushEnv;

pub const PRODUCTION: &str = "api.push.apple.com";
pub const SANDBOX: &str = "api.sandbox.push.apple.com";
/// Apple wants a new provider token at least hourly and at most every 20 minutes.
const JWT_LIFETIME: i64 = 40 * 60;
const TIMEOUT: Duration = Duration::from_secs(30);

pub struct Apns {
    client: Client<HttpsConnector<HttpConnector>, Full<Bytes>>,
    key: SigningKey,
    key_id: String,
    team_id: String,
    topic: String,
    jwt: Mutex<Option<(String, i64)>>,
    hosts: Hosts,
}

/// Where each environment's requests go; tests point both elsewhere.
#[derive(Clone)]
pub struct Hosts {
    pub production: String,
    pub sandbox: String,
}

impl Default for Hosts {
    fn default() -> Self {
        Self {
            production: PRODUCTION.into(),
            sandbox: SANDBOX.into(),
        }
    }
}

impl Apns {
    pub fn new(cfg: &ApnsConfig, topic: &str) -> anyhow::Result<Self> {
        let pem = std::fs::read_to_string(&cfg.key_file)
            .map_err(|e| anyhow::anyhow!("reading the APNs key {}: {e}", cfg.key_file.display()))?;
        let key = SigningKey::from_pkcs8_pem(&pem)
            .map_err(|_| anyhow::anyhow!("{} is not a P-256 PKCS#8 key", cfg.key_file.display()))?;
        Self::with_key(key, &cfg.key_id, &cfg.team_id, topic, Hosts::default())
    }

    pub fn with_key(
        key: SigningKey,
        key_id: &str,
        team_id: &str,
        topic: &str,
        hosts: Hosts,
    ) -> anyhow::Result<Self> {
        let https = hyper_rustls::HttpsConnectorBuilder::new()
            .with_provider_and_webpki_roots(Arc::new(rustls::crypto::ring::default_provider()))?
            .https_only()
            .enable_http2()
            .build();
        let client = Client::builder(TokioExecutor::new())
            .http2_only(true)
            .build(https);
        Ok(Self {
            client,
            key,
            key_id: key_id.to_owned(),
            team_id: team_id.to_owned(),
            topic: topic.to_owned(),
            jwt: Mutex::new(None),
            hosts,
        })
    }

    /// The provider token: `{"alg":"ES256","kid"}.{"iss","iat"}`, signed with the `.p8` key.
    pub fn make_jwt(&self, now_secs: i64) -> String {
        let header = B64.encode(json!({ "alg": "ES256", "kid": self.key_id }).to_string());
        let claims = B64.encode(json!({ "iss": self.team_id, "iat": now_secs }).to_string());
        let signing_input = format!("{header}.{claims}");
        let sig: Signature = self.key.sign(signing_input.as_bytes());
        format!("{signing_input}.{}", B64.encode(sig.to_bytes()))
    }

    fn jwt(&self, now_secs: i64, refresh: bool) -> String {
        let mut cached = self.jwt.lock().unwrap();
        match &*cached {
            Some((t, iat)) if !refresh && now_secs - iat < JWT_LIFETIME => t.clone(),
            _ => {
                let t = self.make_jwt(now_secs);
                *cached = Some((t.clone(), now_secs));
                t
            }
        }
    }

    pub fn request(&self, push: &Push, jwt: &str) -> anyhow::Result<Request<Full<Bytes>>> {
        let token = push
            .token
            .as_deref()
            .ok_or_else(|| anyhow::anyhow!("no device token"))?;
        let host = match push.env {
            PushEnv::Sandbox => &self.hosts.sandbox,
            PushEnv::Production => &self.hosts.production,
        };
        Ok(Request::post(format!("https://{host}/3/device/{token}"))
            .header("authorization", format!("bearer {jwt}"))
            .header("apns-topic", &self.topic)
            .header("apns-push-type", "alert")
            .header("apns-priority", "10")
            .header("apns-expiration", push.expires_at.to_string())
            .header("apns-collapse-id", &push.collapse_id)
            .header("content-type", "application/json")
            .body(Full::new(Bytes::from(push.payload.to_string())))?)
    }
}

/// What an APNs answer means for the delivery, and whether the provider token must be renewed.
pub fn classify(status: u16, body: &[u8]) -> (SendOutcome, bool) {
    let reason = serde_json::from_slice::<serde_json::Value>(body)
        .ok()
        .and_then(|v| v.get("reason").and_then(|r| r.as_str()).map(str::to_owned))
        .unwrap_or_default();
    let what = format!("APNs {status} {reason}").trim().to_owned();
    match (status, reason.as_str()) {
        (200, _) => (SendOutcome::Sent, false),
        (410, _) | (400, "BadDeviceToken" | "DeviceTokenNotForTopic") => {
            (SendOutcome::Unregistered(what), false)
        }
        (403, "ExpiredProviderToken") => (SendOutcome::Retry(what), true),
        (403, _) | (429, _) | (500..=599, _) => (SendOutcome::Retry(what), false),
        _ => (SendOutcome::Failed(what), false),
    }
}

impl Sender for Apns {
    fn needs_token(&self) -> bool {
        true
    }

    async fn send(&self, push: &Push) -> SendOutcome {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_secs() as i64);
        for refresh in [false, true] {
            let req = match self.request(push, &self.jwt(now, refresh)) {
                Ok(r) => r,
                Err(e) => return SendOutcome::Failed(e.to_string()),
            };
            let resp = match tokio::time::timeout(TIMEOUT, self.client.request(req)).await {
                Ok(Ok(r)) => r,
                Ok(Err(e)) => return SendOutcome::Retry(format!("APNs connection: {e}")),
                Err(_) => return SendOutcome::Retry("APNs timed out".into()),
            };
            let status = resp.status().as_u16();
            let body = resp
                .into_body()
                .collect()
                .await
                .map(|b| b.to_bytes())
                .unwrap_or_default();
            let (outcome, renew) = classify(status, &body);
            if !(renew && !refresh) {
                return outcome;
            }
        }
        unreachable!("the second pass always returns")
    }
}
