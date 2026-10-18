//! Debouncing of agent status and a rate-limited outbox: a quiet chat lowers the ban risk.

use std::collections::{HashMap, VecDeque};
use std::time::Duration;

use tokio::time::Instant;

use crate::herdr::types::AgentStatus;

/// A status must hold this long before it can notify.
pub const HOLD: Duration = Duration::from_secs(3);
pub const BURST: u32 = 5;
/// One message token is regained every `REFILL`.
pub const REFILL: Duration = Duration::from_secs(12);
pub const MAX_QUEUE: usize = 20;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Notify {
    Blocked,
    Finished,
}

#[derive(Default)]
struct AgentTrack {
    stable: Option<AgentStatus>,
    pending: Option<(AgentStatus, Instant)>,
    /// Seen working or blocked since the last settled ready state, even briefly.
    went_active: bool,
}

/// Turns raw status observations into at most one notification per real transition.
#[derive(Default)]
pub struct Debouncer {
    agents: HashMap<String, AgentTrack>,
}

fn ready(s: AgentStatus) -> bool {
    matches!(s, AgentStatus::Idle | AgentStatus::Done)
}

impl Debouncer {
    pub fn observe(&mut self, agent: &str, status: AgentStatus, now: Instant) {
        let t = self.agents.entry(agent.to_owned()).or_default();
        // A turn shorter than `HOLD` never settles as working, but it still finished.
        if matches!(status, AgentStatus::Working | AgentStatus::Blocked) {
            t.went_active = true;
        }
        match t.pending {
            Some((s, _)) if s == status => {}
            _ if t.stable == Some(status) && t.pending.is_none() => {}
            _ => t.pending = Some((status, now)),
        }
    }

    /// Statuses that have held for `HOLD`, as notifications.
    pub fn due(&mut self, now: Instant) -> Vec<(String, Notify)> {
        let mut out = Vec::new();
        for (agent, t) in &mut self.agents {
            let Some((status, since)) = t.pending else {
                continue;
            };
            if now.saturating_duration_since(since) < HOLD {
                continue;
            }
            t.pending = None;
            let prev = t.stable.replace(status);
            let notify = if status == AgentStatus::Blocked {
                (prev != Some(AgentStatus::Blocked)).then_some(Notify::Blocked)
            } else if ready(status) {
                std::mem::take(&mut t.went_active).then_some(Notify::Finished)
            } else {
                None
            };
            if let Some(n) = notify {
                out.push((agent.clone(), n));
            }
        }
        out.sort();
        out
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Outgoing {
    pub to: String,
    pub text: String,
    /// Notifications name their agent so a newer one replaces a queued older one.
    pub about: Option<String>,
}

/// Token bucket in front of the transport. Replies jump ahead of notifications.
pub struct Outbox {
    tokens: f64,
    last: Instant,
    replies: VecDeque<Outgoing>,
    notes: VecDeque<Outgoing>,
}

impl Outbox {
    pub fn new(now: Instant) -> Self {
        Self {
            tokens: BURST as f64,
            last: now,
            replies: VecDeque::new(),
            notes: VecDeque::new(),
        }
    }

    pub fn push(&mut self, out: Outgoing) {
        match &out.about {
            None => self.replies.push_back(out),
            Some(agent) => {
                self.notes
                    .retain(|n| !(n.about.as_ref() == Some(agent) && n.to == out.to));
                self.notes.push_back(out);
            }
        }
        while self.replies.len() + self.notes.len() > MAX_QUEUE {
            if self.notes.pop_front().is_none() {
                self.replies.pop_front();
            }
        }
    }

    pub fn len(&self) -> usize {
        self.replies.len() + self.notes.len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Puts back a message the transport failed to send and refunds its token.
    pub fn requeue(&mut self, out: Outgoing) {
        self.tokens = (self.tokens + 1.0).min(BURST as f64);
        match out.about {
            None => self.replies.push_front(out),
            Some(_) => self.notes.push_front(out),
        }
    }

    /// The next message allowed to go out now, if any.
    pub fn pop_ready(&mut self, now: Instant) -> Option<Outgoing> {
        let elapsed = now.saturating_duration_since(self.last).as_secs_f64();
        self.tokens = (self.tokens + elapsed / REFILL.as_secs_f64()).min(BURST as f64);
        self.last = now;
        if self.tokens < 1.0 || self.is_empty() {
            return None;
        }
        self.tokens -= 1.0;
        self.replies.pop_front().or_else(|| self.notes.pop_front())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(t0: Instant, secs: u64) -> Instant {
        t0 + Duration::from_secs(secs)
    }

    #[test]
    fn notifies_once_per_transition_after_hold() {
        let t0 = Instant::now();
        let mut d = Debouncer::default();
        d.observe("a", AgentStatus::Working, t0);
        assert!(
            d.due(at(t0, 5)).is_empty(),
            "first working is just recorded"
        );
        d.observe("a", AgentStatus::Blocked, at(t0, 6));
        assert!(d.due(at(t0, 7)).is_empty(), "not held yet");
        assert_eq!(d.due(at(t0, 9)), vec![("a".into(), Notify::Blocked)]);
        d.observe("a", AgentStatus::Blocked, at(t0, 10));
        assert!(d.due(at(t0, 20)).is_empty(), "same status again");
        d.observe("a", AgentStatus::Done, at(t0, 21));
        assert_eq!(d.due(at(t0, 25)), vec![("a".into(), Notify::Finished)]);
        d.observe("a", AgentStatus::Idle, at(t0, 26));
        assert!(d.due(at(t0, 30)).is_empty(), "done to idle is not news");
    }

    #[test]
    fn flapping_status_is_debounced() {
        let t0 = Instant::now();
        let mut d = Debouncer::default();
        d.observe("a", AgentStatus::Working, t0);
        d.due(at(t0, 4));
        // Claude shows blocked while its hook runs; a short blip must not notify.
        d.observe("a", AgentStatus::Blocked, at(t0, 5));
        d.observe("a", AgentStatus::Working, at(t0, 6));
        assert!(d.due(at(t0, 10)).is_empty());
    }

    #[test]
    fn a_turn_shorter_than_hold_still_finishes() {
        let t0 = Instant::now();
        let mut d = Debouncer::default();
        d.observe("a", AgentStatus::Idle, t0);
        assert!(d.due(at(t0, 4)).is_empty());
        d.observe("a", AgentStatus::Working, at(t0, 5));
        d.observe("a", AgentStatus::Idle, at(t0, 6));
        assert_eq!(d.due(at(t0, 10)), vec![("a".into(), Notify::Finished)]);
        d.observe("a", AgentStatus::Done, at(t0, 11));
        assert!(d.due(at(t0, 15)).is_empty(), "no activity since");
    }

    #[test]
    fn already_blocked_at_startup_notifies() {
        let t0 = Instant::now();
        let mut d = Debouncer::default();
        d.observe("a", AgentStatus::Blocked, t0);
        d.observe("b", AgentStatus::Idle, t0);
        assert_eq!(d.due(at(t0, 4)), vec![("a".into(), Notify::Blocked)]);
    }

    fn msg(text: &str, about: Option<&str>) -> Outgoing {
        Outgoing {
            to: "447700900123".into(),
            text: text.into(),
            about: about.map(Into::into),
        }
    }

    #[test]
    fn outbox_rate_limits_and_refills() {
        let t0 = Instant::now();
        let mut o = Outbox::new(t0);
        for i in 0..8 {
            o.push(msg(&format!("r{i}"), None));
        }
        let sent: Vec<_> = std::iter::from_fn(|| o.pop_ready(t0)).collect();
        assert_eq!(sent.len(), BURST as usize);
        assert!(o.pop_ready(at(t0, 11)).is_none());
        assert_eq!(o.pop_ready(at(t0, 12)).unwrap().text, "r5");
        assert!(o.pop_ready(at(t0, 13)).is_none());
    }

    #[test]
    fn replies_go_first_and_notifications_coalesce() {
        let t0 = Instant::now();
        let mut o = Outbox::new(t0);
        o.push(msg("a blocked", Some("a")));
        o.push(msg("b finished", Some("b")));
        o.push(msg("a finished", Some("a")));
        o.push(msg("reply", None));
        let order: Vec<_> = std::iter::from_fn(|| o.pop_ready(t0))
            .map(|m| m.text)
            .collect();
        assert_eq!(order, ["reply", "b finished", "a finished"]);
    }

    #[test]
    fn outbox_is_bounded() {
        let t0 = Instant::now();
        let mut o = Outbox::new(t0);
        for i in 0..(MAX_QUEUE + 10) {
            o.push(msg(&format!("n{i}"), Some(&format!("agent{i}"))));
        }
        assert_eq!(o.len(), MAX_QUEUE);
        assert_eq!(
            o.pop_ready(t0).unwrap().text,
            "n10",
            "oldest notifications dropped"
        );
    }
}
