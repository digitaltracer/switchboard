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

pub(super) fn insert(c: &rusqlite::Connection, a: &AttentionRecord) -> Result<AttentionRecord> {
    c.execute(
        "INSERT INTO attention (id, machine_id, herdr_session, agent, terminal_id, session_id,
         occurrence, created_at, expires_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)
         ON CONFLICT (machine_id, agent, occurrence) DO NOTHING",
        params![
            a.id,
            a.machine_id,
            a.herdr_session,
            a.agent,
            a.terminal_id,
            a.session_id,
            a.occurrence,
            a.created_at,
            a.expires_at
        ],
    )?;
    Ok(c.query_row(
        "SELECT * FROM attention WHERE machine_id = ?1 AND agent = ?2 AND occurrence = ?3",
        params![a.machine_id, a.agent, a.occurrence],
        record,
    )?)
}

impl Store {
    /// Records an attention unless one exists for its agent and occurrence; returns the one kept.
    pub async fn insert_attention(&self, a: AttentionRecord) -> Result<AttentionRecord> {
        self.call(move |c| {
            let tx = c.transaction()?;
            let kept = insert(&tx, &a)?;
            tx.commit()?;
            Ok(kept)
        })
        .await
    }

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
