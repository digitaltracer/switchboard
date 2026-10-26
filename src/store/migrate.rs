use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

use tokio_rusqlite::rusqlite::Connection;

use super::StoreError;

/// Numbered migrations; the database's `PRAGMA user_version` is the last one applied.
pub const MIGRATIONS: &[(u32, &str)] = &[
    (1, include_str!("../../migrations/0001_init.sql")),
    (2, include_str!("../../migrations/0002_hub.sql")),
    (
        3,
        include_str!("../../migrations/0003_approval_numbers.sql"),
    ),
    (4, include_str!("../../migrations/0004_app_groundwork.sql")),
    (5, include_str!("../../migrations/0005_devices.sql")),
    (6, include_str!("../../migrations/0006_herdr_sessions.sql")),
    (7, include_str!("../../migrations/0007_seen_agents.sql")),
];

pub fn run(conn: &mut Connection, db_path: Option<&Path>) -> Result<u32, StoreError> {
    run_with(conn, db_path, MIGRATIONS)
}

pub(super) fn run_with(
    conn: &mut Connection,
    db_path: Option<&Path>,
    migrations: &[(u32, &str)],
) -> Result<u32, StoreError> {
    let current: u32 = conn.query_row("PRAGMA user_version", [], |r| r.get(0))?;
    let latest = migrations.last().map_or(0, |m| m.0);
    if current > latest {
        return Err(StoreError::Migration(format!(
            "database is at version {current}, newer than this binary's {latest}"
        )));
    }
    if current == latest {
        return Ok(current);
    }
    if current > 0
        && let Some(path) = db_path
    {
        backup(conn, path, current)?;
    }
    for (version, sql) in migrations.iter().filter(|(v, _)| *v > current) {
        let tx = conn.transaction()?;
        tx.execute_batch(sql)
            .map_err(|e| StoreError::Migration(format!("migration {version}: {e}")))?;
        tx.pragma_update(None, "user_version", version)?;
        tx.commit()?;
        tracing::info!(version, "applied migration");
    }
    Ok(latest)
}

/// Consistent copy via `VACUUM INTO`, safe under WAL.
fn backup(conn: &Connection, db_path: &Path, version: u32) -> Result<(), StoreError> {
    let ts = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_millis());
    let target = format!("{}.v{version}-{ts}.bak", db_path.display());
    conn.execute("VACUUM INTO ?1", [&target])?;
    tracing::info!(%target, "backed up database before migration");
    Ok(())
}
