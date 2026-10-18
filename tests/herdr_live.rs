//! Runs against a real Herdr server. Set SWITCHBOARD_HERDR_TEST_SOCKET to the socket of a
//! named test session (never your working session); skipped otherwise.

use std::process::Command;
use std::time::Duration;

use switchboard::herdr::types::*;
use switchboard::herdr::{Change, Client, Mirror};

fn client() -> Option<Client> {
    let socket = std::env::var_os("SWITCHBOARD_HERDR_TEST_SOCKET")?;
    Some(Client::new(socket).with_timeout(Duration::from_secs(10)))
}

fn git(dir: &std::path::Path, args: &[&str]) {
    let ok = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .status()
        .unwrap()
        .success();
    assert!(ok, "git {args:?}");
}

#[tokio::test]
async fn worktree_lifecycle_against_real_herdr() {
    let Some(client) = client() else {
        eprintln!("skipped: SWITCHBOARD_HERDR_TEST_SOCKET not set");
        return;
    };
    client.check_protocol().await.unwrap();

    let repo = tempfile::tempdir().unwrap();
    git(repo.path(), &["init", "-q"]);
    git(
        repo.path(),
        &[
            "-c",
            "user.name=sb",
            "-c",
            "user.email=sb@test",
            "commit",
            "-q",
            "--allow-empty",
            "-m",
            "init",
        ],
    );
    let repo_path = repo
        .path()
        .canonicalize()
        .unwrap()
        .to_string_lossy()
        .into_owned();
    let branch = format!("sb/live-{}", std::process::id());

    let mut mirror = Mirror::new();
    let (mut stream, _) = client
        .sync(&mut mirror, Subscription::lifecycle())
        .await
        .unwrap();

    let created = client
        .worktree_create(WorktreeCreateParams {
            cwd: Some(repo_path.clone()),
            branch: Some(branch.clone()),
            ..Default::default()
        })
        .await
        .unwrap();
    assert_eq!(created.worktree.branch.as_deref(), Some(branch.as_str()));
    let ws = created.workspace.workspace_id.clone();

    let (_, worktrees) = client
        .worktree_list(WorktreeListParams {
            cwd: Some(repo_path.clone()),
            workspace_id: None,
        })
        .await
        .unwrap();
    assert!(
        worktrees
            .iter()
            .any(|w| w.open_workspace_id.as_deref() == Some(ws.as_str()))
    );

    let snap = client.session_snapshot().await.unwrap();
    mirror.install(&snap);
    assert!(mirror.has_workspace(&ws));

    let removed = client.worktree_remove(&ws).await.unwrap();
    assert_eq!(removed, created.worktree.path);

    let gone = tokio::time::timeout(Duration::from_secs(5), async {
        while let Some(ev) = stream.next().await {
            if mirror.apply(&ev.unwrap()).contains(&Change::WorkspaceGone {
                workspace_id: ws.clone(),
            }) {
                return true;
            }
        }
        false
    })
    .await
    .unwrap();
    assert!(gone, "mirror saw the linked workspace close");

    // Herdr opened the source checkout as a workspace too; close only that one.
    if let Some(src) = client
        .session_snapshot()
        .await
        .unwrap()
        .workspaces
        .iter()
        .find(|w| {
            w.worktree.as_ref().and_then(|t| t.checkout_path.as_deref()) == Some(repo_path.as_str())
        })
    {
        client.workspace_close(&src.workspace_id).await.unwrap();
    }
}
