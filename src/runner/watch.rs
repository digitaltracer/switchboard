//! Follows Herdr for owned sessions: forwards agent status and runs the Codex deny fallback.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::Duration;

use tokio::time::Instant;

use super::agents::status_str;
use super::codex::{CodexPane, Facts, FallbackOutcome, Gate, deny_dialog, gate};
use super::sessions::{RunnerSession, Sessions};
use crate::cli::Agent;
use crate::herdr::Mirror;
use crate::herdr::types::{AgentStatus, Subscription};
use crate::proto::Event;
use crate::proto::conn::Backoff;

/// Snapshot reconciliation interval; also how often blocked Codex panes are re-checked.
pub const TICK: Duration = Duration::from_secs(2);
/// How long a Herdr server may refuse connections before its tasks count as exited. Its panes,
/// and the agents in them, go with it.
pub const SERVER_GONE_AFTER: Duration = Duration::from_secs(30);

/// Follows every Herdr server that hosts a task, one watcher each, as tasks come and go.
pub async fn watch(sessions: Arc<Sessions>) {
    let mut watchers: HashMap<Option<String>, tokio::task::JoinHandle<()>> = HashMap::new();
    loop {
        let wanted: HashSet<Option<String>> = if sessions.herdrs.is_multi() {
            sessions
                .all()
                .into_iter()
                .map(|s| s.herdr_session)
                .collect()
        } else {
            HashSet::from([None])
        };
        watchers.retain(|server, task| {
            let keep = wanted.contains(server) && !task.is_finished();
            if !keep {
                task.abort();
            }
            keep
        });
        for server in wanted {
            watchers
                .entry(server.clone())
                .or_insert_with(|| tokio::spawn(watch_server(sessions.clone(), server)));
        }
        tokio::time::sleep(TICK).await;
    }
}

/// One Herdr server and the tasks in it; pane ids only mean something within their server.
async fn watch_server(sessions: Arc<Sessions>, server: Option<String>) {
    let herdr = sessions.herdrs.client(server.as_deref());
    let mut backoff = Backoff::default();
    let mut tracker = Tracker {
        server: server.clone(),
        ..Default::default()
    };
    let mut gone_since: Option<Instant> = None;
    loop {
        let mut mirror = Mirror::new();
        let mut stream = match herdr.sync(&mut mirror, Subscription::lifecycle()).await {
            Ok((stream, _)) => {
                backoff.reset();
                gone_since = None;
                stream
            }
            Err(e) => {
                tracing::warn!(error = %e, herdr_session = ?server, "herdr sync failed");
                if e.server_gone() {
                    let since = *gone_since.get_or_insert_with(Instant::now);
                    if since.elapsed() >= SERVER_GONE_AFTER {
                        tracker.server_gone(&sessions);
                    }
                } else {
                    gone_since = None;
                }
                tokio::time::sleep(backoff.next_delay()).await;
                continue;
            }
        };
        sessions.emit(Event::Resynced {
            sessions: sessions.owned().await,
        });
        tracker.observe(&sessions, &herdr, &mirror).await;
        let mut tick = tokio::time::interval(TICK);
        loop {
            tokio::select! {
                ev = stream.next() => match ev {
                    None => break,
                    Some(Ok(ev)) => { mirror.apply(&ev); }
                    Some(Err(e)) => tracing::warn!(error = %e, "bad herdr event"),
                },
                _ = tick.tick() => match herdr.session_snapshot().await {
                    Ok(snap) => { mirror.install(&snap); }
                    Err(e) => {
                        tracing::warn!(error = %e, herdr_session = ?server, "herdr snapshot failed");
                        break;
                    }
                },
            }
            tracker.observe(&sessions, &herdr, &mirror).await;
        }
        tracing::info!(herdr_session = ?server, "herdr event stream ended; resyncing");
        tokio::time::sleep(backoff.next_delay()).await;
    }
}

#[derive(Default)]
struct Tracker {
    /// The Herdr server this tracker watches; tasks in other servers are not its business.
    server: Option<String>,
    status: HashMap<String, AgentStatus>,
    blocked_since: HashMap<String, Instant>,
    /// Sessions whose pane has appeared in Herdr; only those can be reported as exited.
    seen: HashSet<String>,
    /// Sessions whose pane has shown an agent; the agent leaving the pane is an exit.
    had_agent: HashSet<String>,
    exited: HashSet<String>,
}

impl Tracker {
    /// The server is gone, so every task in it has exited, whether or not it was seen running.
    fn server_gone(&mut self, sessions: &Sessions) {
        let multi = sessions.herdrs.is_multi();
        for s in sessions.all() {
            if (!multi || s.herdr_session == self.server)
                && s.launched
                && self.exited.insert(s.session_id.clone())
            {
                tracing::warn!(session_id = %s.session_id, herdr_session = ?self.server, "the task's Herdr server is gone");
                sessions.emit(Event::SessionExited {
                    session_id: s.session_id,
                });
            }
        }
    }

    async fn observe(
        &mut self,
        sessions: &Sessions,
        herdr: &crate::herdr::Client,
        mirror: &Mirror,
    ) {
        let server = self.server.clone();
        let multi = sessions.herdrs.is_multi();
        let mine: Vec<RunnerSession> = sessions
            .all()
            .into_iter()
            .filter(|s| !multi || s.herdr_session == server)
            .collect();
        for s in mine {
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
                self.fallback(sessions, herdr, &s).await;
            }
        }
    }

    async fn fallback(
        &mut self,
        sessions: &Sessions,
        herdr: &crate::herdr::Client,
        s: &RunnerSession,
    ) {
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
                let Some(pane) = CodexPane::new(herdr, s.agent, &s.agent_name) else {
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
