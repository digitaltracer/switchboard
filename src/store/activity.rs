//! What agents did while no one was looking: their turns as the hub saw them, and hub settings.

use tokio_rusqlite::rusqlite::{OptionalExtension, params};

use super::{Result, Store};

/// Events are kept this long.
pub const KEEP_MS: i64 = 7 * 24 * 60 * 60 * 1000;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentEvent {
    pub machine_id: String,
    pub agent: String,
    pub session_id: Option<String>,
    /// `working`, `finished`, `waiting` or `error`.
    pub kind: String,
    pub at: i64,
}

/// An approval a session's agent asked for, and what became of it.
#[derive(Debug, Clone, PartialEq)]
pub struct ApprovalFact {
    pub session_id: String,
    pub machine_id: String,
    pub agent: Option<String>,
    pub tool_name: String,
    pub tool_input: String,
    pub created_at: i64,
    /// `allow` or `deny`, once decided.
    pub outcome: Option<String>,
    pub reason: Option<String>,
    pub decided_at: Option<i64>,
}

impl Store {
    /// Records one event and drops those older than a week.
    pub async fn record_agent_event(&self, e: AgentEvent) -> Result<()> {
        self.call(move |c| {
            c.execute(
                "INSERT INTO agent_events (machine_id, agent, session_id, kind, at) VALUES (?1, ?2, ?3, ?4, ?5)",
                params![e.machine_id, e.agent, e.session_id, e.kind, e.at],
            )?;
            c.execute("DELETE FROM agent_events WHERE at < ?1", [e.at - KEEP_MS])?;
            Ok(())
        })
        .await
    }

    pub async fn agent_events_since(&self, since: i64) -> Result<Vec<AgentEvent>> {
        self.call(move |c| {
            let mut stmt = c.prepare(
                "SELECT machine_id, agent, session_id, kind, at FROM agent_events WHERE at >= ?1 ORDER BY at, id",
            )?;
            let rows = stmt
                .query_map([since], |r| {
                    Ok(AgentEvent {
                        machine_id: r.get(0)?,
                        agent: r.get(1)?,
                        session_id: r.get(2)?,
                        kind: r.get(3)?,
                        at: r.get(4)?,
                    })
                })?
                .collect::<std::result::Result<Vec<_>, _>>()?;
            Ok(rows)
        })
        .await
    }

    /// Approvals asked for or decided since `since`.
    pub async fn approval_facts_since(&self, since: i64) -> Result<Vec<ApprovalFact>> {
        self.call(move |c| {
            let mut stmt = c.prepare(
                "SELECT a.session_id, s.machine_id, s.agent_name, a.tool_name, a.tool_input, a.created_at,
                        a.outcome, a.reason, a.decided_at
                 FROM approvals a JOIN sessions s ON s.id = a.session_id
                 WHERE a.created_at >= ?1 OR a.decided_at >= ?1 ORDER BY a.created_at",
            )?;
            let rows = stmt
                .query_map([since], |r| {
                    Ok(ApprovalFact {
                        session_id: r.get(0)?,
                        machine_id: r.get(1)?,
                        agent: r.get(2)?,
                        tool_name: r.get(3)?,
                        tool_input: r.get(4)?,
                        created_at: r.get(5)?,
                        outcome: r.get(6)?,
                        reason: r.get(7)?,
                        decided_at: r.get(8)?,
                    })
                })?
                .collect::<std::result::Result<Vec<_>, _>>()?;
            Ok(rows)
        })
        .await
    }

    pub async fn setting(&self, key: &str) -> Result<Option<String>> {
        let key = key.to_owned();
        self.call(move |c| {
            Ok(c.query_row(
                "SELECT value FROM hub_settings WHERE key = ?1",
                [key],
                |r| r.get(0),
            )
            .optional()?)
        })
        .await
    }

    pub async fn set_setting(&self, key: &str, value: &str) -> Result<()> {
        let (key, value) = (key.to_owned(), value.to_owned());
        self.call(move |c| {
            c.execute(
                "INSERT INTO hub_settings (key, value) VALUES (?1, ?2)
                 ON CONFLICT (key) DO UPDATE SET value = ?2",
                params![key, value],
            )?;
            Ok(())
        })
        .await
    }
}
