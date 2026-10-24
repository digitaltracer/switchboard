//! Every Herdr agent on this machine, Switchboard's or not: list, read, prompt, and, for named
//! ones, status from per-pane subscriptions plus snapshot reconciliation. An agent without a
//! name goes by its pane id. New agents are picked up
//! without a restart; whether they notify is the bridge's decision. A stream that ends means
//! events may be lost: resubscribe and reconcile. There is no key path here.

use std::collections::{BTreeMap, HashMap};
use std::time::Duration;

use tokio::sync::mpsc;
use tokio::time::Instant;

use crate::herdr::types::{
    AgentPromptParams, AgentStatus, Event, ReadSource, SessionSnapshot, Subscription,
};
use crate::herdr::{Client, HerdrError, check_agent_target};
use crate::proto::conn::Backoff;
use crate::proto::{AgentPromptOutcome, AgentSummary};

/// Every agent, each with its session id when it is one of `sessions` (agent name to id).
pub async fn list(
    herdr: &Client,
    sessions: &HashMap<String, String>,
) -> Result<Vec<AgentSummary>, HerdrError> {
    let snap = herdr.session_snapshot().await?;
    Ok(snap
        .agents
        .into_iter()
        .map(|a| {
            let name = a.name.unwrap_or_else(|| a.pane_id.clone());
            AgentSummary {
                session_id: sessions.get(&name).cloned(),
                herdr_session: None,
                agent_status: status_str(a.agent_status),
                name,
                pane_id: a.pane_id,
                kind: a.agent,
                title: a.terminal_title_stripped.filter(|t| !t.is_empty()),
            }
        })
        .collect())
}

/// The last `lines`, or the visible screen; history reads are refused while at a dialog.
pub async fn read(
    herdr: &Client,
    agent: &str,
    lines: u32,
    visible: bool,
) -> Result<String, HerdrError> {
    check_agent_target(agent)?;
    let screen = || herdr.agent_read(agent, ReadSource::Visible, None);
    if visible {
        return screen().await.map(|r| r.text);
    }
    match herdr
        .agent_read(agent, ReadSource::RecentUnwrapped, Some(lines))
        .await
    {
        Err(e) if e.server_code() == Some("agent_not_idle") => screen().await.map(|r| r.text),
        other => other.map(|r| r.text),
    }
}

/// Herdr refuses a prompt to an agent at a dialog; the screen says what it is waiting for.
pub async fn prompt(
    herdr: &Client,
    agent: &str,
    text: &str,
) -> Result<AgentPromptOutcome, HerdrError> {
    check_agent_target(agent)?;
    let params = AgentPromptParams {
        target: agent.to_owned(),
        text: text.to_owned(),
        wait: None,
    };
    match herdr.agent_prompt(params).await {
        Ok(_) => Ok(AgentPromptOutcome::Sent),
        Err(e) if e.server_code() == Some("agent_blocked") => Ok(AgentPromptOutcome::Blocked {
            screen: read(herdr, agent, 0, true).await.unwrap_or_default(),
        }),
        Err(e) if e.server_code() == Some("agent_not_found") => Ok(AgentPromptOutcome::NotFound),
        Err(e) => Err(e),
    }
}

pub fn status_str(status: AgentStatus) -> String {
    serde_json::to_value(status)
        .ok()
        .and_then(|v| v.as_str().map(str::to_owned))
        .unwrap_or_else(|| "unknown".into())
}

/// How often a snapshot is taken, which is also the longest a new agent goes unnoticed.
pub const RECONCILE: Duration = Duration::from_secs(5);

/// Pane id to agent name for every named live agent.
pub fn named_panes(snap: &SessionSnapshot) -> BTreeMap<String, String> {
    snap.agents
        .iter()
        .filter_map(|a| Some((a.pane_id.clone(), a.name.clone()?)))
        .collect()
}

fn statuses(snap: &SessionSnapshot) -> Vec<(String, AgentStatus)> {
    snap.agents
        .iter()
        .filter_map(|a| Some((a.name.clone()?, a.agent_status)))
        .collect()
}

pub async fn watch(herdr: Client, tx: mpsc::Sender<(String, AgentStatus)>) {
    watch_every(herdr, tx, RECONCILE).await
}

pub async fn watch_every(
    herdr: Client,
    tx: mpsc::Sender<(String, AgentStatus)>,
    reconcile_every: Duration,
) {
    let mut backoff = Backoff::default();
    while !tx.is_closed() {
        match follow(&herdr, &tx, reconcile_every).await {
            Ok(()) => {
                backoff.reset();
                tokio::time::sleep(Duration::from_millis(200)).await;
            }
            Err(e) => {
                tracing::warn!(error = %e, "herdr watch failed");
                tokio::time::sleep(backoff.next_delay()).await;
            }
        }
    }
}

/// One subscription. Returns when the set of named panes changes or the stream ends.
async fn follow(
    herdr: &Client,
    tx: &mpsc::Sender<(String, AgentStatus)>,
    reconcile_every: Duration,
) -> Result<(), HerdrError> {
    let panes = named_panes(&herdr.session_snapshot().await?);
    let mut subs = Subscription::lifecycle();
    subs.extend(panes.keys().map(|p| Subscription::PaneAgentStatusChanged {
        pane_id: p.clone(),
        agent_status: None,
    }));
    let mut stream = herdr.subscribe(subs).await?;
    // Snapshot after the subscription is acknowledged, so nothing falls in between.
    if !reconcile(herdr, &panes, tx).await? {
        return Ok(());
    }
    let mut tick = tokio::time::interval_at(Instant::now() + reconcile_every, reconcile_every);
    loop {
        tokio::select! {
            ev = stream.next() => match ev {
                None => return Ok(()),
                Some(Ok(Event::AgentStatusChanged { pane_id, agent_status, .. })) => {
                    if let Some(name) = panes.get(&pane_id) {
                        let _ = tx.send((name.clone(), agent_status)).await;
                    }
                }
                Some(Ok(Event::Other(_))) => {}
                // A pane or agent came or went: check whether the named set changed.
                Some(Ok(_)) => {
                    if !reconcile(herdr, &panes, tx).await? {
                        return Ok(());
                    }
                }
                Some(Err(e)) => tracing::warn!(error = %e, "bad herdr event"),
            },
            _ = tick.tick() => {
                if !reconcile(herdr, &panes, tx).await? {
                    return Ok(());
                }
            }
        }
    }
}

/// Sends current statuses; false when the named panes no longer match the subscription.
async fn reconcile(
    herdr: &Client,
    panes: &BTreeMap<String, String>,
    tx: &mpsc::Sender<(String, AgentStatus)>,
) -> Result<bool, HerdrError> {
    let snap = herdr.session_snapshot().await?;
    for status in statuses(&snap) {
        let _ = tx.send(status).await;
    }
    Ok(named_panes(&snap) == *panes)
}

#[cfg(test)]
#[path = "agents_tests.rs"]
mod tests;
