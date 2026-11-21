//! The lock-screen board: what a phone's Live Activity or ongoing notification shows, sealed to
//! one device so the relay and Apple's push service carry only ciphertext. The hub makes a
//! one-off P-256 key, ECDH with the device's live key, HKDF-SHA256 salted with both public keys,
//! then ChaCha20-Poly1305 under a random nonce. The blob is base64url of `epk || nonce || ct`.

use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD as B64;
use chacha20poly1305::aead::{Aead, KeyInit};
use chacha20poly1305::{ChaCha20Poly1305, Key, Nonce};
use hkdf::Hkdf;
use p256::ecdh::diffie_hellman;
use p256::{PublicKey, SecretKey};
use serde::{Deserialize, Serialize};
use sha2::Sha256;

pub const VERSION: u32 = 1;
const INFO: &[u8] = b"switchboard-live-v1";
const EPK_LEN: usize = 65;
const NONCE_LEN: usize = 12;
/// Leaves room under APNs' 4 KB for `aps`, the ids and the relay's own fields.
pub const MAX_BLOB_CHARS: usize = 3000;

#[derive(Debug, thiserror::Error, PartialEq)]
pub enum LiveError {
    #[error("the device's live key is not a P-256 key")]
    BadKey,
    #[error("the board is too big to push")]
    TooBig,
    #[error("the blob does not open")]
    Open,
}

/// Seals `plain` to the device whose live key is `device_pub` (SEC1 uncompressed).
pub fn seal(device_pub: &[u8], plain: &[u8]) -> Result<String, LiveError> {
    let mut nonce = [0u8; NONCE_LEN];
    getrandom::fill(&mut nonce).expect("OS random source");
    seal_with(&random_secret(), nonce, device_pub, plain)
}

pub(crate) fn seal_with(
    eph: &SecretKey,
    nonce: [u8; NONCE_LEN],
    device_pub: &[u8],
    plain: &[u8],
) -> Result<String, LiveError> {
    let device = PublicKey::from_sec1_bytes(device_pub).map_err(|_| LiveError::BadKey)?;
    let epk = eph.public_key().to_sec1_bytes().to_vec();
    let shared = diffie_hellman(eph.to_nonzero_scalar(), device.as_affine());
    let key = derive(shared.raw_secret_bytes(), &epk, device_pub);
    let ct = ChaCha20Poly1305::new(&Key::from(key))
        .encrypt(&Nonce::from(nonce), plain)
        .expect("encrypts");
    let blob = B64.encode([epk.as_slice(), &nonce, &ct].concat());
    if blob.len() > MAX_BLOB_CHARS {
        return Err(LiveError::TooBig);
    }
    Ok(blob)
}

/// The device side, as the phones do it.
pub fn open(device: &SecretKey, blob: &str) -> Result<Vec<u8>, LiveError> {
    let raw = B64.decode(blob).map_err(|_| LiveError::Open)?;
    if raw.len() < EPK_LEN + NONCE_LEN + 16 {
        return Err(LiveError::Open);
    }
    let (epk, rest) = raw.split_at(EPK_LEN);
    let (nonce, ct) = rest.split_at(NONCE_LEN);
    let eph = PublicKey::from_sec1_bytes(epk).map_err(|_| LiveError::Open)?;
    let shared = diffie_hellman(device.to_nonzero_scalar(), eph.as_affine());
    let device_pub = device.public_key().to_sec1_bytes();
    let key = derive(shared.raw_secret_bytes(), epk, &device_pub);
    let nonce: [u8; NONCE_LEN] = nonce.try_into().map_err(|_| LiveError::Open)?;
    ChaCha20Poly1305::new(&Key::from(key))
        .decrypt(&Nonce::from(nonce), ct)
        .map_err(|_| LiveError::Open)
}

fn derive(shared: &[u8], epk: &[u8], device_pub: &[u8]) -> [u8; 32] {
    let salt = [epk, device_pub].concat();
    let mut key = [0u8; 32];
    Hkdf::<Sha256>::new(Some(&salt), shared)
        .expand(INFO, &mut key)
        .expect("32 bytes is a valid length");
    key
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

/// One agent that needs the owner, or stopped on an error.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Waiting {
    /// Its Herdr name.
    #[serde(rename = "a")]
    pub name: String,
    #[serde(rename = "s", default, skip_serializing_if = "Option::is_none")]
    pub space: Option<String>,
    /// Unix seconds it has waited since.
    #[serde(rename = "w")]
    pub since: i64,
    #[serde(rename = "e", default, skip_serializing_if = "std::ops::Not::not")]
    pub error: bool,
}

/// What the card shows. Short keys keep a worst case well inside one push.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Board {
    #[serde(rename = "v")]
    pub version: u32,
    /// Unix seconds the hub built it.
    #[serde(rename = "t")]
    pub at: i64,
    /// Longest wait first; at most `MAX_WAITING`.
    #[serde(rename = "n", default)]
    pub waiting: Vec<Waiting>,
    /// How many more wait beyond `waiting`.
    #[serde(rename = "m", default, skip_serializing_if = "is_zero")]
    pub more: u32,
    #[serde(rename = "r", default)]
    pub running: Vec<String>,
    /// Running agents beyond the named ones.
    #[serde(rename = "rm", default, skip_serializing_if = "is_zero")]
    pub running_more: u32,
    #[serde(rename = "i", default)]
    pub idle: u32,
    /// An agent that finished within the last minute.
    #[serde(rename = "f", default, skip_serializing_if = "Option::is_none")]
    pub finished: Option<String>,
    #[serde(rename = "fa", default, skip_serializing_if = "Option::is_none")]
    pub finished_at: Option<i64>,
}

fn is_zero(n: &u32) -> bool {
    *n == 0
}

/// Named needs-you entries; the rest are counted.
pub const MAX_WAITING: usize = 2;
pub const MAX_RUNNING_NAMES: usize = 8;
/// A finished agent shows for about a minute, then the card returns to calm.
pub const FINISHED_FOR_SECS: i64 = 60;

/// One agent as the hub sees it, names already masked.
#[derive(Debug, Clone, PartialEq)]
pub struct Seen {
    pub machine: String,
    /// Its Herdr name, which identifies it.
    pub name: String,
    /// What the card shows, as the app names it: a session's label, else `agent_label`. Masked.
    pub label: String,
    pub space: Option<String>,
    pub status: String,
    pub error: bool,
    /// Unix seconds since it has needed the owner, from an approval or a dialog the hub recorded.
    pub needs_you_since: Option<i64>,
}

/// An agent Switchboard didn't start as the app names it (`AgentView.agentLabel`): its Herdr
/// name without the session or core before it, or, for a pane with no name, its terminal title,
/// then its kind, then the pane.
pub fn agent_label(name: &str, title: Option<&str>, kind: Option<&str>) -> String {
    let last = name.rsplit('/').next().unwrap_or(name);
    if !last.contains(':') {
        return last.to_owned();
    }
    if let Some(t) = title.map(str::trim).filter(|t| !t.is_empty()) {
        return t.to_owned();
    }
    match kind.filter(|k| !k.is_empty()) {
        Some(k) => k[..1].to_uppercase() + &k[1..],
        None => last.to_owned(),
    }
}

/// How a new board should be pushed to a device that last got `last`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Signal {
    /// An agent newly needs the owner or stopped on an error: show the generic alert.
    pub alert: bool,
    /// Needs-you, error or all-done: high priority.
    pub urgent: bool,
}

impl Signal {
    pub fn between(last: Option<&Board>, board: &Board) -> Self {
        let alert = match last {
            None => !board.waiting.is_empty(),
            Some(l) => {
                board.waiting_count() > l.waiting_count()
                    || board.waiting.iter().any(|w| {
                        !l.waiting
                            .iter()
                            .any(|o| o.name == w.name && o.error == w.error)
                    })
            }
        };
        let all_done = last.is_some_and(|l| l.running_count() > 0)
            && board.running_count() == 0
            && board.waiting.is_empty();
        Self {
            alert,
            urgent: alert || all_done,
        }
    }
}

/// Turns successive listings into boards, remembering what one listing can't say: since when an
/// agent has been waiting, and which one just finished.
#[derive(Debug, Default)]
pub struct Tracker {
    since: std::collections::HashMap<(String, String), i64>,
    status: std::collections::HashMap<(String, String), String>,
    finished: Option<(String, i64)>,
}

impl Tracker {
    pub fn observe(&mut self, now: i64, agents: &[Seen]) -> Board {
        let mut waiting = Vec::new();
        let mut running = Vec::new();
        let mut idle = 0u32;
        let mut status = std::collections::HashMap::new();
        let mut since = std::collections::HashMap::new();
        for a in agents {
            let key = (a.machine.clone(), a.name.clone());
            let waits = a.error || a.status == "blocked" || a.needs_you_since.is_some();
            if waits {
                let first = self.since.get(&key).copied();
                let at = a.needs_you_since.or(first).unwrap_or(now).min(now);
                since.insert(key.clone(), at);
                waiting.push(Waiting {
                    name: a.label.clone(),
                    space: a.space.clone(),
                    since: at,
                    error: a.error,
                });
            } else if a.status == "working" {
                running.push(a.label.clone());
            } else {
                idle += 1;
                if self.status.get(&key).is_some_and(|s| s == "working") {
                    self.finished = Some((a.label.clone(), now));
                }
            }
            status.insert(
                key,
                if waits {
                    "waiting".into()
                } else {
                    a.status.clone()
                },
            );
        }
        self.since = since;
        self.status = status;
        waiting.sort_by(|a, b| a.since.cmp(&b.since).then_with(|| a.name.cmp(&b.name)));
        running.sort();
        let finished = self
            .finished
            .clone()
            .filter(|(_, at)| now - at < FINISHED_FOR_SECS);
        let more = waiting.len().saturating_sub(MAX_WAITING) as u32;
        waiting.truncate(MAX_WAITING);
        let running_more = running.len().saturating_sub(MAX_RUNNING_NAMES) as u32;
        running.truncate(MAX_RUNNING_NAMES);
        Board {
            version: VERSION,
            at: now,
            waiting,
            more,
            running,
            running_more,
            idle,
            finished_at: finished.as_ref().map(|f| f.1),
            finished: finished.map(|f| f.0),
        }
    }
}

impl Board {
    /// Whether two boards show the same thing, whenever each was built.
    pub fn same(&self, other: &Board) -> bool {
        Board {
            at: 0,
            ..self.clone()
        } == Board {
            at: 0,
            ..other.clone()
        }
    }

    pub fn waiting_count(&self) -> usize {
        self.waiting.len() + self.more as usize
    }

    pub fn running_count(&self) -> usize {
        self.running.len() + self.running_more as usize
    }

    /// Nothing working, waiting or just finished: the card ends after a while like this.
    pub fn is_quiet(&self) -> bool {
        self.waiting_count() == 0 && self.running_count() == 0 && self.finished.is_none()
    }

    pub fn is_empty(&self) -> bool {
        self.waiting.is_empty() && self.running.is_empty() && self.idle == 0
    }
}

#[cfg(test)]
#[path = "live_tests.rs"]
mod tests;
