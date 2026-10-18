//! SQLite state and audit log (tokio-rusqlite, bundled SQLite, WAL).

mod migrate;

use std::path::Path;
use std::time::Duration;

use serde_json::Value;
use tokio_rusqlite::Connection;
use tokio_rusqlite::rusqlite::{self, OptionalExtension, Row, params};

pub use migrate::MIGRATIONS;

pub type Result<T> = std::result::Result<T, StoreError>;

#[derive(Debug, thiserror::Error)]
pub enum StoreError {
    #[error("sqlite: {0}")]
    Sqlite(#[from] rusqlite::Error),
    #[error("database connection closed")]
    Closed,
    #[error("{0}")]
    Migration(String),
    #[error("request id {0:?} was already used for a different operation")]
    RequestIdReused(String),
    #[error("bad value in column {column}: {value:?}")]
    BadValue { column: &'static str, value: String },
}

impl From<tokio_rusqlite::Error<StoreError>> for StoreError {
    fn from(err: tokio_rusqlite::Error<StoreError>) -> Self {
        match err {
            tokio_rusqlite::Error::Error(e) => e,
            tokio_rusqlite::Error::Close((_, e)) => StoreError::Sqlite(e),
            _ => StoreError::Closed,
        }
    }
}

macro_rules! str_enum {
    ($name:ident, $column:literal { $($variant:ident => $s:literal),+ $(,)? }) => {
        #[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
        #[serde(rename_all = "snake_case")]
        pub enum $name { $($variant),+ }

        impl $name {
            pub fn as_str(self) -> &'static str {
                match self { $($name::$variant => $s),+ }
            }

            fn parse(s: String) -> Result<Self> {
                match s.as_str() {
                    $($s => Ok($name::$variant),)+
                    _ => Err(StoreError::BadValue { column: $column, value: s }),
                }
            }
        }
    };
}

str_enum!(Lifecycle, "lifecycle" {
    Starting => "starting",
    Running => "running",
    Stopping => "stopping",
    Stopped => "stopped",
    Failed => "failed",
});

str_enum!(OpState, "state" {
    Pending => "pending",
    Done => "done",
    Failed => "failed",
    OutcomeUnknown => "outcome_unknown",
});

str_enum!(Decision, "outcome" {
    Allow => "allow",
    Deny => "deny",
});

#[derive(Debug, Clone, PartialEq)]
pub struct SessionRecord {
    pub id: String,
    pub project_id: String,
    pub machine_id: String,
    pub agent: String,
    pub prompt_sha256: String,
    pub branch: Option<String>,
    pub worktree_path: Option<String>,
    pub workspace_id: Option<String>,
    pub pane_id: Option<String>,
    pub lifecycle: Lifecycle,
    pub agent_status: String,
    pub revision: i64,
    pub created_at: i64,
    pub updated_at: i64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Operation {
    pub request_id: String,
    pub kind: String,
    pub fingerprint: String,
    pub session_id: Option<String>,
    pub state: OpState,
    pub result: Option<Value>,
    pub created_at: i64,
    pub updated_at: i64,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Begin {
    /// Recorded as pending; the caller now performs it.
    Started,
    /// A retry: return or reconcile this record instead.
    Existing(Operation),
}

#[derive(Debug, Clone, PartialEq)]
pub struct ApprovalRecord {
    pub id: String,
    pub session_id: String,
    pub tool_name: String,
    pub tool_input: Value,
    pub created_at: i64,
    pub deadline: i64,
    pub outcome: Option<Decision>,
    pub reason: Option<String>,
    pub decided_at: Option<i64>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum DecideOutcome {
    Applied(Decision),
    /// Replay of a used approval: the recorded outcome, not re-applied.
    AlreadyDecided {
        outcome: Decision,
        reason: Option<String>,
    },
    /// Deadline passed; recorded as deny.
    Expired,
    NotFound,
}

#[derive(Debug, Clone, PartialEq)]
pub struct AuditEntry {
    pub id: i64,
    pub ts: i64,
    pub actor: String,
    pub session_id: Option<String>,
    pub kind: String,
    pub detail: Value,
}

#[derive(Clone)]
pub struct Store {
    conn: Connection,
}

impl Store {
    pub async fn open(path: &Path) -> Result<Self> {
        let conn = Connection::open(path).await?;
        let path = path.to_owned();
        conn.call(move |c| {
            configure(c)?;
            migrate::run(c, Some(&path))
        })
        .await?;
        Ok(Self { conn })
    }

    pub async fn open_in_memory() -> Result<Self> {
        let conn = Connection::open_in_memory().await?;
        conn.call(|c| {
            configure(c)?;
            migrate::run(c, None)
        })
        .await?;
        Ok(Self { conn })
    }

    async fn call<R, F>(&self, f: F) -> Result<R>
    where
        F: FnOnce(&mut rusqlite::Connection) -> Result<R> + Send + 'static,
        R: Send + 'static,
    {
        Ok(self.conn.call(f).await?)
    }

    pub async fn schema_version(&self) -> Result<u32> {
        self.call(|c| Ok(c.query_row("PRAGMA user_version", [], |r| r.get(0))?))
            .await
    }

    // Operations (idempotency by request id).

    pub async fn begin_operation(
        &self,
        request_id: &str,
        kind: &str,
        fingerprint: &str,
        now: i64,
    ) -> Result<Begin> {
        let (request_id, kind, fingerprint) = (
            request_id.to_owned(),
            kind.to_owned(),
            fingerprint.to_owned(),
        );
        self.call(move |c| {
            let tx = c.transaction()?;
            let existing = tx
                .query_row(
                    "SELECT * FROM operations WHERE request_id = ?1",
                    [&request_id],
                    raw_operation,
                )
                .optional()?;
            let begin = match existing {
                Some(raw) => {
                    let op = raw.finish()?;
                    if op.kind != kind || op.fingerprint != fingerprint {
                        return Err(StoreError::RequestIdReused(request_id));
                    }
                    Begin::Existing(op)
                }
                None => {
                    tx.execute(
                        "INSERT INTO operations (request_id, kind, fingerprint, state, created_at, updated_at)
                         VALUES (?1, ?2, ?3, 'pending', ?4, ?4)",
                        params![request_id, kind, fingerprint, now],
                    )?;
                    Begin::Started
                }
            };
            tx.commit()?;
            Ok(begin)
        })
        .await
    }

    pub async fn finish_operation(
        &self,
        request_id: &str,
        state: OpState,
        session_id: Option<&str>,
        result: Option<&Value>,
        now: i64,
    ) -> Result<()> {
        let request_id = request_id.to_owned();
        let session_id = session_id.map(str::to_owned);
        let result = result.map(Value::to_string);
        self.call(move |c| {
            c.execute(
                "UPDATE operations SET state = ?2, session_id = COALESCE(?3, session_id),
                 result = ?4, updated_at = ?5 WHERE request_id = ?1",
                params![request_id, state.as_str(), session_id, result, now],
            )?;
            Ok(())
        })
        .await
    }

    pub async fn get_operation(&self, request_id: &str) -> Result<Option<Operation>> {
        let request_id = request_id.to_owned();
        self.call(move |c| {
            c.query_row(
                "SELECT * FROM operations WHERE request_id = ?1",
                [request_id],
                raw_operation,
            )
            .optional()?
            .map(RawOperation::finish)
            .transpose()
        })
        .await
    }

    // Sessions.

    pub async fn insert_session(&self, s: SessionRecord) -> Result<()> {
        self.call(move |c| {
            c.execute(
                "INSERT INTO sessions (id, project_id, machine_id, agent, prompt_sha256, branch,
                 worktree_path, workspace_id, pane_id, lifecycle, agent_status, revision,
                 created_at, updated_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14)",
                params![
                    s.id,
                    s.project_id,
                    s.machine_id,
                    s.agent,
                    s.prompt_sha256,
                    s.branch,
                    s.worktree_path,
                    s.workspace_id,
                    s.pane_id,
                    s.lifecycle.as_str(),
                    s.agent_status,
                    s.revision,
                    s.created_at,
                    s.updated_at
                ],
            )?;
            Ok(())
        })
        .await
    }

    pub async fn get_session(&self, id: &str) -> Result<Option<SessionRecord>> {
        let id = id.to_owned();
        self.call(move |c| {
            c.query_row("SELECT * FROM sessions WHERE id = ?1", [id], raw_session)
                .optional()?
                .map(RawSession::finish)
                .transpose()
        })
        .await
    }

    pub async fn list_sessions(&self) -> Result<Vec<SessionRecord>> {
        self.call(|c| {
            let mut stmt = c.prepare("SELECT * FROM sessions ORDER BY created_at DESC")?;
            let rows = stmt.query_map([], raw_session)?;
            rows.map(|r| r?.finish()).collect()
        })
        .await
    }

    /// A session started with the same inputs at or after `since`, for the start dedupe window.
    pub async fn find_recent_start(
        &self,
        project_id: &str,
        machine_id: &str,
        agent: &str,
        prompt_sha256: &str,
        since: i64,
    ) -> Result<Option<SessionRecord>> {
        let args = [project_id, machine_id, agent, prompt_sha256].map(str::to_owned);
        self.call(move |c| {
            c.query_row(
                "SELECT * FROM sessions WHERE project_id = ?1 AND machine_id = ?2 AND agent = ?3
                 AND prompt_sha256 = ?4 AND created_at >= ?5 AND lifecycle != 'failed'
                 ORDER BY created_at DESC LIMIT 1",
                params![args[0], args[1], args[2], args[3], since],
                raw_session,
            )
            .optional()?
            .map(RawSession::finish)
            .transpose()
        })
        .await
    }

    pub async fn set_session_location(
        &self,
        id: &str,
        branch: &str,
        worktree_path: &str,
        workspace_id: &str,
        pane_id: &str,
        now: i64,
    ) -> Result<bool> {
        let args = [id, branch, worktree_path, workspace_id, pane_id].map(str::to_owned);
        self.call(move |c| {
            let n = c.execute(
                "UPDATE sessions SET branch = ?2, worktree_path = ?3, workspace_id = ?4,
                 pane_id = ?5, updated_at = ?6 WHERE id = ?1",
                params![args[0], args[1], args[2], args[3], args[4], now],
            )?;
            Ok(n == 1)
        })
        .await
    }

    pub async fn set_session_status(
        &self,
        id: &str,
        lifecycle: Lifecycle,
        agent_status: &str,
        revision: i64,
        now: i64,
    ) -> Result<bool> {
        let (id, agent_status) = (id.to_owned(), agent_status.to_owned());
        self.call(move |c| {
            let n = c.execute(
                "UPDATE sessions SET lifecycle = ?2, agent_status = ?3, revision = ?4,
                 updated_at = ?5 WHERE id = ?1",
                params![id, lifecycle.as_str(), agent_status, revision, now],
            )?;
            Ok(n == 1)
        })
        .await
    }

    // Approvals.

    pub async fn create_approval(&self, a: ApprovalRecord) -> Result<()> {
        self.call(move |c| {
            c.execute(
                "INSERT INTO approvals (id, session_id, tool_name, tool_input, created_at, deadline)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                params![
                    a.id,
                    a.session_id,
                    a.tool_name,
                    a.tool_input.to_string(),
                    a.created_at,
                    a.deadline
                ],
            )?;
            Ok(())
        })
        .await
    }

    pub async fn get_approval(&self, id: &str) -> Result<Option<ApprovalRecord>> {
        let id = id.to_owned();
        self.call(move |c| {
            c.query_row("SELECT * FROM approvals WHERE id = ?1", [id], raw_approval)
                .optional()?
                .map(RawApproval::finish)
                .transpose()
        })
        .await
    }

    pub async fn pending_approvals(&self, session_id: &str) -> Result<Vec<ApprovalRecord>> {
        let session_id = session_id.to_owned();
        self.call(move |c| {
            let mut stmt = c.prepare(
                "SELECT * FROM approvals WHERE session_id = ?1 AND outcome IS NULL
                 ORDER BY created_at",
            )?;
            let rows = stmt.query_map([session_id], raw_approval)?;
            rows.map(|r| r?.finish()).collect()
        })
        .await
    }

    /// Decides once. A late or repeated decision never changes the recorded outcome.
    pub async fn decide_approval(
        &self,
        id: &str,
        decision: Decision,
        reason: Option<&str>,
        now: i64,
    ) -> Result<DecideOutcome> {
        let id = id.to_owned();
        let reason = reason.map(str::to_owned);
        self.call(move |c| {
            let tx = c.transaction()?;
            let Some(current) = tx
                .query_row("SELECT * FROM approvals WHERE id = ?1", [&id], raw_approval)
                .optional()?
            else {
                return Ok(DecideOutcome::NotFound);
            };
            let current = current.finish()?;
            let outcome = if let Some(outcome) = current.outcome {
                DecideOutcome::AlreadyDecided {
                    outcome,
                    reason: current.reason,
                }
            } else if now >= current.deadline {
                record_decision(&tx, &id, Decision::Deny, Some("expired"), now)?;
                DecideOutcome::Expired
            } else {
                record_decision(&tx, &id, decision, reason.as_deref(), now)?;
                DecideOutcome::Applied(decision)
            };
            tx.commit()?;
            Ok(outcome)
        })
        .await
    }

    /// Denies every undecided approval whose deadline has passed.
    pub async fn expire_approvals(&self, now: i64) -> Result<Vec<String>> {
        self.call(move |c| {
            let mut stmt = c.prepare(
                "UPDATE approvals SET outcome = 'deny', reason = 'expired', decided_at = ?1
                 WHERE outcome IS NULL AND deadline <= ?1 RETURNING id",
            )?;
            let ids = stmt.query_map([now], |r| r.get(0))?;
            Ok(ids.collect::<rusqlite::Result<_>>()?)
        })
        .await
    }

    /// Denies every undecided approval of a session, e.g. on stop or a lost runner.
    pub async fn deny_pending(&self, session_id: &str, reason: &str, now: i64) -> Result<usize> {
        let (session_id, reason) = (session_id.to_owned(), reason.to_owned());
        self.call(move |c| {
            Ok(c.execute(
                "UPDATE approvals SET outcome = 'deny', reason = ?2, decided_at = ?3
                 WHERE session_id = ?1 AND outcome IS NULL",
                params![session_id, reason, now],
            )?)
        })
        .await
    }

    // Audit.

    pub async fn append_audit(
        &self,
        ts: i64,
        actor: &str,
        session_id: Option<&str>,
        kind: &str,
        detail: &Value,
    ) -> Result<i64> {
        let (actor, kind) = (actor.to_owned(), kind.to_owned());
        let session_id = session_id.map(str::to_owned);
        let detail = detail.to_string();
        self.call(move |c| {
            c.execute(
                "INSERT INTO audit (ts, actor, session_id, kind, detail) VALUES (?1, ?2, ?3, ?4, ?5)",
                params![ts, actor, session_id, kind, detail],
            )?;
            Ok(c.last_insert_rowid())
        })
        .await
    }

    pub async fn list_audit(
        &self,
        session_id: Option<&str>,
        limit: u32,
    ) -> Result<Vec<AuditEntry>> {
        let session_id = session_id.map(str::to_owned);
        self.call(move |c| {
            let mut stmt = c.prepare(
                "SELECT id, ts, actor, session_id, kind, detail FROM audit
                 WHERE ?1 IS NULL OR session_id = ?1 ORDER BY id DESC LIMIT ?2",
            )?;
            let rows = stmt.query_map(params![session_id, limit], |r| {
                Ok((
                    r.get(0)?,
                    r.get(1)?,
                    r.get(2)?,
                    r.get(3)?,
                    r.get(4)?,
                    r.get::<_, String>(5)?,
                ))
            })?;
            rows.map(|r| {
                let (id, ts, actor, session_id, kind, detail) = r?;
                Ok(AuditEntry {
                    id,
                    ts,
                    actor,
                    session_id,
                    kind,
                    detail: parse_json("detail", detail)?,
                })
            })
            .collect()
        })
        .await
    }
}

fn configure(c: &mut rusqlite::Connection) -> Result<()> {
    c.busy_timeout(Duration::from_secs(5))?;
    c.query_row("PRAGMA journal_mode = WAL", [], |_| Ok(()))?;
    c.execute_batch("PRAGMA foreign_keys = ON; PRAGMA synchronous = NORMAL;")?;
    Ok(())
}

fn record_decision(
    tx: &rusqlite::Transaction<'_>,
    id: &str,
    decision: Decision,
    reason: Option<&str>,
    now: i64,
) -> Result<()> {
    tx.execute(
        "UPDATE approvals SET outcome = ?2, reason = ?3, decided_at = ?4 WHERE id = ?1",
        params![id, decision.as_str(), reason, now],
    )?;
    Ok(())
}

fn parse_json(column: &'static str, s: String) -> Result<Value> {
    serde_json::from_str(&s).map_err(|_| StoreError::BadValue { column, value: s })
}

// Row mappers return raw strings; `finish` does the fallible enum and JSON parsing.

struct RawSession(SessionRecord, String);

fn raw_session(r: &Row<'_>) -> rusqlite::Result<RawSession> {
    Ok(RawSession(
        SessionRecord {
            id: r.get("id")?,
            project_id: r.get("project_id")?,
            machine_id: r.get("machine_id")?,
            agent: r.get("agent")?,
            prompt_sha256: r.get("prompt_sha256")?,
            branch: r.get("branch")?,
            worktree_path: r.get("worktree_path")?,
            workspace_id: r.get("workspace_id")?,
            pane_id: r.get("pane_id")?,
            lifecycle: Lifecycle::Starting,
            agent_status: r.get("agent_status")?,
            revision: r.get("revision")?,
            created_at: r.get("created_at")?,
            updated_at: r.get("updated_at")?,
        },
        r.get("lifecycle")?,
    ))
}

impl RawSession {
    fn finish(self) -> Result<SessionRecord> {
        Ok(SessionRecord {
            lifecycle: Lifecycle::parse(self.1)?,
            ..self.0
        })
    }
}

struct RawOperation(Operation, String, Option<String>);

fn raw_operation(r: &Row<'_>) -> rusqlite::Result<RawOperation> {
    Ok(RawOperation(
        Operation {
            request_id: r.get("request_id")?,
            kind: r.get("kind")?,
            fingerprint: r.get("fingerprint")?,
            session_id: r.get("session_id")?,
            state: OpState::Pending,
            result: None,
            created_at: r.get("created_at")?,
            updated_at: r.get("updated_at")?,
        },
        r.get("state")?,
        r.get("result")?,
    ))
}

impl RawOperation {
    fn finish(self) -> Result<Operation> {
        Ok(Operation {
            state: OpState::parse(self.1)?,
            result: self.2.map(|s| parse_json("result", s)).transpose()?,
            ..self.0
        })
    }
}

struct RawApproval(ApprovalRecord, String, Option<String>);

fn raw_approval(r: &Row<'_>) -> rusqlite::Result<RawApproval> {
    Ok(RawApproval(
        ApprovalRecord {
            id: r.get("id")?,
            session_id: r.get("session_id")?,
            tool_name: r.get("tool_name")?,
            tool_input: Value::Null,
            created_at: r.get("created_at")?,
            deadline: r.get("deadline")?,
            outcome: None,
            reason: r.get("reason")?,
            decided_at: r.get("decided_at")?,
        },
        r.get("tool_input")?,
        r.get("outcome")?,
    ))
}

impl RawApproval {
    fn finish(self) -> Result<ApprovalRecord> {
        Ok(ApprovalRecord {
            tool_input: parse_json("tool_input", self.1)?,
            outcome: self.2.map(Decision::parse).transpose()?,
            ..self.0
        })
    }
}

#[cfg(test)]
mod tests;
