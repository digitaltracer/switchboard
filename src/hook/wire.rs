//! What the hook helper and the runner's 127.0.0.1 endpoint exchange: one JSON line each way.

use std::io;
use std::net::SocketAddr;
use std::path::Path;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::cli::Agent;

pub const MAX_LINE: u64 = 1 << 20;

/// Per-session file outside the worktree; the hook command line names it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SecretFile {
    pub addr: SocketAddr,
    pub session_id: String,
    pub token: String,
}

impl SecretFile {
    pub fn write(&self, path: &Path) -> io::Result<()> {
        use std::io::Write;
        use std::os::unix::fs::OpenOptionsExt;
        let mut f = std::fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .mode(0o600)
            .open(path)?;
        f.write_all(&serde_json::to_vec(self).map_err(io::Error::other)?)
    }

    pub fn read(path: &Path) -> io::Result<Self> {
        let bytes = std::fs::read(path)?;
        if bytes.len() > 64 << 10 {
            return Err(io::Error::other("secret file too large"));
        }
        serde_json::from_slice(&bytes).map_err(io::Error::other)
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct HookRequest {
    pub session_id: String,
    pub token: String,
    pub agent: Agent,
    pub event: String,
    pub payload: Value,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "reply", rename_all = "snake_case")]
pub enum HookReply {
    /// Non-decision events such as SessionStart.
    Ack,
    Allow {
        updated_input: Option<Value>,
    },
    Deny {
        message: String,
    },
}

impl HookReply {
    pub fn deny(message: impl Into<String>) -> Self {
        HookReply::Deny {
            message: message.into(),
        }
    }
}
