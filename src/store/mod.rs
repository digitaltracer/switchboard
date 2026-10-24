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

            pub(crate) fn parse(s: String) -> Result<Self> {
                match s.as_str() {
                    $($s => Ok($name::$variant),)+
                    _ => Err(StoreError::BadValue { column: $column, value: s }),
                }
            }
        }
    };
}

mod devices;
mod push;

pub use devices::{
    ApprovalProof, Challenge, ChallengePurpose, Claim, DeviceRecord, DeviceState, NewDevice,
    PushEnv,
};
pub use push::{DeliveryUpdate, DueDelivery};

// `exited`: the agent left its pane without a stop. `deleted`: stopped, and its Herdr session
// deleted; kept for the audit but no longer listed.
str_enum!(Lifecycle, "lifecycle" {
    Starting => "starting",
    Running => "running",
    Stopping => "stopping",
    Stopped => "stopped",
    Exited => "exited",
    Failed => "failed",
    Deleted => "deleted",
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

// `not_delivered`: the runner never took the decision, so the tool did not run.
str_enum!(Delivery, "delivery" {
    Delivered => "delivered",
    NotDelivered => "not_delivered",
    Unknown => "unknown",
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
    pub agent_name: Option<String>,
    pub muted: bool,
    pub hook_failed: bool,
    pub error: Option<String>,
    /// The commit the worktree was created from; absent for sessions from before it was kept.
    pub start_commit: Option<String>,
    /// The Herdr session the task runs in.
    pub herdr_session: Option<String>,
}

/// Fields to change on a session; `None` leaves a field as it is. Every update bumps the revision.
#[derive(Debug, Clone, Default)]
pub struct SessionPatch {
    pub lifecycle: Option<Lifecycle>,
    pub agent_status: Option<String>,
    pub hook_failed: Option<bool>,
    pub muted: Option<bool>,
    pub error: Option<String>,
    pub branch: Option<String>,
    pub worktree_path: Option<String>,
    pub workspace_id: Option<String>,
    pub pane_id: Option<String>,
    pub start_commit: Option<String>,
    pub herdr_session: Option<String>,
    /// The agent finished a turn: queue a "finished" notification unless the session is muted.
    pub finished: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub struct NewOperation {
    pub request_id: String,
    pub kind: String,
    pub fingerprint: String,
    pub session_id: Option<String>,
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

/// A recorded approval's short number, and whether it notifies now.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NewApproval {
    pub number: i64,
    pub notify: bool,
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
    pub delivery: Option<Delivery>,
    /// Short number for chat; set by `create_approval`.
    pub number: Option<i64>,
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
    /// The signed approval's challenge didn't match, was used or expired, or its device isn't active.
    /// Nothing was recorded.
    BadProof,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct NotifySettings {
    pub mute_all: bool,
    pub imported_at: Option<i64>,
    pub agents: Vec<NotifyAgent>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct NotifyAgent {
    pub name: String,
    pub watched: bool,
    pub muted: bool,
}

str_enum!(OutboxKind, "kind" {
    Approval => "approval",
    Finished => "finished",
    DevicePaired => "device_paired",
});

/// A notification to send, recorded with the change that caused it.
#[derive(Debug, Clone, PartialEq)]
pub struct OutboxEvent {
    pub id: i64,
    pub dedupe_key: String,
    pub kind: OutboxKind,
    pub session_id: Option<String>,
    pub approval_id: Option<String>,
    pub created_at: i64,
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
        self.begin_session_operation(
            NewOperation {
                request_id: request_id.to_owned(),
                kind: kind.to_owned(),
                fingerprint: fingerprint.to_owned(),
                session_id: None,
            },
            now,
        )
        .await
    }

    pub async fn begin_session_operation(&self, op: NewOperation, now: i64) -> Result<Begin> {
        self.call(move |c| {
            let tx = c.transaction()?;
            let begin = begin_op(&tx, &op, now)?;
            tx.commit()?;
            Ok(begin)
        })
        .await
    }

    /// Records a start's operation and its session in one transaction, before any runner call.
    pub async fn begin_start(
        &self,
        op: NewOperation,
        session: SessionRecord,
        now: i64,
    ) -> Result<Begin> {
        self.call(move |c| {
            let tx = c.transaction()?;
            let begin = begin_op(&tx, &op, now)?;
            if begin == Begin::Started {
                insert_session(&tx, &session)?;
            }
            tx.commit()?;
            Ok(begin)
        })
        .await
    }

    /// Settles an open operation; one already done or failed keeps its recorded outcome.
    pub async fn finish_operation(
        &self,
        request_id: &str,
        state: OpState,
        session_id: Option<&str>,
        result: Option<&Value>,
        now: i64,
    ) -> Result<bool> {
        let request_id = request_id.to_owned();
        let session_id = session_id.map(str::to_owned);
        let result = result.map(Value::to_string);
        self.call(move |c| {
            let n = c.execute(
                "UPDATE operations SET state = ?2, session_id = COALESCE(?3, session_id),
                 result = ?4, updated_at = ?5
                 WHERE request_id = ?1 AND state IN ('pending', 'outcome_unknown')",
                params![request_id, state.as_str(), session_id, result, now],
            )?;
            Ok(n == 1)
        })
        .await
    }

    /// Forgets an operation that never reached a runner, so a retry can run it.
    pub async fn delete_operation(&self, request_id: &str) -> Result<()> {
        let request_id = request_id.to_owned();
        self.call(move |c| {
            c.execute(
                "DELETE FROM operations WHERE request_id = ?1 AND state = 'pending'",
                [request_id],
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

    /// Settles a session's open operations of `kind`, e.g. a start resolved by reconciliation.
    pub async fn resolve_session_operations(
        &self,
        session_id: &str,
        kind: &str,
        state: OpState,
        result: &Value,
        now: i64,
    ) -> Result<usize> {
        let (session_id, kind, result) =
            (session_id.to_owned(), kind.to_owned(), result.to_string());
        self.call(move |c| {
            Ok(c.execute(
                "UPDATE operations SET state = ?3, result = ?4, updated_at = ?5
                 WHERE session_id = ?1 AND kind = ?2 AND state IN ('pending', 'outcome_unknown')",
                params![session_id, kind, state.as_str(), result, now],
            )?)
        })
        .await
    }

    /// A session's operations of `kind` that are still pending or `outcome_unknown`.
    pub async fn open_operations(&self, session_id: &str, kind: &str) -> Result<Vec<Operation>> {
        let (session_id, kind) = (session_id.to_owned(), kind.to_owned());
        self.call(move |c| {
            let mut stmt = c.prepare(
                "SELECT * FROM operations WHERE session_id = ?1 AND kind = ?2
                 AND state IN ('pending', 'outcome_unknown') ORDER BY created_at",
            )?;
            let rows = stmt.query_map(params![session_id, kind], raw_operation)?;
            let raws = rows.collect::<rusqlite::Result<Vec<_>>>()?;
            raws.into_iter().map(RawOperation::finish).collect()
        })
        .await
    }

    /// After a hub restart nothing is still in flight: pending operations become `outcome_unknown`.
    pub async fn mark_pending_unknown(&self, now: i64) -> Result<usize> {
        self.call(move |c| {
            Ok(c.execute(
                "UPDATE operations SET state = 'outcome_unknown', updated_at = ?1 WHERE state = 'pending'",
                [now],
            )?)
        })
        .await
    }

    // Sessions.

    pub async fn insert_session(&self, s: SessionRecord) -> Result<()> {
        self.call(move |c| {
            insert_session(c, &s)?;
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

    pub async fn get_session_by_agent_name(&self, name: &str) -> Result<Option<SessionRecord>> {
        let name = name.to_owned();
        self.call(move |c| {
            c.query_row(
                "SELECT * FROM sessions WHERE agent_name = ?1",
                [name],
                raw_session,
            )
            .optional()?
            .map(RawSession::finish)
            .transpose()
        })
        .await
    }

    /// Applies `patch` and moves the session to a new revision; `None` if there is no such session.
    pub async fn update_session(
        &self,
        id: &str,
        patch: SessionPatch,
        now: i64,
    ) -> Result<Option<SessionRecord>> {
        let id = id.to_owned();
        self.call(move |c| {
            let tx = c.transaction()?;
            let revision = bump(&tx)?;
            let s = tx
                .query_row(
                    "UPDATE sessions SET lifecycle = COALESCE(?2, lifecycle),
                     agent_status = COALESCE(?3, agent_status), hook_failed = COALESCE(?4, hook_failed),
                     muted = COALESCE(?5, muted), error = COALESCE(?6, error),
                     branch = COALESCE(?7, branch), worktree_path = COALESCE(?8, worktree_path),
                     workspace_id = COALESCE(?9, workspace_id), pane_id = COALESCE(?10, pane_id),
                     start_commit = COALESCE(?11, start_commit), revision = ?12, updated_at = ?13,
                     herdr_session = COALESCE(?14, herdr_session)
                     WHERE id = ?1 RETURNING *",
                    params![
                        id,
                        patch.lifecycle.map(Lifecycle::as_str),
                        patch.agent_status,
                        patch.hook_failed,
                        patch.muted,
                        patch.error,
                        patch.branch,
                        patch.worktree_path,
                        patch.workspace_id,
                        patch.pane_id,
                        patch.start_commit,
                        revision,
                        now,
                        patch.herdr_session
                    ],
                    raw_session,
                )
                .optional()?
                .map(RawSession::finish)
                .transpose()?;
            if patch.finished && s.is_some() {
                queue(
                    &tx,
                    &format!("finished:{id}:{revision}"),
                    OutboxKind::Finished,
                    Some(&id),
                    None,
                    now,
                    None,
                )?;
            }
            tx.commit()?;
            Ok(s)
        })
        .await
    }

    /// The latest change counter: it moves with every session change and every list-wide one.
    pub async fn revision(&self) -> Result<i64> {
        self.call(
            |c| Ok(c.query_row("SELECT value FROM revisions WHERE id = 1", [], |r| r.get(0))?),
        )
        .await
    }

    /// Records a change the session list shows that touches no session row, e.g. mute-all.
    pub async fn bump_revision(&self) -> Result<i64> {
        self.call(|c| {
            let tx = c.transaction()?;
            let v = bump(&tx)?;
            tx.commit()?;
            Ok(v)
        })
        .await
    }

    // Outbox.

    /// Notifications recorded after `after_id`, oldest first.
    pub async fn outbox_after(&self, after_id: i64, limit: u32) -> Result<Vec<OutboxEvent>> {
        self.call(move |c| {
            let mut stmt = c.prepare(
                "SELECT id, dedupe_key, kind, session_id, approval_id, created_at FROM outbox
                 WHERE id > ?1 ORDER BY id LIMIT ?2",
            )?;
            let rows = stmt.query_map(params![after_id, limit], |r| {
                Ok((
                    OutboxEvent {
                        id: r.get(0)?,
                        dedupe_key: r.get(1)?,
                        kind: OutboxKind::Approval,
                        session_id: r.get(3)?,
                        approval_id: r.get(4)?,
                        created_at: r.get(5)?,
                    },
                    r.get::<_, String>(2)?,
                ))
            })?;
            rows.map(|r| {
                let (e, kind) = r?;
                Ok(OutboxEvent {
                    kind: OutboxKind::parse(kind)?,
                    ..e
                })
            })
            .collect()
        })
        .await
    }

    // Approvals.

    /// Records a new approval and returns its short number. Only a session's oldest open
    /// approval notifies, so a burst of requests sends one; the next notifies when it closes.
    pub async fn create_approval(&self, a: ApprovalRecord) -> Result<NewApproval> {
        self.call(move |c| {
            let tx = c.transaction()?;
            let number = tx.query_row(
                "INSERT INTO approvals (id, session_id, tool_name, tool_input, created_at, deadline, number)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, (SELECT COALESCE(MAX(number), 0) + 1 FROM approvals))
                 RETURNING number",
                params![
                    a.id,
                    a.session_id,
                    a.tool_name,
                    a.tool_input.to_string(),
                    a.created_at,
                    a.deadline
                ],
                |r| r.get(0),
            )?;
            let notify: bool = tx.query_row(
                "SELECT NOT EXISTS (SELECT 1 FROM approvals
                 WHERE session_id = ?1 AND outcome IS NULL AND id != ?2)",
                params![a.session_id, a.id],
                |r| r.get(0),
            )?;
            if notify {
                queue_approval(&tx, &a.session_id, &a.id, a.created_at)?;
            }
            tx.commit()?;
            Ok(NewApproval { number, notify })
        })
        .await
    }

    /// After `closed_number` closes: the session's oldest open approval, notified, if it came
    /// after the closed one. An older one is still open, so it already notified.
    pub async fn notify_next_approval(
        &self,
        session_id: &str,
        closed_number: i64,
        now: i64,
    ) -> Result<Option<ApprovalRecord>> {
        let session_id = session_id.to_owned();
        self.call(move |c| {
            let tx = c.transaction()?;
            let next = tx
                .query_row(
                    "SELECT * FROM approvals WHERE session_id = ?1 AND outcome IS NULL
                     ORDER BY number LIMIT 1",
                    [&session_id],
                    raw_approval,
                )
                .optional()?
                .map(RawApproval::finish)
                .transpose()?
                .filter(|a| a.number.is_some_and(|n| n > closed_number));
            if let Some(a) = &next {
                queue_approval(&tx, &a.session_id, &a.id, now)?;
            }
            tx.commit()?;
            Ok(next)
        })
        .await
    }

    pub async fn get_approval_by_number(&self, number: i64) -> Result<Option<ApprovalRecord>> {
        self.call(move |c| {
            c.query_row(
                "SELECT * FROM approvals WHERE number = ?1",
                [number],
                raw_approval,
            )
            .optional()?
            .map(RawApproval::finish)
            .transpose()
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
        self.decide_approval_proved(id, decision, reason, now, None)
            .await
    }

    /// As `decide_approval`; a proof's nonce is used in the same transaction as the decision.
    pub async fn decide_approval_proved(
        &self,
        id: &str,
        decision: Decision,
        reason: Option<&str>,
        now: i64,
        proof: Option<ApprovalProof>,
    ) -> Result<DecideOutcome> {
        let id = id.to_owned();
        let reason = reason.map(str::to_owned);
        self.call(move |c| {
            let tx = c.transaction()?;
            if let Some(p) = &proof
                && !devices::use_challenge(
                    &tx,
                    &p.nonce,
                    &p.device_id,
                    ChallengePurpose::Approval,
                    Some(&id),
                    now,
                )?
            {
                return Ok(DecideOutcome::BadProof);
            }
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
    pub async fn expire_approvals(&self, now: i64) -> Result<Vec<ApprovalRecord>> {
        self.approvals_returning(
            "UPDATE approvals SET outcome = 'deny', reason = 'expired', decided_at = ?1
             WHERE outcome IS NULL AND deadline <= ?1 RETURNING *",
            vec![now.into()],
        )
        .await
    }

    /// Denies every undecided approval of a session, e.g. on stop or exit.
    pub async fn deny_pending(
        &self,
        session_id: &str,
        reason: &str,
        now: i64,
    ) -> Result<Vec<ApprovalRecord>> {
        self.approvals_returning(
            "UPDATE approvals SET outcome = 'deny', reason = ?2, decided_at = ?3
             WHERE session_id = ?1 AND outcome IS NULL RETURNING *",
            vec![
                session_id.to_owned().into(),
                reason.to_owned().into(),
                now.into(),
            ],
        )
        .await
    }

    /// Denies every undecided approval of a machine's sessions, when its runner is lost.
    pub async fn deny_pending_on_machine(
        &self,
        machine_id: &str,
        reason: &str,
        now: i64,
    ) -> Result<Vec<ApprovalRecord>> {
        self.approvals_returning(
            "UPDATE approvals SET outcome = 'deny', reason = ?2, decided_at = ?3
             WHERE outcome IS NULL
             AND session_id IN (SELECT id FROM sessions WHERE machine_id = ?1) RETURNING *",
            vec![
                machine_id.to_owned().into(),
                reason.to_owned().into(),
                now.into(),
            ],
        )
        .await
    }

    /// Denies every undecided approval, when the hub restarts.
    pub async fn deny_all_pending(&self, reason: &str, now: i64) -> Result<Vec<ApprovalRecord>> {
        self.approvals_returning(
            "UPDATE approvals SET outcome = 'deny', reason = ?1, decided_at = ?2
             WHERE outcome IS NULL RETURNING *",
            vec![reason.to_owned().into(), now.into()],
        )
        .await
    }

    /// Allows recorded but never confirmed by a runner, after a hub restart: they may have run.
    pub async fn mark_unconfirmed_allows_unknown(&self) -> Result<Vec<ApprovalRecord>> {
        self.approvals_returning(
            "UPDATE approvals SET delivery = 'unknown'
             WHERE outcome = 'allow' AND delivery IS NULL RETURNING *",
            vec![],
        )
        .await
    }

    /// Denials on a machine not confirmed by its runner whose deadline is after `since`.
    pub async fn undelivered_denials(
        &self,
        machine_id: &str,
        since: i64,
    ) -> Result<Vec<ApprovalRecord>> {
        self.approvals_returning(
            "SELECT a.* FROM approvals a JOIN sessions s ON s.id = a.session_id
             WHERE s.machine_id = ?1 AND a.outcome = 'deny'
             AND (a.delivery IS NULL OR a.delivery != 'delivered')
             AND a.deadline > ?2 ORDER BY a.created_at",
            vec![machine_id.to_owned().into(), since.into()],
        )
        .await
    }

    /// Records what the runner did with a decision; an undelivered allow is recorded as a deny.
    pub async fn settle_delivery(
        &self,
        id: &str,
        delivery: Delivery,
    ) -> Result<Option<ApprovalRecord>> {
        let id = id.to_owned();
        self.call(move |c| {
            let tx = c.transaction()?;
            if delivery == Delivery::NotDelivered {
                tx.execute(
                    "UPDATE approvals SET outcome = 'deny', reason = 'not_delivered'
                     WHERE id = ?1 AND outcome = 'allow'",
                    [&id],
                )?;
            }
            let a = tx
                .query_row(
                    "UPDATE approvals SET delivery = ?2 WHERE id = ?1 RETURNING *",
                    params![id, delivery.as_str()],
                    raw_approval,
                )
                .optional()?
                .map(RawApproval::finish)
                .transpose()?;
            tx.commit()?;
            Ok(a)
        })
        .await
    }

    async fn approvals_returning(
        &self,
        sql: &'static str,
        args: Vec<rusqlite::types::Value>,
    ) -> Result<Vec<ApprovalRecord>> {
        self.call(move |c| {
            let mut stmt = c.prepare(sql)?;
            let rows = stmt.query_map(rusqlite::params_from_iter(args), raw_approval)?;
            let raws = rows.collect::<rusqlite::Result<Vec<_>>>()?;
            raws.into_iter().map(RawApproval::finish).collect()
        })
        .await
    }

    // Notification state shared with the WhatsApp bridge.

    pub async fn notify_settings(&self) -> Result<NotifySettings> {
        self.call(|c| {
            let (mute_all, imported_at) = c.query_row(
                "SELECT mute_all, imported_at FROM notify_settings WHERE id = 1",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )?;
            let mut stmt =
                c.prepare("SELECT name, watched, muted FROM notify_agents ORDER BY name")?;
            let agents = stmt
                .query_map([], |r| {
                    Ok(NotifyAgent {
                        name: r.get(0)?,
                        watched: r.get(1)?,
                        muted: r.get(2)?,
                    })
                })?
                .collect::<rusqlite::Result<_>>()?;
            Ok(NotifySettings {
                mute_all,
                imported_at,
                agents,
            })
        })
        .await
    }

    pub async fn set_mute_all(&self, mute_all: bool) -> Result<()> {
        self.call(move |c| {
            c.execute(
                "UPDATE notify_settings SET mute_all = ?1 WHERE id = 1",
                [mute_all],
            )?;
            Ok(())
        })
        .await
    }

    /// Changes an agent that is not a session; `None` leaves a flag as it is.
    pub async fn set_notify_agent(
        &self,
        name: &str,
        watched: Option<bool>,
        muted: Option<bool>,
        now: i64,
    ) -> Result<NotifyAgent> {
        let name = name.to_owned();
        self.call(move |c| {
            Ok(c.query_row(
                "INSERT INTO notify_agents (name, watched, muted, updated_at)
                 VALUES (?1, COALESCE(?2, 0), COALESCE(?3, 0), ?4)
                 ON CONFLICT (name) DO UPDATE SET watched = COALESCE(?2, watched),
                 muted = COALESCE(?3, muted), updated_at = ?4
                 RETURNING name, watched, muted",
                params![name, watched, muted, now],
                |r| {
                    Ok(NotifyAgent {
                        name: r.get(0)?,
                        watched: r.get(1)?,
                        muted: r.get(2)?,
                    })
                },
            )?)
        })
        .await
    }

    /// Merges the bridge's own watch file, once. Returns false if it was already imported.
    pub async fn import_notify(
        &self,
        watched: Vec<String>,
        muted: Vec<String>,
        mute_all: bool,
        now: i64,
    ) -> Result<bool> {
        self.call(move |c| {
            let tx = c.transaction()?;
            let imported: Option<i64> = tx.query_row(
                "SELECT imported_at FROM notify_settings WHERE id = 1",
                [],
                |r| r.get(0),
            )?;
            if imported.is_some() {
                return Ok(false);
            }
            tx.execute(
                "UPDATE notify_settings SET mute_all = mute_all OR ?1, imported_at = ?2 WHERE id = 1",
                params![mute_all, now],
            )?;
            let revision = bump(&tx)?;
            for name in &muted {
                let n = tx.execute(
                    "UPDATE sessions SET muted = 1, revision = ?3, updated_at = ?2
                     WHERE agent_name = ?1",
                    params![name, now, revision],
                )?;
                if n == 0 {
                    tx.execute(
                        "INSERT INTO notify_agents (name, muted, updated_at) VALUES (?1, 1, ?2)
                         ON CONFLICT (name) DO UPDATE SET muted = 1, updated_at = ?2",
                        params![name, now],
                    )?;
                }
            }
            for name in &watched {
                tx.execute(
                    "INSERT INTO notify_agents (name, watched, updated_at) VALUES (?1, 1, ?2)
                     ON CONFLICT (name) DO UPDATE SET watched = 1, updated_at = ?2",
                    params![name, now],
                )?;
            }
            tx.commit()?;
            Ok(true)
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

/// Moves the change counter on and returns its new value.
fn bump(c: &rusqlite::Connection) -> Result<i64> {
    Ok(c.query_row(
        "UPDATE revisions SET value = value + 1 WHERE id = 1 RETURNING value",
        [],
        |r| r.get(0),
    )?)
}

/// Queues a notification for every active device but `except`. A session's notification
/// follows its mutes and mute-all; a repeated key is a no-op.
fn queue_approval(c: &rusqlite::Connection, session_id: &str, id: &str, now: i64) -> Result<()> {
    queue(
        c,
        &format!("approval:{id}"),
        OutboxKind::Approval,
        Some(session_id),
        Some(id),
        now,
        None,
    )
}

fn queue(
    c: &rusqlite::Connection,
    dedupe_key: &str,
    kind: OutboxKind,
    session_id: Option<&str>,
    approval_id: Option<&str>,
    now: i64,
    except: Option<&str>,
) -> Result<()> {
    let n = c.execute(
        "INSERT INTO outbox (dedupe_key, kind, session_id, approval_id, created_at)
         SELECT ?1, ?2, ?3, ?4, ?5
         WHERE ?3 IS NULL OR (NOT EXISTS (SELECT 1 FROM sessions WHERE id = ?3 AND muted)
         AND NOT (SELECT mute_all FROM notify_settings WHERE id = 1))
         ON CONFLICT (dedupe_key) DO NOTHING",
        params![dedupe_key, kind.as_str(), session_id, approval_id, now],
    )?;
    if n == 1 {
        c.execute(
            "INSERT INTO outbox_deliveries (outbox_id, device_id, state, next_attempt_at)
             SELECT ?1, id, 'pending', ?2 FROM devices WHERE state = 'active' AND id IS NOT ?3",
            params![c.last_insert_rowid(), now, except],
        )?;
    }
    Ok(())
}

fn insert_session(c: &rusqlite::Connection, s: &SessionRecord) -> Result<()> {
    let revision = bump(c)?.max(s.revision);
    c.execute(
        "INSERT INTO sessions (id, project_id, machine_id, agent, prompt_sha256, branch,
         worktree_path, workspace_id, pane_id, lifecycle, agent_status, revision,
         created_at, updated_at, agent_name, muted, hook_failed, error, start_commit, herdr_session)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18, ?19, ?20)",
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
            revision,
            s.created_at,
            s.updated_at,
            s.agent_name,
            s.muted,
            s.hook_failed,
            s.error,
            s.start_commit,
            s.herdr_session
        ],
    )?;
    Ok(())
}

/// Records `op` as pending unless its request id exists; a reused id with other arguments fails.
fn begin_op(tx: &rusqlite::Transaction<'_>, op: &NewOperation, now: i64) -> Result<Begin> {
    let existing = tx
        .query_row(
            "SELECT * FROM operations WHERE request_id = ?1",
            [&op.request_id],
            raw_operation,
        )
        .optional()?;
    if let Some(raw) = existing {
        let existing = raw.finish()?;
        if existing.kind != op.kind || existing.fingerprint != op.fingerprint {
            return Err(StoreError::RequestIdReused(op.request_id.clone()));
        }
        return Ok(Begin::Existing(existing));
    }
    tx.execute(
        "INSERT INTO operations (request_id, kind, fingerprint, session_id, state, created_at, updated_at)
         VALUES (?1, ?2, ?3, ?4, 'pending', ?5, ?5)",
        params![op.request_id, op.kind, op.fingerprint, op.session_id, now],
    )?;
    Ok(Begin::Started)
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
            agent_name: r.get("agent_name")?,
            muted: r.get("muted")?,
            hook_failed: r.get("hook_failed")?,
            error: r.get("error")?,
            start_commit: r.get("start_commit")?,
            herdr_session: r.get("herdr_session")?,
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

struct RawApproval(ApprovalRecord, String, Option<String>, Option<String>);

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
            delivery: None,
            number: r.get("number")?,
        },
        r.get("tool_input")?,
        r.get("outcome")?,
        r.get("delivery")?,
    ))
}

impl RawApproval {
    fn finish(self) -> Result<ApprovalRecord> {
        Ok(ApprovalRecord {
            tool_input: parse_json("tool_input", self.1)?,
            outcome: self.2.map(Decision::parse).transpose()?,
            delivery: self.3.map(Delivery::parse).transpose()?,
            ..self.0
        })
    }
}

#[cfg(test)]
mod tests;
