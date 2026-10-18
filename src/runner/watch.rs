//! Follows Herdr for owned sessions: forwards agent status and runs the Codex deny fallback.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::Duration;

use tokio::time::Instant;

use super::codex::{CodexPane, Facts, FallbackOutcome, Gate, deny_dialog, gate};
use super::sessions::{RunnerSession, Sessions};
use crate::cli::Agent;
use crate::herdr::Mirror;
use crate::herdr::types::{AgentStatus, Subscription};
use crate::proto::Event;
use crate::proto::conn::Backoff;

/// Snapshot reconciliation interval; also how often blocked Codex panes are re-checked.
pub const TICK: Duration = Duration::from_secs(2);

pub async fn watch(sessions: Arc<Sessions>) {
    let mut backoff = Backoff::default();
    let mut tracker = Tracker::default();
    loop {
        let mut mirror = Mirror::new();
        let mut stream = match sessions
            .herdr
            .sync(&mut mirror, Subscription::lifecycle())
            .await
        {
            Ok((stream, _)) => {
                backoff.reset();
                stream
            }
            Err(e) => {
                tracing::warn!(error = %e, "herdr sync failed");
                tokio::time::sleep(backoff.next_delay()).await;
                continue;
            }
        };
        sessions.emit(Event::Resynced {
            sessions: sessions.owned().await,
        });
        tracker.observe(&sessions, &mirror).await;
        let mut tick = tokio::time::interval(TICK);
        loop {
            tokio::select! {
                ev = stream.next() => match ev {
                    None => break,
                    Some(Ok(ev)) => { mirror.apply(&ev); }
                    Some(Err(e)) => tracing::warn!(error = %e, "bad herdr event"),
                },
                _ = tick.tick() => match sessions.herdr.session_snapshot().await {
                    Ok(snap) => { mirror.install(&snap); }
                    Err(e) => {
                        tracing::warn!(error = %e, "herdr snapshot failed");
                        break;
                    }
                },
            }
            tracker.observe(&sessions, &mirror).await;
        }
        tracing::info!("herdr event stream ended; resyncing");
        tokio::time::sleep(backoff.next_delay()).await;
    }
}

#[derive(Default)]
struct Tracker {
    status: HashMap<String, AgentStatus>,
    blocked_since: HashMap<String, Instant>,
    /// Sessions whose pane has appeared in Herdr; only those can be reported as exited.
    seen: HashSet<String>,
    /// Sessions whose pane has shown an agent; the agent leaving the pane is an exit.
    had_agent: HashSet<String>,
    exited: HashSet<String>,
}

impl Tracker {
    async fn observe(&mut self, sessions: &Sessions, mirror: &Mirror) {
        for s in sessions.all() {
            let id = s.session_id.clone();
            let Some(pane_id) = s.pane_id.as_deref() else {
                continue;
            };
            let view = mirror.pane(pane_id);
            if view.is_some() {
                self.seen.insert(id.clone());
            }
            if view.is_some_and(|v| v.agent.is_some()) {
                self.had_agent.insert(id.clone());
            }
            let agent_left =
                view.is_some_and(|v| v.agent.is_none()) && self.had_agent.contains(&id);
            if view.is_none_or(|v| v.exited) || agent_left {
                if mirror.is_synced() && self.seen.contains(&id) && self.exited.insert(id.clone()) {
                    sessions.emit(Event::SessionExited { session_id: id });
                }
                continue;
            }
            let status = view.map_or(AgentStatus::Unknown, |v| v.agent_status);
            if self.status.insert(id.clone(), status) != Some(status) {
                sessions.emit(Event::AgentStatus {
                    session_id: id.clone(),
                    agent_status: status_str(status),
                    revision: 0,
                });
            }
            if status == AgentStatus::Blocked {
                self.blocked_since
                    .entry(id.clone())
                    .or_insert_with(Instant::now);
            } else {
                self.blocked_since.remove(&id);
            }
            if s.agent == Agent::Codex {
                self.fallback(sessions, &s).await;
            }
        }
    }

    async fn fallback(&mut self, sessions: &Sessions, s: &RunnerSession) {
        let Some(hooks) = sessions.hooks.state(&s.session_id) else {
            return;
        };
        let facts = Facts {
            agent: s.agent,
            codex_version: s.codex_version.as_deref(),
            blocked_since: self.blocked_since.get(&s.session_id).copied(),
            hooks,
            now: Instant::now(),
        };
        match gate(&facts) {
            Gate::Wait(_) | Gate::Refuse(_) => {}
            Gate::HookFailed(reason) => sessions.hook_failed(s, reason, None),
            Gate::ReadScreen => {
                let Some(pane) = CodexPane::new(&sessions.herdr, s.agent, &s.agent_name) else {
                    return;
                };
                let version = s.codex_version.as_deref().unwrap_or_default();
                match deny_dialog(&pane, version).await {
                    FallbackOutcome::Denied { screen_excerpt } => {
                        sessions.fallback_denied(s, screen_excerpt);
                        self.blocked_since.remove(&s.session_id);
                    }
                    FallbackOutcome::NothingToDo => {}
                    FallbackOutcome::HookFailed {
                        reason,
                        screen_excerpt,
                    } => {
                        sessions.hook_failed(s, reason, Some(screen_excerpt));
                    }
                }
            }
        }
    }
}

fn status_str(status: AgentStatus) -> String {
    serde_json::to_value(status)
        .ok()
        .and_then(|v| v.as_str().map(str::to_owned))
        .unwrap_or_else(|| "unknown".into())
}
