//! The runner's 127.0.0.1 endpoint for hook helpers, and the approvals waiting on the hub.
//! Decisions only arrive from the hub; anything undecided becomes a deny.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::{mpsc, oneshot};
use tokio::time::Instant;

use crate::cli::Agent;
use crate::hook::wire::{HookReply, HookRequest, MAX_LINE};
use crate::proto::Event;
use crate::proto::auth::{hash_token, token_matches};

const REQUEST_READ_TIMEOUT: Duration = Duration::from_secs(10);

/// What the fallback needs to know about a session's hooks.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct HookState {
    pub live: bool,
    pub pending: usize,
    pub open_calls: usize,
    /// When a decision was last delivered to a helper.
    pub last_answered: Option<Instant>,
    /// When a helper last went away without its answer.
    pub last_failure: Option<Instant>,
    /// Latched once the session is surfaced as `hook_failed`; no more keys are sent.
    pub hook_failed: bool,
}

struct SessionHooks {
    token_hash: String,
    agent: Agent,
    deny_patterns: Vec<String>,
    state: HookState,
    pending: HashMap<String, oneshot::Sender<HookReply>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DecideError {
    UnknownSession,
    /// Already decided, expired, or the helper went away.
    NotPending,
}

pub struct HookHub {
    sessions: Mutex<HashMap<String, SessionHooks>>,
    events: mpsc::Sender<Event>,
    approval_deadline: Duration,
}

impl HookHub {
    pub fn new(events: mpsc::Sender<Event>, approval_deadline: Duration) -> Arc<Self> {
        Arc::new(Self {
            sessions: Mutex::default(),
            events,
            approval_deadline,
        })
    }

    pub fn register(
        &self,
        session_id: &str,
        token: &str,
        agent: Agent,
        deny_patterns: Vec<String>,
    ) {
        self.sessions.lock().unwrap().insert(
            session_id.to_owned(),
            SessionHooks {
                token_hash: hash_token(token),
                agent,
                deny_patterns,
                state: HookState::default(),
                pending: HashMap::new(),
            },
        );
    }

    /// Forgets a session; helpers still waiting get a deny.
    pub fn unregister(&self, session_id: &str) {
        self.sessions.lock().unwrap().remove(session_id);
    }

    pub fn state(&self, session_id: &str) -> Option<HookState> {
        let sessions = self.sessions.lock().unwrap();
        sessions.get(session_id).map(|s| HookState {
            pending: s.pending.len(),
            ..s.state
        })
    }

    #[cfg(test)]
    pub(crate) fn test_mark_live(&self, session_id: &str) {
        self.with(session_id, |s| s.state.live = true);
    }

    pub fn mark_hook_failed(&self, session_id: &str) {
        if let Some(s) = self.sessions.lock().unwrap().get_mut(session_id) {
            s.state.hook_failed = true;
        }
    }

    pub fn decide(
        &self,
        session_id: &str,
        approval_id: &str,
        allow: bool,
    ) -> Result<(), DecideError> {
        let mut sessions = self.sessions.lock().unwrap();
        let s = sessions
            .get_mut(session_id)
            .ok_or(DecideError::UnknownSession)?;
        let tx = s
            .pending
            .remove(approval_id)
            .ok_or(DecideError::NotPending)?;
        let reply = if allow {
            HookReply::Allow {
                updated_input: None,
            }
        } else {
            HookReply::deny("denied via switchboard")
        };
        tx.send(reply).map_err(|_| DecideError::NotPending)
    }

    fn emit(&self, event: Event) {
        if let Err(e) = self.events.try_send(event) {
            tracing::warn!(error = %e, "dropping runner event");
        }
    }

    fn with<R>(&self, session_id: &str, f: impl FnOnce(&mut SessionHooks) -> R) -> Option<R> {
        self.sessions.lock().unwrap().get_mut(session_id).map(f)
    }

    pub async fn serve(self: Arc<Self>, listener: TcpListener) {
        loop {
            match listener.accept().await {
                Ok((stream, _)) => {
                    tokio::spawn(self.clone().handle(stream));
                }
                Err(e) => {
                    tracing::warn!(error = %e, "hook endpoint accept failed");
                    tokio::time::sleep(Duration::from_millis(100)).await;
                }
            }
        }
    }

    async fn handle(self: Arc<Self>, stream: TcpStream) {
        let (read, mut write) = stream.into_split();
        let mut reader = BufReader::new(read);
        let reply =
            match tokio::time::timeout(REQUEST_READ_TIMEOUT, read_request(&mut reader)).await {
                Ok(Ok(req)) => self.dispatch(req, &mut reader).await,
                Ok(Err(e)) => Some((None, HookReply::deny(format!("bad hook request: {e}")))),
                Err(_) => Some((None, HookReply::deny("hook request timed out"))),
            };
        let Some((session_id, reply)) = reply else {
            return;
        };
        let mut line = serde_json::to_vec(&reply).expect("replies serialize");
        line.push(b'\n');
        let delivered = write.write_all(&line).await.is_ok();
        if let Some(id) = session_id {
            self.with(&id, |s| {
                if delivered {
                    s.state.last_answered = Some(Instant::now());
                } else {
                    s.state.last_failure = Some(Instant::now());
                }
            });
        }
    }

    /// Returns `None` when the helper went away and there is nobody to answer.
    async fn dispatch(
        &self,
        req: HookRequest,
        reader: &mut BufReader<tokio::net::tcp::OwnedReadHalf>,
    ) -> Option<(Option<String>, HookReply)> {
        let sid = req.session_id.clone();
        let authorized = self
            .with(&sid, |s| {
                s.agent == req.agent && token_matches(&req.token, &s.token_hash)
            })
            .unwrap_or(false);
        if !authorized {
            return Some((None, HookReply::deny("switchboard: unauthorized hook call")));
        }
        let _call = OpenCall::new(self, &sid);
        match req.event.as_str() {
            "SessionStart" => {
                self.with(&sid, |s| s.state.live = true);
                Some((Some(sid), HookReply::Ack))
            }
            "PermissionRequest" => self.permission(sid, &req.payload, reader).await,
            _ => Some((Some(sid), HookReply::Ack)),
        }
    }

    async fn permission(
        &self,
        sid: String,
        payload: &Value,
        reader: &mut BufReader<tokio::net::tcp::OwnedReadHalf>,
    ) -> Option<(Option<String>, HookReply)> {
        let tool_name = payload
            .get("tool_name")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_owned();
        let tool_input = payload.get("tool_input").cloned().unwrap_or(Value::Null);

        let blocked_by = self
            .with(&sid, |s| {
                matching_deny_pattern(&s.deny_patterns, &tool_name, &tool_input)
            })
            .flatten();
        if let Some(pattern) = blocked_by {
            self.emit(Event::Audit {
                session_id: sid.clone(),
                kind: "deny_pattern".into(),
                detail: json!({ "tool_name": tool_name, "pattern": pattern }),
            });
            return Some((
                Some(sid),
                HookReply::deny(format!("switchboard: blocked by deny pattern {pattern:?}")),
            ));
        }

        let approval_id = super::random_id();
        let (tx, rx) = oneshot::channel();
        self.with(&sid, |s| s.pending.insert(approval_id.clone(), tx))?;
        self.emit(Event::ApprovalRequested {
            session_id: sid.clone(),
            approval_id: approval_id.clone(),
            tool_name,
            tool_input,
            deadline_ms: self.approval_deadline.as_millis() as u64,
        });

        let mut probe = [0u8; 1];
        let closed = |reason: &str| Event::ApprovalClosed {
            session_id: sid.clone(),
            approval_id: approval_id.clone(),
            reason: reason.into(),
        };
        tokio::select! {
            decided = rx => Some((Some(sid.clone()), decided.unwrap_or_else(|_| HookReply::deny("switchboard: session stopped")))),
            _ = tokio::time::sleep(self.approval_deadline) => {
                self.with(&sid, |s| s.pending.remove(&approval_id));
                self.emit(closed("expired"));
                Some((Some(sid.clone()), HookReply::deny("switchboard: approval expired")))
            }
            // The helper never sends more than one line, so any read result means it went away.
            _ = reader.read(&mut probe) => {
                self.with(&sid, |s| {
                    s.pending.remove(&approval_id);
                    s.state.last_failure = Some(Instant::now());
                });
                self.emit(closed("hook_helper_gone"));
                None
            }
        }
    }
}

/// Counts a helper connection as open for the fallback's "hook still running" check.
struct OpenCall<'a> {
    hub: &'a HookHub,
    session_id: String,
}

impl<'a> OpenCall<'a> {
    fn new(hub: &'a HookHub, session_id: &str) -> Self {
        hub.with(session_id, |s| s.state.open_calls += 1);
        Self {
            hub,
            session_id: session_id.to_owned(),
        }
    }
}

impl Drop for OpenCall<'_> {
    fn drop(&mut self) {
        self.hub.with(&self.session_id, |s| s.state.open_calls -= 1);
    }
}

async fn read_request(
    reader: &mut BufReader<tokio::net::tcp::OwnedReadHalf>,
) -> Result<HookRequest, String> {
    let mut buf = Vec::new();
    (&mut *reader)
        .take(MAX_LINE)
        .read_until(b'\n', &mut buf)
        .await
        .map_err(|e| e.to_string())?;
    serde_json::from_slice(&buf).map_err(|e| e.to_string())
}

/// Case-sensitive substring match on the tool name, a shell command, or the raw tool input.
pub fn matching_deny_pattern(
    patterns: &[String],
    tool_name: &str,
    tool_input: &Value,
) -> Option<String> {
    let command = tool_input
        .get("command")
        .and_then(Value::as_str)
        .unwrap_or("");
    let raw = tool_input.to_string();
    patterns
        .iter()
        .find(|p| {
            tool_name.contains(p.as_str())
                || command.contains(p.as_str())
                || raw.contains(p.as_str())
        })
        .cloned()
}

#[cfg(test)]
#[path = "hooks_tests.rs"]
mod tests;
