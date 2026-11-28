//! Keeps each phone's lock-screen card current. After a change, and every few seconds while
//! things settle, the hub rebuilds the board; a device gets it when it differs from what it last
//! got, sealed to its live key. A quiet but non-empty board is re-sent before it goes stale, so
//! a grey card means the hub can't be reached.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use tokio::task::JoinHandle;

use super::service::SessionService;
use crate::live::{Board, Signal};
use crate::push::{LivePush, Push, SendOutcome, Sender, live_payload};
use crate::store::{LiveTarget, OutboxKind, PushEnv, PushProvider};

/// Changes closer together than this go out as one update.
pub const DEBOUNCE: Duration = Duration::from_secs(4);
/// The board is rebuilt this often anyway: listings don't announce every status change.
pub const TICK: Duration = Duration::from_secs(15);
/// The card greys this long after its last update.
pub const STALE_AFTER_SECS: i64 = 60 * 60;
/// An unchanged, non-empty board is sent again this long after the last one.
pub const REFRESH_AFTER_SECS: i64 = 45 * 60;
/// iOS ends a card after eight hours; an older one's token is no use.
pub const MAX_CARD_SECS: i64 = 8 * 60 * 60;
/// A card ends once everything has been idle this long, unless the device shows it always.
pub const QUIET_END_SECS: i64 = 15 * 60;
/// Always: a card this old is replaced through push-to-start, ahead of iOS's eight hours.
pub const RENEW_AFTER_SECS: i64 = 7 * 60 * 60 + 45 * 60;

/// Since when a device's board has been quiet, and whether its card was ended for it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Quiet {
    pub since: i64,
    pub ended: bool,
}

pub struct LiveLoop<S> {
    svc: Arc<SessionService>,
    sender: S,
    /// Per device, the board it last got and when.
    sent: HashMap<String, (Board, i64)>,
    quiet: HashMap<String, Quiet>,
    /// Per device, the card a push-to-start replaced and when, ended once the new one reports in.
    replaced: HashMap<String, (String, i64)>,
}

/// What one device should get now, if anything.
#[derive(Debug, Clone, PartialEq)]
pub enum Plan {
    Skip,
    Send { token: String, live: LivePush },
}

/// Decides one device's push from the board, its registration and what it last got.
pub fn plan(
    t: &LiveTarget,
    board: &Board,
    last: Option<&(Board, i64)>,
    quiet: Option<&Quiet>,
    now: i64,
) -> Plan {
    let always = t.reg.always;
    // While working: quiet for long enough, the card ends, once, and nothing more goes until
    // there's work again. Always: it stays.
    if board.is_quiet() && !always {
        match quiet {
            Some(q) if q.ended => return Plan::Skip,
            Some(q) if now - q.since >= QUIET_END_SECS => return end(t, board, now),
            _ => {}
        }
    }
    let started = t.reg.activity_started_at;
    let card_alive =
        t.reg.activity_token.is_some() && started.is_none_or(|s| now - s < MAX_CARD_SECS);
    // Not twice for one card: a start waits until the last one is past its eight hours.
    let may_start = t.reg.start_token.is_some() && started.is_none_or(|s| now - s >= MAX_CARD_SECS);
    let renew = always
        && t.reg.start_token.is_some()
        && card_alive
        && started.is_some_and(|s| now - s >= RENEW_AFTER_SECS);
    let missing = always && !card_alive && may_start;
    let signal = Signal::between(last.map(|l| &l.0), board);
    let changed = last.is_none_or(|(b, _)| !b.same(board));
    let refresh = last.is_some_and(|(_, at)| now - at >= REFRESH_AFTER_SECS) && !board.is_empty();
    let apns = t.push_provider == PushProvider::Apns;
    if !changed && !refresh && !signal.urgent && !(apns && (renew || missing)) {
        return Plan::Skip;
    }
    let live = |event: &str, blob: Option<String>| LivePush {
        event: event.into(),
        blob,
        urgent: signal.urgent,
        alert: signal.alert,
        timestamp: now,
        stale_at: now + STALE_AFTER_SECS,
    };
    match t.push_provider {
        PushProvider::Fcm => match &t.push_token {
            Some(token) => Plan::Send {
                token: token.clone(),
                live: live("update", None),
            },
            None => Plan::Skip,
        },
        PushProvider::Apns => {
            let Some(key) = t.reg.live_key.as_deref().and_then(decode_key) else {
                return Plan::Skip;
            };
            let start = t.reg.start_token.clone();
            let (event, token) = match (&t.reg.activity_token, start) {
                // Always: a fresh card before iOS ends this one at eight hours.
                (_, Some(s)) if renew => ("start", s),
                (Some(a), _) if card_alive => ("update", a.clone()),
                // A new card for something that needs the owner, or, always, for none at all.
                (_, Some(s)) if may_start && (signal.alert || always) => ("start", s),
                _ => return Plan::Skip,
            };
            let plain = serde_json::to_vec(board).expect("serializes");
            match crate::live::seal(&key, &plain) {
                Ok(blob) => Plan::Send {
                    token,
                    live: live(event, Some(blob)),
                },
                Err(e) => {
                    tracing::warn!(device = %t.device_id, error = %e, "sealing the board");
                    Plan::Skip
                }
            }
        }
    }
}

/// Ends the card: iOS's running card by its token, Android's notification by a wake-up.
fn end(t: &LiveTarget, board: &Board, now: i64) -> Plan {
    let live = |blob: Option<String>| LivePush {
        event: "end".into(),
        blob,
        urgent: false,
        alert: false,
        timestamp: now,
        stale_at: now,
    };
    match t.push_provider {
        PushProvider::Fcm => match &t.push_token {
            Some(token) => Plan::Send {
                token: token.clone(),
                live: live(None),
            },
            None => Plan::Skip,
        },
        PushProvider::Apns => match &t.reg.activity_token {
            Some(token) => end_card(t, token, board, now),
            None => Plan::Skip,
        },
    }
}

/// Ends the card a push-to-start replaced, once the phone has reported a different one.
pub fn retire(t: &LiveTarget, old: &str, board: &Board, now: i64) -> Plan {
    match &t.reg.activity_token {
        Some(new) if t.push_provider == PushProvider::Apns && new != old => {
            end_card(t, old, board, now)
        }
        _ => Plan::Skip,
    }
}

fn end_card(t: &LiveTarget, token: &str, board: &Board, now: i64) -> Plan {
    let Some(key) = t.reg.live_key.as_deref().and_then(decode_key) else {
        return Plan::Skip;
    };
    let plain = serde_json::to_vec(board).expect("serializes");
    match crate::live::seal(&key, &plain) {
        Ok(blob) => Plan::Send {
            token: token.into(),
            live: LivePush {
                event: "end".into(),
                blob: Some(blob),
                urgent: false,
                alert: false,
                timestamp: now,
                stale_at: now,
            },
        },
        Err(_) => Plan::Skip,
    }
}

fn push(t: &LiveTarget, token: &str, live: LivePush) -> Push {
    Push {
        device_id: t.device_id.clone(),
        kind: OutboxKind::Live,
        session_id: None,
        approval_id: None,
        attention_id: None,
        token: Some(token.into()),
        env: t.push_env.unwrap_or(PushEnv::Production),
        provider: t.push_provider,
        payload: live_payload(&live),
        collapse_id: "live".into(),
        expires_at: live.stale_at,
        live: Some(live),
    }
}

fn decode_key(k: &str) -> Option<Vec<u8>> {
    use base64::Engine;
    base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(k)
        .ok()
}

impl<S: Sender> LiveLoop<S> {
    pub fn new(svc: Arc<SessionService>, sender: S) -> Self {
        Self {
            svc,
            sender,
            sent: HashMap::new(),
            quiet: HashMap::new(),
            replaced: HashMap::new(),
        }
    }

    /// Rebuilds the board once and sends it where it is due.
    pub async fn tick(&mut self) {
        let targets = match self.svc.store().live_targets().await {
            Ok(t) if !t.is_empty() => t,
            Ok(_) => return,
            Err(e) => {
                tracing::warn!(error = %e, "listing lock-screen cards");
                return;
            }
        };
        let board = match self.svc.board().await {
            Ok(b) => b,
            Err(e) => {
                tracing::warn!(error = %e, "building the board");
                return;
            }
        };
        let now = self.svc.now() / 1000;
        if board.is_quiet() {
            for t in &targets {
                self.quiet.entry(t.device_id.clone()).or_insert(Quiet {
                    since: now,
                    ended: false,
                });
            }
        } else {
            self.quiet.clear();
        }
        self.sent
            .retain(|d, _| targets.iter().any(|t| &t.device_id == d));
        self.replaced.retain(|d, (_, at)| {
            now - *at < MAX_CARD_SECS && targets.iter().any(|t| &t.device_id == d)
        });
        for t in &targets {
            if let Some((old, _)) = self.replaced.get(&t.device_id)
                && let Plan::Send { token, live } = retire(t, old, &board, now)
            {
                match self.sender.send(&push(t, &token, live)).await {
                    SendOutcome::Retry(e) => {
                        tracing::debug!(device = %t.device_id, error = %e, "replaced card not ended yet");
                    }
                    _ => {
                        self.replaced.remove(&t.device_id);
                    }
                }
            }
            let Plan::Send { token, live } = plan(
                t,
                &board,
                self.sent.get(&t.device_id),
                self.quiet.get(&t.device_id),
                now,
            ) else {
                continue;
            };
            let start = live.event == "start";
            let ending = live.event == "end";
            match self.sender.send(&push(t, &token, live)).await {
                SendOutcome::Sent => {
                    self.sent.insert(t.device_id.clone(), (board.clone(), now));
                    if ending && let Some(q) = self.quiet.get_mut(&t.device_id) {
                        q.ended = true;
                    }
                    if start {
                        if let Some(old) = &t.reg.activity_token {
                            self.replaced
                                .insert(t.device_id.clone(), (old.clone(), now));
                        }
                        if let Err(e) = self.svc.store().live_started(&t.device_id, now).await {
                            tracing::warn!(error = %e, "recording a started card");
                        }
                    }
                }
                SendOutcome::Unregistered(e) => {
                    tracing::info!(device = %t.device_id, error = %e, "a card's token is gone");
                    if let Err(e) = self.svc.store().drop_live_token(&t.device_id, &token).await {
                        tracing::warn!(error = %e, "forgetting a card's token");
                    }
                }
                SendOutcome::Retry(e) | SendOutcome::Failed(e) => {
                    tracing::debug!(device = %t.device_id, error = %e, "card update not sent");
                }
            }
        }
    }

    pub fn spawn(mut self) -> JoinHandle<()> {
        tokio::spawn(async move {
            let mut changes = self.svc.watch_changes();
            let mut events = self.svc.subscribe();
            loop {
                tokio::select! {
                    _ = changes.changed() => {}
                    _ = events.recv() => {}
                    _ = tokio::time::sleep(TICK) => {}
                }
                tokio::time::sleep(DEBOUNCE).await;
                while events.try_recv().is_ok() {}
                changes.mark_unchanged();
                self.tick().await;
            }
        })
    }
}

#[cfg(test)]
#[path = "live_tests.rs"]
mod tests;
