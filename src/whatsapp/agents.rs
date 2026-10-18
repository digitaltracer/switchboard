//! What the bridge may do to agents: list, read and prompt. There is no key path.
//! The hub's session service can replace `HerdrAgents` behind the same trait later.

use std::future::Future;

use crate::herdr::types::{AgentPromptParams, AgentStatus, ReadSource};
use crate::herdr::{Client, HerdrError};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentSummary {
    pub name: Option<String>,
    pub pane_id: String,
    pub kind: Option<String>,
    pub status: AgentStatus,
}

impl AgentSummary {
    pub fn label(&self) -> &str {
        self.name.as_deref().unwrap_or(&self.pane_id)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PromptOutcome {
    Sent,
    /// Herdr refused because the agent sits at a dialog; nothing was sent.
    Blocked {
        screen: String,
    },
    NotFound,
}

pub trait AgentControl: Send + Sync + 'static {
    fn list(&self) -> impl Future<Output = Result<Vec<AgentSummary>, String>> + Send;
    fn read(&self, target: &str, lines: u32)
    -> impl Future<Output = Result<String, String>> + Send;
    /// The currently visible screen, which is what shows a dialog.
    fn screen(&self, target: &str) -> impl Future<Output = Result<String, String>> + Send;
    fn prompt(
        &self,
        target: &str,
        text: &str,
    ) -> impl Future<Output = Result<PromptOutcome, String>> + Send;
}

pub struct HerdrAgents {
    herdr: Client,
}

impl HerdrAgents {
    pub fn new(herdr: Client) -> Self {
        Self { herdr }
    }
}

impl AgentControl for HerdrAgents {
    async fn list(&self) -> Result<Vec<AgentSummary>, String> {
        let snap = self
            .herdr
            .session_snapshot()
            .await
            .map_err(|e| e.to_string())?;
        Ok(snap
            .agents
            .into_iter()
            .map(|a| AgentSummary {
                name: a.name,
                pane_id: a.pane_id,
                kind: a.agent,
                status: a.agent_status,
            })
            .collect())
    }

    async fn read(&self, target: &str, lines: u32) -> Result<String, String> {
        // Herdr refuses history reads while an agent sits at a dialog.
        match self
            .herdr
            .agent_read(target, ReadSource::RecentUnwrapped, Some(lines))
            .await
        {
            Err(e) if e.server_code() == Some("agent_not_idle") => self.screen(target).await,
            other => other.map(|r| r.text).map_err(|e| e.to_string()),
        }
    }

    async fn screen(&self, target: &str) -> Result<String, String> {
        self.herdr
            .agent_read(target, ReadSource::Visible, None)
            .await
            .map(|r| r.text)
            .map_err(|e| e.to_string())
    }

    async fn prompt(&self, target: &str, text: &str) -> Result<PromptOutcome, String> {
        let params = AgentPromptParams {
            target: target.to_owned(),
            text: text.to_owned(),
            wait: None,
        };
        match self.herdr.agent_prompt(params).await {
            Ok(_) => Ok(PromptOutcome::Sent),
            Err(e) if e.server_code() == Some("agent_blocked") => Ok(PromptOutcome::Blocked {
                screen: self.screen(target).await.unwrap_or_default(),
            }),
            Err(e) if e.server_code() == Some("agent_not_found") => Ok(PromptOutcome::NotFound),
            Err(e @ HerdrError::Io { .. }) | Err(e @ HerdrError::Timeout { .. }) => {
                Err(format!("{e} (it may or may not have been delivered)"))
            }
            Err(e) => Err(e.to_string()),
        }
    }
}
