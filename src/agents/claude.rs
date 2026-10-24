//! Claude Code profile. Verified against 2.1.302 (spike-findings.md).

use std::io;
use std::path::Path;

use serde::Deserialize;
use serde_json::{Value, json};

use super::HOOK_TIMEOUT_SECS;

/// What Claude Code runs without asking. Whatever still asks goes to the hook either way.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize)]
pub enum PermissionMode {
    /// Claude's safety check runs routine actions and refuses risky ones itself; questions
    /// and plans still come to you.
    #[default]
    #[serde(rename = "auto")]
    Auto,
    /// Edits run; every shell command and every other tool asks you.
    #[serde(rename = "acceptEdits")]
    AcceptEdits,
}

impl PermissionMode {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Auto => "auto",
            Self::AcceptEdits => "acceptEdits",
        }
    }
}

/// Settings passed with `--settings`, kept outside the worktree.
/// `onFailure: block` turns a killed, failed or timed-out helper into a deny.
pub fn settings(hook_command: &str, mode: PermissionMode, deny_patterns: bool) -> Value {
    let hook = |timeout| {
        json!([{
            "matcher": "*",
            "hooks": [{ "type": "command", "command": hook_command, "timeout": timeout, "onFailure": "block" }],
        }])
    };
    let mut s = json!({
        "hooks": {
            "PermissionRequest": hook(HOOK_TIMEOUT_SECS),
            // Proves the hook settings are loaded and the helper reaches the runner.
            "SessionStart": [{
                "hooks": [{ "type": "command", "command": hook_command, "timeout": 30 }],
            }],
        }
    });
    match mode {
        // acceptEdits would otherwise run touch, rm, mv and redirects without asking the hook.
        PermissionMode::AcceptEdits => s["permissions"] = json!({ "ask": ["Bash"] }),
        // Auto mode asks for little, so deny patterns are checked before every tool instead.
        PermissionMode::Auto if deny_patterns => s["hooks"]["PreToolUse"] = hook(30),
        PermissionMode::Auto => {}
    }
    s
}

/// Set explicitly: a user's own default mode may be anything.
pub fn launch_args(settings_path: &Path, mode: PermissionMode) -> Vec<String> {
    vec![
        "--settings".into(),
        settings_path.to_string_lossy().into_owned(),
        "--permission-mode".into(),
        mode.as_str().into(),
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

    const HOOK: &str = "/bin/switchboard hook --agent claude --secret-file /s";

    #[test]
    fn permission_hook_fails_closed() {
        for mode in [PermissionMode::Auto, PermissionMode::AcceptEdits] {
            let s = settings(HOOK, mode, false);
            let h = &s["hooks"]["PermissionRequest"][0]["hooks"][0];
            assert_eq!(h["onFailure"], "block");
            assert_eq!(h["timeout"], HOOK_TIMEOUT_SECS);
            assert_eq!(s["hooks"]["PermissionRequest"][0]["matcher"], "*");
            assert!(s["hooks"]["SessionStart"][0]["hooks"][0]["command"].is_string());
            let args = launch_args(Path::new("/var/sb/s1/claude-settings.json"), mode);
            assert_eq!(args[2..], ["--permission-mode", mode.as_str()]);
        }
    }

    #[test]
    fn auto_mode_asks_little_and_checks_deny_patterns_before_every_tool() {
        let auto = settings(HOOK, PermissionMode::Auto, false);
        assert!(
            auto.get("permissions").is_none(),
            "shell commands don't all ask"
        );
        assert!(auto["hooks"].get("PreToolUse").is_none());
        let guarded = settings(HOOK, PermissionMode::Auto, true);
        let pre = &guarded["hooks"]["PreToolUse"][0];
        assert_eq!(
            (
                pre["matcher"].as_str(),
                pre["hooks"][0]["onFailure"].as_str()
            ),
            (Some("*"), Some("block"))
        );

        let strict = settings(HOOK, PermissionMode::AcceptEdits, true);
        assert_eq!(strict["permissions"]["ask"], json!(["Bash"]));
        assert!(
            strict["hooks"].get("PreToolUse").is_none(),
            "every shell command asks already"
        );
        assert_eq!(PermissionMode::default(), PermissionMode::Auto);
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
