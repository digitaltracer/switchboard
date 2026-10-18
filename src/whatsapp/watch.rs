//! Status of every named agent from Herdr: per-pane status subscriptions plus snapshot
//! reconciliation. New agents are picked up without a restart; whether they notify is the
//! relay's decision. A stream that ends means events may be lost: resubscribe and reconcile.

use std::collections::BTreeMap;
use std::time::Duration;

use tokio::sync::mpsc;
use tokio::time::Instant;

use crate::herdr::types::{AgentStatus, Event, SessionSnapshot, Subscription};
use crate::herdr::{Client, HerdrError};
use crate::proto::conn::Backoff;

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
#[path = "watch_tests.rs"]
mod tests;
