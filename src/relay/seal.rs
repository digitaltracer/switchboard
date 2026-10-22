//! End-to-end encryption between a phone and its hub, through the relay. Each request uses a
//! fresh phone key: ECDH with the hub's static P-256 key, HKDF-SHA256 into a request key and a
//! reply key, ChaCha20-Poly1305 under each. The request key is single-use, so its nonce is zero;
//! each reply carries a random nonce, so the reply key stays safe even if it seals twice. The relay
//! sees which hub a message is for and nothing else. The app's `RelayTransport` is the phone side.

use std::collections::HashMap;
use std::path::Path;
use std::sync::Mutex;

use base64::Engine;
use base64::engine::general_purpose::{STANDARD as B64STD, URL_SAFE_NO_PAD as B64};
use chacha20poly1305::aead::{Aead, KeyInit};
use chacha20poly1305::{ChaCha20Poly1305, Key, Nonce};
use hkdf::Hkdf;
use p256::ecdh::diffie_hellman;
use p256::ecdsa::signature::Signer;
use p256::ecdsa::{Signature, SigningKey};
use p256::pkcs8::{DecodePrivateKey, EncodePrivateKey};
use p256::{PublicKey, SecretKey};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

pub const VERSION: u32 = 2;
const REQUEST_INFO: &[u8] = b"switchboard-relay-v1 request";
const REPLY_INFO: &[u8] = b"switchboard-relay-v1 reply";
/// How far a request's clock may be from the hub's.
pub const MAX_SKEW_MS: i64 = 5 * 60 * 1000;

#[derive(Debug, thiserror::Error, PartialEq)]
pub enum SealError {
    #[error("unsupported envelope version {0}")]
    Version(u32),
    #[error("bad envelope: {0}")]
    Malformed(String),
    #[error("the envelope does not decrypt")]
    Decrypt,
    #[error("the request is too old or from the future")]
    Stale,
    #[error("the request was already seen")]
    Replay,
}

/// What the phone posts to the relay.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RequestEnvelope {
    pub v: u32,
    /// The phone's one-off P-256 public key, SEC1 uncompressed, base64url.
    pub epk: String,
    /// ChaCha20-Poly1305 ciphertext and tag, base64url.
    pub ct: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ResponseEnvelope {
    pub v: u32,
    /// The 12-byte nonce, base64url.
    pub n: String,
    pub ct: String,
}

/// An HTTP request as the phone made it; the hub runs it against its own app API.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PlainRequest {
    /// Unix milliseconds on the phone.
    pub ts: i64,
    pub method: String,
    /// Path and query, e.g. `/app/v1/sessions?after_revision=3`.
    pub path: String,
    pub headers: Vec<(String, String)>,
    /// Standard base64.
    #[serde(default)]
    pub body: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PlainResponse {
    pub status: u16,
    pub headers: Vec<(String, String)>,
    /// Standard base64.
    #[serde(default)]
    pub body: String,
}

impl PlainRequest {
    pub fn body_bytes(&self) -> Result<Vec<u8>, SealError> {
        B64STD
            .decode(&self.body)
            .map_err(|e| SealError::Malformed(format!("body: {e}")))
    }
}

impl PlainResponse {
    pub fn new(status: u16, headers: Vec<(String, String)>, body: &[u8]) -> Self {
        Self {
            status,
            headers,
            body: B64STD.encode(body),
        }
    }

    pub fn body_bytes(&self) -> Result<Vec<u8>, SealError> {
        B64STD
            .decode(&self.body)
            .map_err(|e| SealError::Malformed(format!("body: {e}")))
    }
}

/// The hub's relay identity: a signing key that proves it to the relay, and a key-agreement key
/// phones encrypt to. Its relay id is a hash of both public keys, so nobody else can claim it.
pub struct HubKeys {
    sign: SigningKey,
    kx: SecretKey,
}

#[derive(Serialize, Deserialize)]
struct KeyFile {
    /// PKCS#8 DER, standard base64.
    sign: String,
    kx: String,
}

impl HubKeys {
    pub fn generate() -> Self {
        Self {
            sign: SigningKey::from(random_secret()),
            kx: random_secret(),
        }
    }

    /// Reads the key file, creating it with mode 0600 on first use. Never overwrites one.
    pub fn load_or_create(path: &Path) -> anyhow::Result<Self> {
        use std::io::Write;
        use std::os::unix::fs::OpenOptionsExt;
        match std::fs::read_to_string(path) {
            Ok(text) => {
                let f: KeyFile = serde_json::from_str(&text)
                    .map_err(|_| anyhow::anyhow!("{} is not a relay key file", path.display()))?;
                let der = |s: &str| {
                    B64STD
                        .decode(s)
                        .map_err(|_| anyhow::anyhow!("bad key encoding"))
                };
                Ok(Self {
                    sign: SigningKey::from_pkcs8_der(&der(&f.sign)?)
                        .map_err(|_| anyhow::anyhow!("bad signing key in {}", path.display()))?,
                    kx: SecretKey::from_pkcs8_der(&der(&f.kx)?).map_err(|_| {
                        anyhow::anyhow!("bad key-agreement key in {}", path.display())
                    })?,
                })
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                let keys = Self::generate();
                let f = KeyFile {
                    sign: B64STD.encode(keys.sign.to_pkcs8_der()?.as_bytes()),
                    kx: B64STD.encode(keys.kx.to_pkcs8_der()?.as_bytes()),
                };
                if let Some(dir) = path.parent() {
                    std::fs::create_dir_all(dir)?;
                }
                let mut file = std::fs::OpenOptions::new()
                    .write(true)
                    .create_new(true)
                    .mode(0o600)
                    .open(path)?;
                file.write_all(serde_json::to_string(&f)?.as_bytes())?;
                file.sync_all()?;
                Ok(keys)
            }
            Err(e) => Err(anyhow::anyhow!("reading {}: {e}", path.display())),
        }
    }

    /// Fixed keys, for cross-language test vectors.
    #[cfg(test)]
    pub(crate) fn from_secrets(sign: &[u8], kx: &[u8]) -> Self {
        Self {
            sign: SigningKey::from_slice(sign).expect("a P-256 scalar"),
            kx: SecretKey::from_slice(kx).expect("a P-256 scalar"),
        }
    }

    pub fn kx_public(&self) -> Vec<u8> {
        self.kx.public_key().to_sec1_bytes().to_vec()
    }

    pub fn sign_public(&self) -> Vec<u8> {
        self.sign.verifying_key().to_sec1_bytes().to_vec()
    }

    pub fn relay_id(&self) -> String {
        relay_id(&self.kx_public(), &self.sign_public())
    }

    /// DER ECDSA over SHA-256, base64url.
    pub fn sign(&self, message: &str) -> String {
        let sig: Signature = self.sign.sign(message.as_bytes());
        B64.encode(sig.to_der().as_bytes())
    }

    /// Decrypts a phone's request; the reply key seals the answer.
    pub fn open_request(
        &self,
        env: &RequestEnvelope,
    ) -> Result<(PlainRequest, ReplyKey), SealError> {
        if env.v != VERSION {
            return Err(SealError::Version(env.v));
        }
        let epk = decode(&env.epk, "epk")?;
        let phone = PublicKey::from_sec1_bytes(&epk)
            .map_err(|_| SealError::Malformed("epk is not a P-256 key".into()))?;
        let shared = diffie_hellman(self.kx.to_nonzero_scalar(), phone.as_affine());
        let (req_key, reply_key) = derive(shared.raw_secret_bytes(), &epk, &self.kx_public());
        let plain = aead(&req_key)
            .decrypt(&Nonce::default(), decode(&env.ct, "ct")?.as_slice())
            .map_err(|_| SealError::Decrypt)?;
        let req = serde_json::from_slice(&plain)
            .map_err(|e| SealError::Malformed(format!("request: {e}")))?;
        Ok((
            req,
            ReplyKey {
                key: reply_key,
                epk: env.epk.clone(),
            },
        ))
    }
}

/// The key a hub answers one request with.
pub struct ReplyKey {
    key: [u8; 32],
    /// Identifies the request for the replay guard.
    pub epk: String,
}

impl ReplyKey {
    /// Consumes the key: a request gets one answer.
    pub fn seal(self, resp: &PlainResponse) -> ResponseEnvelope {
        let mut nonce = [0u8; 12];
        getrandom::fill(&mut nonce).expect("OS random source");
        self.seal_with(resp, nonce)
    }

    pub(crate) fn seal_with(self, resp: &PlainResponse, nonce: [u8; 12]) -> ResponseEnvelope {
        let plain = serde_json::to_vec(resp).expect("serializes");
        let ct = aead(&self.key)
            .encrypt(&Nonce::from(nonce), plain.as_slice())
            .expect("encrypts");
        ResponseEnvelope {
            v: VERSION,
            n: B64.encode(nonce),
            ct: B64.encode(ct),
        }
    }
}

/// `hex(sha256(kx_pub || sign_pub))[..32]`; the app computes the same from the pairing link.
pub fn relay_id(kx_pub: &[u8], sign_pub: &[u8]) -> String {
    let mut h = Sha256::new();
    h.update(kx_pub);
    h.update(sign_pub);
    hex::encode(h.finalize())[..32].to_owned()
}

/// What a hub signs to prove its relay id to the relay.
pub fn registration_message(nonce: &str, kx_pub: &str, sign_pub: &str) -> String {
    format!("switchboard-relay-hub-v{VERSION}\nnonce={nonce}\nkx={kx_pub}\nsign={sign_pub}")
}

/// Refuses requests outside the clock window, and any request key seen before.
#[derive(Default)]
pub struct ReplayGuard {
    seen: Mutex<HashMap<String, i64>>,
}

impl ReplayGuard {
    pub fn check(&self, epk: &str, ts: i64, now: i64) -> Result<(), SealError> {
        if (ts - now).abs() > MAX_SKEW_MS {
            return Err(SealError::Stale);
        }
        let mut seen = self.seen.lock().unwrap();
        seen.retain(|_, t| now - *t <= 2 * MAX_SKEW_MS);
        if seen.insert(epk.to_owned(), now).is_some() {
            return Err(SealError::Replay);
        }
        Ok(())
    }
}

fn random_secret() -> SecretKey {
    loop {
        let mut b = [0u8; 32];
        getrandom::fill(&mut b).expect("OS random source");
        if let Ok(k) = SecretKey::from_slice(&b) {
            return k;
        }
    }
}

fn decode(s: &str, what: &str) -> Result<Vec<u8>, SealError> {
    B64.decode(s)
        .map_err(|_| SealError::Malformed(format!("{what} is not base64url")))
}

fn aead(key: &[u8; 32]) -> ChaCha20Poly1305 {
    ChaCha20Poly1305::new(&Key::from(*key))
}

/// HKDF-SHA256 over the ECDH secret, salted with both public keys.
fn derive(shared: &[u8], epk: &[u8], hub_kx: &[u8]) -> ([u8; 32], [u8; 32]) {
    let salt = [epk, hub_kx].concat();
    let hk = Hkdf::<Sha256>::new(Some(&salt), shared);
    let (mut req, mut reply) = ([0u8; 32], [0u8; 32]);
    hk.expand(REQUEST_INFO, &mut req)
        .expect("32 bytes is a valid length");
    hk.expand(REPLY_INFO, &mut reply)
        .expect("32 bytes is a valid length");
    (req, reply)
}

/// The phone side, as the app does it; used by tests and the relay's own checks.
pub mod phone {
    use super::*;

    pub struct Pending {
        reply_key: [u8; 32],
    }

    /// Seals with a given one-off key, so a test vector is reproducible.
    pub fn seal_with(
        eph: &SecretKey,
        hub_kx_pub: &[u8],
        req: &PlainRequest,
    ) -> (RequestEnvelope, Pending) {
        let hub = PublicKey::from_sec1_bytes(hub_kx_pub).expect("a P-256 key");
        let epk = eph.public_key().to_sec1_bytes().to_vec();
        let shared = diffie_hellman(eph.to_nonzero_scalar(), hub.as_affine());
        let (req_key, reply_key) = derive(shared.raw_secret_bytes(), &epk, hub_kx_pub);
        let ct = aead(&req_key)
            .encrypt(
                &Nonce::default(),
                serde_json::to_vec(req).expect("serializes").as_slice(),
            )
            .expect("encrypts");
        (
            RequestEnvelope {
                v: VERSION,
                epk: B64.encode(epk),
                ct: B64.encode(ct),
            },
            Pending { reply_key },
        )
    }

    pub fn seal(hub_kx_pub: &[u8], req: &PlainRequest) -> (RequestEnvelope, Pending) {
        seal_with(&random_secret(), hub_kx_pub, req)
    }

    impl Pending {
        pub fn open(&self, env: &ResponseEnvelope) -> Result<PlainResponse, SealError> {
            if env.v != VERSION {
                return Err(SealError::Version(env.v));
            }
            let nonce: [u8; 12] = decode(&env.n, "n")?
                .try_into()
                .map_err(|_| SealError::Malformed("n is not 12 bytes".into()))?;
            let plain = aead(&self.reply_key)
                .decrypt(&Nonce::from(nonce), decode(&env.ct, "ct")?.as_slice())
                .map_err(|_| SealError::Decrypt)?;
            serde_json::from_slice(&plain)
                .map_err(|e| SealError::Malformed(format!("response: {e}")))
        }
    }
}

#[cfg(test)]
#[path = "seal_tests.rs"]
mod tests;
