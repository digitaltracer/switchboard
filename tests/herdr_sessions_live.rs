//! The runner's Herdr sessions against the real `herdr`, in a throwaway Herdr config folder
//! (`XDG_CONFIG_HOME`), so the user's own sessions are never touched. Skipped unless
//! SWITCHBOARD_LIVE_HERDR_SESSIONS is set.

use std::process::Command;
use std::time::Duration;

use switchboard::herdr::types::WorktreeCreateParams;
use switchboard::runner::herdrs::{Herdrs, Starter};

fn herdr(xdg: &std::path::Path, args: &[&str]) -> String {
    let mut cmd = Command::new("herdr");
    cmd.args(args).env("XDG_CONFIG_HOME", xdg);
    for (k, _) in std::env::vars() {
        if k.starts_with("HERDR_") {
            cmd.env_remove(k);
        }
    }
    let out = cmd.output().unwrap();
    String::from_utf8_lossy(&out.stdout).into_owned() + &String::from_utf8_lossy(&out.stderr)
}

#[tokio::test]
async fn each_task_gets_a_real_herdr_session_of_its_own() {
    if std::env::var_os("SWITCHBOARD_LIVE_HERDR_SESSIONS").is_none() {
        return;
    }
    // A short path: Unix socket paths are limited to about 100 bytes.
    let xdg = tempfile::Builder::new()
        .prefix("sbh")
        .tempdir_in("/tmp")
        .unwrap();
    let config_dir = xdg.path().join("herdr");
    let herdrs = Herdrs::multi(
        config_dir.clone(),
        config_dir.join("herdr.sock"),
        Starter::Herdr {
            bin: "herdr".into(),
            xdg: Some(xdg.path().to_owned()),
            log_dir: xdg.path().join("logs"),
        },
    );
    assert!(
        herdrs.running().await.is_empty(),
        "a fresh folder has no sessions"
    );
    let _cleanup = StopOnDrop(xdg.path().to_owned());

    let a = herdrs
        .create(&herdrs.free_name("demo-aaaa", "aaaa1111"))
        .await
        .unwrap();
    let b = herdrs
        .create(&herdrs.free_name("demo-bbbb", "bbbb2222"))
        .await
        .unwrap();
    assert_eq!(
        (a.as_deref(), b.as_deref()),
        (Some("demo-aaaa"), Some("demo-bbbb"))
    );
    let running: Vec<Option<String>> = herdrs.running().await.into_iter().map(|(n, _)| n).collect();
    assert!(running.contains(&a) && running.contains(&b), "{running:?}");
    let listed = herdr(xdg.path(), &["session", "list"]);
    assert!(
        listed.contains("demo-aaaa") && listed.contains("demo-bbbb"),
        "{listed}"
    );

    // A worktree and workspace in the new session, as a task's start makes them.
    let repo = xdg.path().join("repo");
    std::fs::create_dir(&repo).unwrap();
    let git = |args: &[&str]| {
        assert!(
            Command::new("git")
                .arg("-C")
                .arg(&repo)
                .args(args)
                .status()
                .unwrap()
                .success()
        )
    };
    git(&["init", "-q", "-b", "main"]);
    git(&[
        "-c",
        "user.name=t",
        "-c",
        "user.email=t@t",
        "commit",
        "-q",
        "--allow-empty",
        "-m",
        "init",
    ]);
    let client = herdrs.client(a.as_deref());
    // Herdr keeps worktrees under ~/.herdr whatever the config folder, so each run's is its own.
    let run = xdg.path().file_name().unwrap().to_string_lossy();
    let created = client
        .worktree_create(WorktreeCreateParams {
            cwd: Some(repo.to_string_lossy().into_owned()),
            branch: Some(format!("sb/live-{run}")),
            ..Default::default()
        })
        .await
        .unwrap();
    let path = std::path::PathBuf::from(&created.worktree.path);
    assert!(path.exists());
    let ws = created.workspace.workspace_id.clone();
    let mine = client.session_snapshot().await.unwrap();
    assert!(
        mine.workspaces.iter().any(|w| w.workspace_id == ws),
        "the workspace is in the task's session"
    );
    client.worktree_remove(&ws).await.unwrap();
    assert!(
        !path.exists(),
        "a clean worktree is removed, not left in ~/.herdr"
    );
    let _ = std::fs::remove_dir(path.parent().unwrap());

    for name in [a.unwrap(), b.unwrap()] {
        herdrs.stop(&name).await.unwrap();
        tokio::time::sleep(Duration::from_millis(500)).await;
        herdr(xdg.path(), &["session", "delete", &name]);
    }
    assert!(herdrs.running().await.is_empty(), "both stopped");
    println!("live herdr sessions ok");
}

/// Stops this test's Herdr servers however it ends, so none outlive a failed assertion.
struct StopOnDrop(std::path::PathBuf);

impl Drop for StopOnDrop {
    fn drop(&mut self) {
        for name in ["demo-aaaa", "demo-bbbb"] {
            herdr(&self.0, &["--session", name, "server", "stop"]);
        }
    }
}
