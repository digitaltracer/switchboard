//! Herdr agents Switchboard didn't start, as the hub last saw them, so ended ones can be shown.

use std::collections::HashSet;

use tokio_rusqlite::rusqlite::{self, OptionalExtension, Row, params};

use super::{Result, Store};

/// Ended agents are forgotten after this long.
const KEEP_ENDED_MS: i64 = 7 * 24 * 60 * 60 * 1000;

/// One agent from a machine's listing.
#[derive(Debug, Clone, PartialEq)]
pub struct SeenAgent {
    pub herdr_session: Option<String>,
    pub pane_id: String,
    pub kind: Option<String>,
    pub name: String,
    pub title: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct SeenAgentRecord {
    pub machine_id: String,
    pub herdr_session: Option<String>,
    pub pane_id: String,
    pub kind: Option<String>,
    pub name: String,
    pub title: Option<String>,
    pub first_seen: i64,
    pub last_seen: i64,
    pub ended_at: Option<i64>,
}

fn record(r: &Row) -> rusqlite::Result<SeenAgentRecord> {
    Ok(SeenAgentRecord {
        machine_id: r.get(0)?,
        herdr_session: r.get(1)?,
        pane_id: r.get(2)?,
        kind: r.get(3)?,
        name: r.get(4)?,
        title: r.get(5)?,
        first_seen: r.get(6)?,
        last_seen: r.get(7)?,
        ended_at: r.get(8)?,
    })
}

impl Store {
    /// Records a full listing of `machine`'s agents: agents in it are live, any other live one has
    /// ended. A pane that hosts a new agent starts a new record.
    pub async fn record_agents(&self, machine: &str, live: Vec<SeenAgent>, now: i64) -> Result<()> {
        let machine = machine.to_owned();
        self.call(move |c| {
            let tx = c.transaction()?;
            let mut kept = HashSet::new();
            for a in live {
                let id: Option<i64> = tx
                    .query_row(
                        "SELECT id FROM seen_agents WHERE machine_id = ?1 AND herdr_session IS ?2
                         AND pane_id = ?3 AND kind IS ?4 AND ended_at IS NULL",
                        params![machine, a.herdr_session, a.pane_id, a.kind],
                        |r| r.get(0),
                    )
                    .optional()?;
                let id = match id {
                    Some(id) => {
                        tx.execute(
                            "UPDATE seen_agents SET name = ?2, title = COALESCE(?3, title),
                             last_seen = ?4 WHERE id = ?1",
                            params![id, a.name, a.title, now],
                        )?;
                        id
                    }
                    None => {
                        tx.execute(
                            "INSERT INTO seen_agents (machine_id, herdr_session, pane_id, kind,
                             name, title, first_seen, last_seen) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?7)",
                            params![machine, a.herdr_session, a.pane_id, a.kind, a.name, a.title, now],
                        )?;
                        tx.last_insert_rowid()
                    }
                };
                kept.insert(id);
            }
            let open: Vec<i64> = tx
                .prepare("SELECT id FROM seen_agents WHERE machine_id = ?1 AND ended_at IS NULL")?
                .query_map(params![machine], |r| r.get(0))?
                .collect::<rusqlite::Result<_>>()?;
            for id in open.into_iter().filter(|id| !kept.contains(id)) {
                tx.execute(
                    "UPDATE seen_agents SET ended_at = ?2 WHERE id = ?1",
                    params![id, now],
                )?;
            }
            tx.execute(
                "DELETE FROM seen_agents WHERE ended_at < ?1",
                params![now - KEEP_ENDED_MS],
            )?;
            tx.commit()?;
            Ok(())
        })
        .await
    }

    /// Agents that ended at or after `since`, latest first.
    pub async fn ended_agents(&self, since: i64) -> Result<Vec<SeenAgentRecord>> {
        self.call(move |c| {
            let mut stmt = c.prepare(
                "SELECT machine_id, herdr_session, pane_id, kind, name, title, first_seen,
                 last_seen, ended_at FROM seen_agents WHERE ended_at >= ?1 ORDER BY ended_at DESC",
            )?;
            let rows = stmt
                .query_map(params![since], record)?
                .collect::<rusqlite::Result<_>>()?;
            Ok(rows)
        })
        .await
    }
}
