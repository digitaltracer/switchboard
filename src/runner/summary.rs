//! The opt-in "while you were away" summary: `claude -p` on the owner's own Claude account, with
//! nothing of the owner's setup able to run. Every built-in tool is off, settings files are
//! ignored (so no hooks or plugins), MCP servers come only from an empty list, skills and
//! commands are off, nothing is saved, and it runs in an empty temporary folder with a time
//! limit. The prompt holds only facts the hub already masked.

use std::process::Stdio;
use std::time::Duration;

use tokio::io::AsyncWriteExt;

pub const TIMEOUT: Duration = Duration::from_secs(60);
pub const MAX_PROMPT_CHARS: usize = 24_000;
pub const MAX_SUMMARY_CHARS: usize = 2_000;

/// The whole command line after `claude`; a test pins it.
pub fn argv(model: &str) -> Vec<String> {
    [
        "-p",
        "--model",
        model,
        "--tools",
        "",
        "--restricted",
        "--strict-mcp-config",
        "--mcp-config",
        r#"{"mcpServers":{}}"#,
        "--disable-slash-commands",
        "--no-session-persistence",
        "--output-format",
        "text",
    ]
    .iter()
    .map(|s| (*s).to_owned())
    .collect()
}

/// A model name as `--model` takes it: `haiku`, `claude-haiku-5-5`.
pub fn valid_model(m: &str) -> bool {
    !m.is_empty()
        && m.len() <= 64
        && m.chars()
            .all(|c| c.is_ascii_alphanumeric() || "-._".contains(c))
}

pub async fn summarize(prompt: &str, model: &str) -> Result<String, String> {
    if !valid_model(model) {
        return Err("not a model name".into());
    }
    let prompt: String = prompt.chars().take(MAX_PROMPT_CHARS).collect();
    let dir = EmptyDir::new().map_err(|e| format!("making an empty folder: {e}"))?;
    let mut child = tokio::process::Command::new("claude")
        .args(argv(model))
        .current_dir(&dir.0)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .kill_on_drop(true)
        .spawn()
        .map_err(|e| format!("starting claude: {e}"))?;
    let mut stdin = child.stdin.take().expect("piped");
    let run = async move {
        stdin.write_all(prompt.as_bytes()).await?;
        drop(stdin);
        child.wait_with_output().await
    };
    let out = tokio::time::timeout(TIMEOUT, run)
        .await
        .map_err(|_| "the summary took too long".to_owned())?
        .map_err(|e| format!("claude: {e}"))?;
    if !out.status.success() {
        return Err("claude didn't finish the summary".into());
    }
    let text: String = String::from_utf8_lossy(&out.stdout)
        .trim()
        .chars()
        .take(MAX_SUMMARY_CHARS)
        .collect();
    if text.is_empty() {
        return Err("claude gave an empty summary".into());
    }
    Ok(text)
}

/// A fresh private folder, removed when dropped.
struct EmptyDir(std::path::PathBuf);

impl EmptyDir {
    fn new() -> std::io::Result<Self> {
        use std::os::unix::fs::DirBuilderExt;
        let path = std::env::temp_dir().join(format!("switchboard-summary-{}", super::random_id()));
        std::fs::DirBuilder::new().mode(0o700).create(&path)?;
        Ok(Self(path))
    }
}

impl Drop for EmptyDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_summary_runs_with_every_tool_setting_hook_and_mcp_server_off() {
        let a = argv("haiku");
        let after = |flag: &str| a.iter().position(|x| x == flag).map(|i| a[i + 1].as_str());
        assert_eq!(a[0], "-p");
        assert_eq!(after("--model"), Some("haiku"));
        assert_eq!(after("--tools"), Some(""), "no built-in tools");
        assert!(
            a.contains(&"--restricted".into()),
            "user, project and local settings, so hooks and plugins, are ignored"
        );
        assert!(a.contains(&"--strict-mcp-config".into()));
        assert_eq!(after("--mcp-config"), Some(r#"{"mcpServers":{}}"#));
        assert!(a.contains(&"--disable-slash-commands".into()));
        assert!(a.contains(&"--no-session-persistence".into()));
        assert!(
            !a.iter()
                .any(|x| x.contains("dangerously") || x == "bypassPermissions" || x == "--bare")
        );
        assert!(valid_model("claude-haiku-5-5") && !valid_model("haiku; rm") && !valid_model(""));
    }
}
