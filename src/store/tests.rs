use serde_json::json;
use tokio_rusqlite::rusqlite;

use super::*;

fn session(id: &str, created_at: i64) -> SessionRecord {
    SessionRecord {
        id: id.into(),
        project_id: "switchboard".into(),
        machine_id: "vps".into(),
        agent: "claude".into(),
        prompt_sha256: "p1".into(),
        branch: None,
        worktree_path: None,
        workspace_id: None,
        pane_id: None,
        lifecycle: Lifecycle::Starting,
        agent_status: "unknown".into(),
        revision: 0,
        created_at,
        updated_at: created_at,
    }
}

fn approval(id: &str, deadline: i64) -> ApprovalRecord {
    ApprovalRecord {
        id: id.into(),
        session_id: "s1".into(),
        tool_name: "Bash".into(),
        tool_input: json!({"command": "ls"}),
        created_at: 0,
        deadline,
        outcome: None,
        reason: None,
        decided_at: None,
    }
}

async fn store_with_session() -> Store {
    let store = Store::open_in_memory().await.unwrap();
    store.insert_session(session("s1", 1_000)).await.unwrap();
    store
}

#[test]
fn migrations_are_numbered_contiguously() {
    for (i, (v, _)) in MIGRATIONS.iter().enumerate() {
        assert_eq!(*v as usize, i + 1);
    }
}

#[tokio::test]
async fn opens_file_db_in_wal_and_reopens() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("sb.db");
    let store = Store::open(&path).await.unwrap();
    assert_eq!(
        store.schema_version().await.unwrap(),
        MIGRATIONS.len() as u32
    );
    store.insert_session(session("s1", 1)).await.unwrap();
    let mode: String = store
        .call(|c| Ok(c.query_row("PRAGMA journal_mode", [], |r| r.get(0))?))
        .await
        .unwrap();
    assert_eq!(mode, "wal");
    drop(store);

    let store = Store::open(&path).await.unwrap();
    assert!(store.get_session("s1").await.unwrap().is_some());
    // Already current: no backup is taken.
    let backups = std::fs::read_dir(dir.path())
        .unwrap()
        .filter(|e| {
            e.as_ref()
                .unwrap()
                .file_name()
                .to_string_lossy()
                .ends_with(".bak")
        })
        .count();
    assert_eq!(backups, 0);
}

#[test]
fn failed_migration_keeps_version_and_data_and_backs_up() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("sb.db");
    let mut conn = rusqlite::Connection::open(&path).unwrap();
    let v1: &[(u32, &str)] = &[(1, "CREATE TABLE t (x INTEGER); INSERT INTO t VALUES (7);")];
    assert_eq!(migrate::run_with(&mut conn, Some(&path), v1).unwrap(), 1);

    let broken: &[(u32, &str)] = &[v1[0], (2, "CREATE TABLE u (y INTEGER); THIS IS NOT SQL;")];
    let err = migrate::run_with(&mut conn, Some(&path), broken).unwrap_err();
    assert!(err.to_string().contains("migration 2"), "{err}");

    let version: u32 = conn
        .query_row("PRAGMA user_version", [], |r| r.get(0))
        .unwrap();
    assert_eq!(version, 1);
    let x: i64 = conn.query_row("SELECT x FROM t", [], |r| r.get(0)).unwrap();
    assert_eq!(x, 7);
    let u_exists: i64 = conn
        .query_row(
            "SELECT count(*) FROM sqlite_master WHERE name = 'u'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(u_exists, 0, "partial migration must roll back");

    let backup = std::fs::read_dir(dir.path())
        .unwrap()
        .map(|e| e.unwrap().path())
        .find(|p| p.to_string_lossy().contains(".v1-"))
        .expect("backup written before migrating");
    let b = rusqlite::Connection::open(backup).unwrap();
    let x: i64 = b.query_row("SELECT x FROM t", [], |r| r.get(0)).unwrap();
    assert_eq!(x, 7);
}

#[test]
fn refuses_database_from_newer_binary() {
    let mut conn = rusqlite::Connection::open_in_memory().unwrap();
    conn.pragma_update(None, "user_version", 99).unwrap();
    let err = migrate::run(&mut conn, None).unwrap_err();
    assert!(err.to_string().contains("newer"), "{err}");
}

#[tokio::test]
async fn operation_retry_returns_existing_record() {
    let store = Store::open_in_memory().await.unwrap();
    assert_eq!(
        store
            .begin_operation("r1", "session_start", "fp", 1)
            .await
            .unwrap(),
        Begin::Started
    );
    let result = json!({"session_id": "s1", "branch": "sb/s1"});
    store
        .finish_operation("r1", OpState::Done, Some("s1"), Some(&result), 2)
        .await
        .unwrap();

    let Begin::Existing(op) = store
        .begin_operation("r1", "session_start", "fp", 3)
        .await
        .unwrap()
    else {
        panic!("expected existing operation");
    };
    assert_eq!(op.state, OpState::Done);
    assert_eq!(op.session_id.as_deref(), Some("s1"));
    assert_eq!(op.result, Some(result));
}

#[tokio::test]
async fn operation_request_id_reuse_with_other_args_is_rejected() {
    let store = Store::open_in_memory().await.unwrap();
    store
        .begin_operation("r1", "session_start", "fp", 1)
        .await
        .unwrap();
    let err = store
        .begin_operation("r1", "session_start", "other", 2)
        .await
        .unwrap_err();
    assert!(matches!(err, StoreError::RequestIdReused(_)));
    let err = store
        .begin_operation("r1", "session_stop", "fp", 2)
        .await
        .unwrap_err();
    assert!(matches!(err, StoreError::RequestIdReused(_)));
}

#[tokio::test]
async fn pending_operation_survives_reopen() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("sb.db");
    let store = Store::open(&path).await.unwrap();
    store
        .begin_operation("r1", "session_start", "fp", 1)
        .await
        .unwrap();
    drop(store);
    let store = Store::open(&path).await.unwrap();
    let op = store.get_operation("r1").await.unwrap().unwrap();
    assert_eq!(op.state, OpState::Pending);
}

#[tokio::test]
async fn session_round_trip_and_updates() {
    let store = store_with_session().await;
    assert!(
        store
            .set_session_location("s1", "sb/s1", "/wt/s1", "w2", "w2:p1", 5)
            .await
            .unwrap()
    );
    assert!(
        store
            .set_session_status("s1", Lifecycle::Running, "working", 9, 6)
            .await
            .unwrap()
    );
    let s = store.get_session("s1").await.unwrap().unwrap();
    assert_eq!(s.lifecycle, Lifecycle::Running);
    assert_eq!(s.pane_id.as_deref(), Some("w2:p1"));
    assert_eq!((s.revision, s.updated_at), (9, 6));
    assert!(
        !store
            .set_session_status("nope", Lifecycle::Running, "idle", 0, 7)
            .await
            .unwrap()
    );
    assert_eq!(store.list_sessions().await.unwrap().len(), 1);
}

#[tokio::test]
async fn recent_start_dedupe_window() {
    let store = store_with_session().await;
    let hit = store
        .find_recent_start("switchboard", "vps", "claude", "p1", 1_000)
        .await
        .unwrap();
    assert_eq!(hit.unwrap().id, "s1");
    let outside = store
        .find_recent_start("switchboard", "vps", "claude", "p1", 1_001)
        .await
        .unwrap();
    assert!(outside.is_none());
    let other_machine = store
        .find_recent_start("switchboard", "mac", "claude", "p1", 0)
        .await
        .unwrap();
    assert!(other_machine.is_none());
}

#[tokio::test]
async fn approval_decides_once_and_replay_returns_recorded_outcome() {
    let store = store_with_session().await;
    store.create_approval(approval("a1", 100)).await.unwrap();
    assert_eq!(store.pending_approvals("s1").await.unwrap().len(), 1);

    let first = store
        .decide_approval("a1", Decision::Allow, None, 10)
        .await
        .unwrap();
    assert_eq!(first, DecideOutcome::Applied(Decision::Allow));
    let replay = store
        .decide_approval("a1", Decision::Deny, None, 11)
        .await
        .unwrap();
    assert_eq!(
        replay,
        DecideOutcome::AlreadyDecided {
            outcome: Decision::Allow,
            reason: None
        }
    );
    assert!(store.pending_approvals("s1").await.unwrap().is_empty());
    let a = store.get_approval("a1").await.unwrap().unwrap();
    assert_eq!(a.tool_input, json!({"command": "ls"}));
    assert_eq!(a.decided_at, Some(10));
}

#[tokio::test]
async fn approval_after_deadline_is_denied() {
    let store = store_with_session().await;
    store.create_approval(approval("a1", 100)).await.unwrap();
    let late = store
        .decide_approval("a1", Decision::Allow, None, 100)
        .await
        .unwrap();
    assert_eq!(late, DecideOutcome::Expired);
    let a = store.get_approval("a1").await.unwrap().unwrap();
    assert_eq!(a.outcome, Some(Decision::Deny));
    assert_eq!(a.reason.as_deref(), Some("expired"));
    assert_eq!(
        store
            .decide_approval("missing", Decision::Allow, None, 1)
            .await
            .unwrap(),
        DecideOutcome::NotFound
    );
}

#[tokio::test]
async fn expire_and_deny_pending_sweep_only_undecided() {
    let store = store_with_session().await;
    store.create_approval(approval("a1", 50)).await.unwrap();
    store.create_approval(approval("a2", 500)).await.unwrap();
    store.create_approval(approval("a3", 500)).await.unwrap();
    store
        .decide_approval("a3", Decision::Allow, None, 10)
        .await
        .unwrap();

    assert_eq!(
        store.expire_approvals(60).await.unwrap(),
        vec!["a1".to_string()]
    );
    assert_eq!(
        store.deny_pending("s1", "runner_lost", 70).await.unwrap(),
        1
    );
    let a2 = store.get_approval("a2").await.unwrap().unwrap();
    assert_eq!(
        (a2.outcome, a2.reason.as_deref()),
        (Some(Decision::Deny), Some("runner_lost"))
    );
    let a3 = store.get_approval("a3").await.unwrap().unwrap();
    assert_eq!(a3.outcome, Some(Decision::Allow));
}

#[tokio::test]
async fn approvals_require_known_session() {
    let store = Store::open_in_memory().await.unwrap();
    assert!(store.create_approval(approval("a1", 100)).await.is_err());
}

#[tokio::test]
async fn audit_append_and_filter() {
    let store = store_with_session().await;
    store
        .append_audit(1, "bot:dot", Some("s1"), "prompt", &json!({"text": "hi"}))
        .await
        .unwrap();
    store
        .append_audit(2, "admin", None, "registration_opened", &json!({}))
        .await
        .unwrap();
    let all = store.list_audit(None, 10).await.unwrap();
    assert_eq!(all.len(), 2);
    assert_eq!(all[0].kind, "registration_opened");
    let s1 = store.list_audit(Some("s1"), 10).await.unwrap();
    assert_eq!(s1.len(), 1);
    assert_eq!(s1[0].detail, json!({"text": "hi"}));
}
