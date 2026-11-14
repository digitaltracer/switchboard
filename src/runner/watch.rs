//! Follows Herdr and herdr-core for owned sessions: forwards agent status, reports exits and runs
//! the Codex deny fallback.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::Duration;

use tokio::time::Instant;

use super::agents::status_str;
use super::backend::Server;
use super::codex::{CodexPane, Facts, FallbackOutcome, Gate, deny_dialog, gate};
use super::core_watch::{self, Core};
use super::sessions::{RunnerSession, SESSION_LABEL, Sessions};
use crate::cli::Agent;
use crate::herdr::types::{AgentStatus, Subscription};
use crate::herdr::{Mirror, PaneView};
use crate::proto::Event;
use crate::proto::conn::Backoff;

/// Snapshot reconciliation interval; also how often blocked Codex panes are re-checked.
pub const TICK: Duration = Duration::from_secs(2);
/// How long a Herdr server may refuse connections before its tasks count as exited. Its panes,
/// and the agents in them, go with it.
pub const SERVER_GONE_AFTER: Duration = Duration::from_secs(30);
/// How often a server whose tasks have already exited is tried again, quietly.
pub const GONE_RETRY: Duration = Duration::from_secs(60);

/// True once a server has refused connections long enough that its tasks count as exited.
fn gone_for_good(server_gone: bool, gone_since: Option<Instant>) -> bool {
    server_gone && gone_since.is_some_and(|since| since.elapsed() >= SERVER_GONE_AFTER)
}

/// Follows every Herdr server that hosts a task, one watcher each, as tasks come and go.
pub async fn watch(sessions: Arc<Sessions>) {
    let mut watchers: HashMap<Option<String>, tokio::task::JoinHandle<()>> = HashMap::new();
    loop {
        let wanted: HashSet<Option<String>> = if sessions.herdrs.is_multi() {
            sessions
                .all()
                .into_iter()
                .filter(|s| !on_core(s))
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
                let gone = gone_for_good(e.server_gone(), gone_since);
                if gone {
                    tracing::debug!(error = %e, herdr_session = ?server, "herdr sync failed");
                } else {
                    tracing::warn!(error = %e, herdr_session = ?server, "herdr sync failed");
                }
                if e.server_gone() {
                    let since = *gone_since.get_or_insert_with(Instant::now);
                    if since.elapsed() >= SERVER_GONE_AFTER {
                        tracker.server_gone(&sessions);
                    }
                } else {
                    gone_since = None;
                }
                let delay = if gone {
                    GONE_RETRY
                } else {
                    backoff.next_delay()
                };
                tokio::time::sleep(delay).await;
                continue;
            }
        };
        sessions.emit(Event::Resynced {
            sessions: sessions.owned().await,
        });
        let srv = Server::herdr(server.clone(), herdr.clone());
        tracker
            .observe(&sessions, &srv, mirror.is_synced(), |s| {
                s.pane_id.as_deref().and_then(|p| mirror.pane(p)).cloned()
            })
            .await;
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
            tracker
                .observe(&sessions, &srv, mirror.is_synced(), |s| {
                    s.pane_id.as_deref().and_then(|p| mirror.pane(p)).cloned()
                })
                .await;
        }
        tracing::info!(herdr_session = ?server, "herdr event stream ended; resyncing");
        tokio::time::sleep(backoff.next_delay()).await;
    }
}

fn on_core(s: &RunnerSession) -> bool {
    s.herdr_session.as_deref() == Some(core_watch::SERVER)
}

/// Follows the tasks on herdr-core from its mirror. A task's terminal counts only on the core it
/// started on, with its identity and label unchanged; after a runner restart a missing one exits.
pub async fn watch_core(sessions: Arc<Sessions>, core: Arc<Core>) {
    let mut tracker = Tracker {
        server: Some(core_watch::SERVER.into()),
        core: true,
        ..Default::default()
    };
    let mut gone_since: Option<Instant> = None;
    let mut tick = tokio::time::interval(TICK);
    loop {
        tick.tick().await;
        let Some(core_id) = core.core_id() else {
            let since = *gone_since.get_or_insert_with(Instant::now);
            if since.elapsed() >= SERVER_GONE_AFTER {
                tracker.server_gone(&sessions);
            }
            continue;
        };
        gone_since = None;
        if !core.is_synced() {
            continue;
        }
        let facts = core.facts().unwrap_or_default();
        for s in sessions
            .all()
            .into_iter()
            .filter(|s| on_core(s) && s.launched)
        {
            tracker.seen.insert(s.session_id);
        }
        let server = Server::Core(core.clone());
        tracker
            .observe(&sessions, &server, true, |s| {
                let t = core.terminal(s.pane_id.as_deref()?)?;
                let same = s.core_id.as_deref() == Some(core_id.as_str())
                    && s.terminal_identity.as_deref() == Some(t.identity(&core_id).as_str())
                    && t.labels.get(SESSION_LABEL) == Some(&s.session_id);
                if !same {
                    return None;
                }
                let f = facts.iter().find(|f| f.target == t.address);
                Some(PaneView {
                    workspace_id: s.workspace_id.clone().unwrap_or_default(),
                    agent_status: f.map_or(AgentStatus::Unknown, |f| f.status),
                    agent: f.and_then(|f| f.kind.clone()),
                    exited: !t.is_live(),
                })
            })
            .await;
    }
}

#[derive(Default)]
struct Tracker {
    /// The server this tracker watches; tasks in other servers are not its business.
    server: Option<String>,
    /// It watches herdr-core, whose tasks no Herdr tracker follows.
    core: bool,
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
    fn mine(&self, sessions: &Sessions, s: &RunnerSession) -> bool {
        if self.core {
            on_core(s)
        } else {
            !on_core(s) && (!sessions.herdrs.is_multi() || s.herdr_session == self.server)
        }
    }

    fn server_gone(&mut self, sessions: &Sessions) {
        for s in sessions.all() {
            if self.mine(sessions, &s) && s.launched && self.exited.insert(s.session_id.clone()) {
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
        server: &Server,
        synced: bool,
        view_of: impl Fn(&RunnerSession) -> Option<PaneView>,
    ) {
        let mine: Vec<RunnerSession> = sessions
            .all()
            .into_iter()
            .filter(|s| self.mine(sessions, s))
            .collect();
        for s in mine {
            let id = s.session_id.clone();
            if s.pane_id.is_none() {
                continue;
            }
            let view = view_of(&s);
            let view = view.as_ref();
            if view.is_some() {
                self.seen.insert(id.clone());
            }
            if view.is_some_and(|v| v.agent.is_some()) {
                self.had_agent.insert(id.clone());
            }
            let agent_left =
                view.is_some_and(|v| v.agent.is_none()) && self.had_agent.contains(&id);
            if view.is_none_or(|v| v.exited) || agent_left {
                if synced && self.seen.contains(&id) && self.exited.insert(id.clone()) {
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
                self.fallback(sessions, server, &s).await;
            }
        }
    }

    async fn fallback(&mut self, sessions: &Sessions, server: &Server, s: &RunnerSession) {
        let Some(hooks) = sessions.hooks.state(&s.session_id) else {
            return;
        };
        let facts = Facts {
            agent: s.agent,
            blocked_since: self.blocked_since.get(&s.session_id).copied(),
            hooks,
            now: Instant::now(),
        };
        match gate(&facts) {
            Gate::Wait(_) | Gate::Refuse(_) => {}
            Gate::HookFailed(reason) => sessions.hook_failed(s, reason, None),
            Gate::ReadScreen => {
                let Some(pane) = CodexPane::on(server.clone(), s.agent, &s.agent_name) else {
                    return;
                };
                let pane_id = s.pane_id.as_deref().unwrap_or_default();
                let _lock = sessions
                    .pane_locks
                    .lock(s.herdr_session.as_deref(), pane_id)
                    .await;
                match deny_dialog(&pane).await {
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

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test(start_paused = true)]
    async fn a_server_counts_as_gone_for_good_only_after_refusing_long_enough() {
        let since = Instant::now();
        assert!(!gone_for_good(true, None));
        assert!(!gone_for_good(true, Some(since)));
        tokio::time::advance(SERVER_GONE_AFTER).await;
        assert!(gone_for_good(true, Some(since)));
        assert!(
            !gone_for_good(false, Some(since)),
            "a server that answers isn't gone"
        );
    }
}
