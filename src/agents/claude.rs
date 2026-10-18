//! Claude Code profile. Verified against 2.1.302 (spike-findings.md).

use std::io;
use std::path::Path;

use serde_json::{Value, json};

use super::HOOK_TIMEOUT_SECS;

/// Settings passed with `--settings`, kept outside the worktree.
/// `onFailure: block` turns a killed, failed or timed-out helper into a deny.
pub fn settings(hook_command: &str) -> Value {
    json!({
        // acceptEdits would otherwise run touch, rm, mv and redirects without asking the hook.
        "permissions": { "ask": ["Bash"] },
        "hooks": {
            "PermissionRequest": [{
                "matcher": "*",
                "hooks": [{
                    "type": "command",
                    "command": hook_command,
                    "timeout": HOOK_TIMEOUT_SECS,
                    "onFailure": "block",
                }],
            }],
            // Proves the hook settings are loaded and the helper reaches the runner.
            "SessionStart": [{
                "hooks": [{ "type": "command", "command": hook_command, "timeout": 30 }],
            }],
        }
    })
}

/// Edits are accepted, other tools ask the hook; explicit as a user's default may be `auto`.
pub const PERMISSION_MODE: &str = "acceptEdits";

pub fn launch_args(settings_path: &Path) -> Vec<String> {
    vec![
        "--settings".into(),
        settings_path.to_string_lossy().into_owned(),
        "--permission-mode".into(),
        PERMISSION_MODE.into(),
    ]
}

/// Marks the main repo root trusted in `~/.claude.json`, which covers all its worktrees.
/// A persistent write to the user's Claude config; returns whether anything changed.
pub fn pretrust(claude_json: &Path, repo_root: &Path) -> io::Result<bool> {
    let key = repo_root.to_string_lossy().into_owned();
    let mut doc: Value = match std::fs::read_to_string(claude_json) {
        Ok(text) => serde_json::from_str(&text).map_err(io::Error::other)?,
        Err(e) if e.kind() == io::ErrorKind::NotFound => json!({}),
        Err(e) => return Err(e),
    };
    let root = doc
        .as_object_mut()
        .ok_or_else(|| io::Error::other("claude config is not a JSON object"))?;
    let projects = root.entry("projects").or_insert_with(|| json!({}));
    let project = projects
        .as_object_mut()
        .ok_or_else(|| io::Error::other("claude config `projects` is not an object"))?
        .entry(key)
        .or_insert_with(|| json!({}));
    if project.get("hasTrustDialogAccepted") == Some(&Value::Bool(true)) {
        return Ok(false);
    }
    project
        .as_object_mut()
        .ok_or_else(|| io::Error::other("claude project entry is not an object"))?
        .insert("hasTrustDialogAccepted".into(), Value::Bool(true));
    let tmp = claude_json.with_extension("json.switchboard-tmp");
    std::fs::write(
        &tmp,
        serde_json::to_vec_pretty(&doc).map_err(io::Error::other)?,
    )?;
    std::fs::rename(&tmp, claude_json)?;
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn permission_hook_fails_closed() {
        let s = settings("/bin/switchboard hook --agent claude --secret-file /s");
        let h = &s["hooks"]["PermissionRequest"][0]["hooks"][0];
        assert_eq!(h["onFailure"], "block");
        assert_eq!(h["timeout"], HOOK_TIMEOUT_SECS);
        assert_eq!(s["hooks"]["PermissionRequest"][0]["matcher"], "*");
        assert_eq!(s["permissions"]["ask"], json!(["Bash"]));
        assert!(s["hooks"]["SessionStart"][0]["hooks"][0]["command"].is_string());
        let args = launch_args(Path::new("/var/sb/s1/claude-settings.json"));
        assert_eq!(args[2..], ["--permission-mode", "acceptEdits"]);
    }

    #[test]
    fn pretrust_sets_only_the_repo_key_and_keeps_the_rest() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(".claude.json");
        std::fs::write(
            &path,
            r#"{"userID":"u","projects":{"/other":{"hasTrustDialogAccepted":false,"x":1}}}"#,
        )
        .unwrap();
        assert!(pretrust(&path, Path::new("/srv/repo")).unwrap());
        assert!(
            !pretrust(&path, Path::new("/srv/repo")).unwrap(),
            "idempotent"
        );
        let doc: Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(doc["projects"]["/srv/repo"]["hasTrustDialogAccepted"], true);
        assert_eq!(
            doc["projects"]["/other"],
            json!({"hasTrustDialogAccepted": false, "x": 1})
        );
        assert_eq!(doc["userID"], "u");
    }

    #[test]
    fn pretrust_refuses_malformed_config() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(".claude.json");
        std::fs::write(&path, "[1,2]").unwrap();
        assert!(pretrust(&path, Path::new("/srv/repo")).is_err());
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "[1,2]");
    }
}
