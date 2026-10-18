//! Local mirror of Herdr pane state: installed from `session.snapshot`, advanced by events.

use std::collections::{HashMap, HashSet};

use super::types::{AgentStatus, Event, SessionSnapshot, Subscription};
use super::{Client, EventStream, HerdrError, PROTOCOL, Result};

#[derive(Debug, Clone, PartialEq)]
pub struct PaneView {
    pub workspace_id: String,
    pub agent_status: AgentStatus,
    pub agent: Option<String>,
    pub exited: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Change {
    Status {
        pane_id: String,
        from: Option<AgentStatus>,
        to: AgentStatus,
    },
    PaneExited {
        pane_id: String,
    },
    PaneGone {
        pane_id: String,
    },
    WorkspaceGone {
        workspace_id: String,
    },
}

#[derive(Debug, Default)]
pub struct Mirror {
    panes: HashMap<String, PaneView>,
    workspaces: HashSet<String>,
    synced: bool,
}

impl Mirror {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn pane(&self, pane_id: &str) -> Option<&PaneView> {
        self.panes.get(pane_id)
    }

    pub fn has_workspace(&self, workspace_id: &str) -> bool {
        self.workspaces.contains(workspace_id)
    }

    /// False until a snapshot is installed, and again after `mark_lost`.
    pub fn is_synced(&self) -> bool {
        self.synced
    }

    pub fn mark_lost(&mut self) {
        self.synced = false;
    }

    /// Replaces the mirror with a snapshot and reports what changed against the old state.
    pub fn install(&mut self, snap: &SessionSnapshot) -> Vec<Change> {
        let mut panes: HashMap<String, PaneView> = snap
            .panes
            .iter()
            .map(|p| {
                let view = PaneView {
                    workspace_id: p.workspace_id.clone(),
                    agent_status: p.agent_status,
                    agent: p.agent.clone(),
                    exited: false,
                };
                (p.pane_id.clone(), view)
            })
            .collect();
        for a in &snap.agents {
            if let Some(view) = panes.get_mut(&a.pane_id) {
                view.agent_status = a.agent_status;
                view.agent = a.agent.clone().or(view.agent.take());
            }
        }
        let workspaces: HashSet<String> = snap
            .workspaces
            .iter()
            .map(|w| w.workspace_id.clone())
            .collect();

        let mut changes = Vec::new();
        let mut gone_ws: Vec<_> = self.workspaces.difference(&workspaces).cloned().collect();
        gone_ws.sort();
        changes.extend(
            gone_ws
                .into_iter()
                .map(|workspace_id| Change::WorkspaceGone { workspace_id }),
        );
        let mut old: Vec<_> = self.panes.iter().collect();
        old.sort_by(|a, b| a.0.cmp(b.0));
        for (id, before) in old {
            match panes.get(id) {
                None => changes.push(Change::PaneGone {
                    pane_id: id.clone(),
                }),
                Some(now) if now.agent_status != before.agent_status => {
                    changes.push(Change::Status {
                        pane_id: id.clone(),
                        from: Some(before.agent_status),
                        to: now.agent_status,
                    })
                }
                Some(_) => {}
            }
        }
        self.panes = panes;
        self.workspaces = workspaces;
        self.synced = true;
        changes
    }

    pub fn apply(&mut self, event: &Event) -> Vec<Change> {
        match event {
            Event::AgentStatusChanged {
                pane_id,
                workspace_id,
                agent_status,
            } => {
                let view = self
                    .panes
                    .entry(pane_id.clone())
                    .or_insert_with(|| PaneView {
                        workspace_id: workspace_id.clone(),
                        agent_status: AgentStatus::Unknown,
                        agent: None,
                        exited: false,
                    });
                if view.agent_status == *agent_status {
                    return vec![];
                }
                let from = Some(std::mem::replace(&mut view.agent_status, *agent_status));
                vec![Change::Status {
                    pane_id: pane_id.clone(),
                    from,
                    to: *agent_status,
                }]
            }
            Event::AgentDetected { pane_id, agent } => {
                if let Some(view) = self.panes.get_mut(pane_id) {
                    view.agent = agent.clone();
                }
                vec![]
            }
            Event::PaneCreated(p) => {
                self.workspaces.insert(p.workspace_id.clone());
                self.panes.insert(
                    p.pane_id.clone(),
                    PaneView {
                        workspace_id: p.workspace_id.clone(),
                        agent_status: p.agent_status,
                        agent: p.agent.clone(),
                        exited: false,
                    },
                );
                vec![]
            }
            Event::PaneExited { pane_id, .. } => match self.panes.get_mut(pane_id) {
                Some(view) if !view.exited => {
                    view.exited = true;
                    vec![Change::PaneExited {
                        pane_id: pane_id.clone(),
                    }]
                }
                _ => vec![],
            },
            Event::PaneClosed { pane_id, .. } => match self.panes.remove(pane_id) {
                Some(_) => vec![Change::PaneGone {
                    pane_id: pane_id.clone(),
                }],
                None => vec![],
            },
            Event::WorkspaceClosed { workspace_id }
            | Event::WorktreeRemoved { workspace_id, .. } => self.remove_workspace(workspace_id),
            Event::Other(_) => vec![],
        }
    }

    fn remove_workspace(&mut self, workspace_id: &str) -> Vec<Change> {
        if !self.workspaces.remove(workspace_id) {
            return vec![];
        }
        let mut gone: Vec<String> = self
            .panes
            .iter()
            .filter(|(_, v)| v.workspace_id == workspace_id)
            .map(|(k, _)| k.clone())
            .collect();
        gone.sort();
        let mut changes = vec![Change::WorkspaceGone {
            workspace_id: workspace_id.to_owned(),
        }];
        for pane_id in gone {
            self.panes.remove(&pane_id);
            changes.push(Change::PaneGone { pane_id });
        }
        changes
    }
}

impl Client {
    /// Bootstrap without a gap, as Herdr documents it: subscribe and wait for the ack, take a
    /// snapshot, install it, then read buffered events from the returned stream in order.
    /// Call again whenever the stream ends.
    pub async fn sync(
        &self,
        mirror: &mut Mirror,
        subscriptions: Vec<Subscription>,
    ) -> Result<(EventStream, Vec<Change>)> {
        mirror.mark_lost();
        let stream = self.subscribe(subscriptions).await?;
        let snap = self.session_snapshot().await?;
        if snap.protocol != PROTOCOL {
            return Err(HerdrError::Incompatible {
                version: snap.version,
                protocol: snap.protocol,
            });
        }
        let changes = mirror.install(&snap);
        Ok((stream, changes))
    }
}
