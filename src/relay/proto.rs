//! Frames between a hub and the relay, JSON over one WebSocket the hub opens.
//!
//! The hub sends `Hello` with its two public keys, the relay a `Challenge`, the hub an `Auth`
//! signed with its signing key, and the relay `Ready` with the relay id both keys hash to. After
//! that the relay forwards phone requests as `Request`, and the hub answers each one with a
//! `Response`, or a `Reject` when it can't even decrypt it. The hub asks for pushes with `Push`.

use serde::{Deserialize, Serialize};

use super::seal::{RequestEnvelope, ResponseEnvelope};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum HubFrame {
    Hello {
        version: u32,
        /// SEC1 public keys, base64url.
        kx_key: String,
        sign_key: String,
    },
    Auth {
        /// DER ECDSA over `registration_message`, base64url.
        signature: String,
    },
    Response {
        id: u64,
        envelope: ResponseEnvelope,
    },
    /// The request couldn't be opened, so there is no key to answer it with.
    Reject {
        id: u64,
        reason: String,
    },
    Push {
        id: u64,
        push: RelayPush,
    },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum RelayFrame {
    Challenge {
        nonce: String,
    },
    Ready {
        relay_id: String,
    },
    Request {
        id: u64,
        envelope: RequestEnvelope,
    },
    PushResult {
        id: u64,
        outcome: PushOutcome,
        error: Option<String>,
    },
}

/// A notification for one device. The relay writes the text from `kind`; a hub sends ids only.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RelayPush {
    pub token: String,
    /// `sandbox` or `production`.
    pub env: String,
    /// `approval`, `finished` or `device_paired`.
    pub kind: String,
    pub session_id: Option<String>,
    pub approval_id: Option<String>,
    pub collapse_id: String,
    pub expires_at: i64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PushOutcome {
    Sent,
    Retry,
    Unregistered,
    Failed,
}

pub const MAX_FRAME_BYTES: usize = 8 << 20;
