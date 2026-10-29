//! Agents waiting at a dialog, as a `waiting` push names them by an opaque id.

use tokio_rusqlite::rusqlite::{self, OptionalExtension, Row, params};

use super::{Result, Store};

#[derive(Debug, Clone, PartialEq)]
pub struct AttentionRecord {
    pub id: String,
    pub machine_id: String,
    pub herdr_session: Option<String>,
    pub agent: String,
    /// The pane incarnation the dialog was in.
    pub terminal_id: String,
    pub session_id: Option<String>,
    pub occurrence: String,
    pub created_at: i64,
    pub expires_at: i64,
    pub resolved_at: Option<i64>,
    pub resolution: Option<String>,
}

fn record(r: &Row) -> rusqlite::Result<AttentionRecord> {
    Ok(AttentionRecord {
        id: r.get("id")?,
        machine_id: r.get("machine_id")?,
        herdr_session: r.get("herdr_session")?,
        agent: r.get("agent")?,
        terminal_id: r.get("terminal_id")?,
        session_id: r.get("session_id")?,
        occurrence: r.get("occurrence")?,
        created_at: r.get("created_at")?,
        expires_at: r.get("expires_at")?,
        resolved_at: r.get("resolved_at")?,
        resolution: r.get("resolution")?,
    })
}

impl Store {
    pub async fn get_attention(&self, id: &str) -> Result<Option<AttentionRecord>> {
        let id = id.to_owned();
        self.call(move |c| {
            Ok(
                c.query_row("SELECT * FROM attention WHERE id = ?1", [id], record)
                    .optional()?,
            )
        })
        .await
    }

    /// Resolves the agent's open attentions for `occurrence`, or all of them when it's `None`.
    pub async fn resolve_attention(
        &self,
        machine_id: &str,
        agent: &str,
        occurrence: Option<&str>,
        resolution: &str,
        now: i64,
    ) -> Result<usize> {
        let args = (
            machine_id.to_owned(),
            agent.to_owned(),
            occurrence.map(str::to_owned),
            resolution.to_owned(),
        );
        self.call(move |c| {
            Ok(c.execute(
                "UPDATE attention SET resolved_at = ?5, resolution = ?4
                 WHERE machine_id = ?1 AND agent = ?2 AND (?3 IS NULL OR occurrence = ?3)
                 AND resolved_at IS NULL",
                params![args.0, args.1, args.2, args.3, now],
            )?)
        })
        .await
    }
}
