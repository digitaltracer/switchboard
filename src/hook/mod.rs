//! Permission-hook helper. Stub: reads the hook input, logs it, and denies.
//! The runner round trip arrives with implementation step 5.

use std::io::{Read, Write};
use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::mpsc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde_json::{Value, json};

use crate::cli::Agent;

const MAX_INPUT: u64 = 1 << 20;
const READ_DEADLINE: Duration = Duration::from_secs(5);

pub fn run(agent: Agent, _secret_file: Option<PathBuf>, log: Option<PathBuf>) -> ExitCode {
    let input = read_stdin(READ_DEADLINE);
    let event = input
        .as_ref()
        .ok()
        .and_then(|v| v.get("hook_event_name"))
        .and_then(Value::as_str)
        .unwrap_or("PermissionRequest")
        .to_owned();
    let reason = match &input {
        Ok(_) => "switchboard: no runner connected (hook stub)".to_owned(),
        Err(err) => format!("switchboard: bad hook input: {err}"),
    };
    let out = deny_output(&event, &reason);
    if let Some(path) = log {
        let _ = append_log(&path, agent, &input, &out);
    }
    let mut stdout = std::io::stdout().lock();
    let _ = writeln!(stdout, "{out}");
    let _ = stdout.flush();
    ExitCode::SUCCESS
}

/// The deny answer; Claude Code and Codex share these shapes.
pub fn deny_output(event: &str, reason: &str) -> Value {
    if event == "PreToolUse" {
        json!({
            "hookSpecificOutput": {
                "hookEventName": "PreToolUse",
                "permissionDecision": "deny",
                "permissionDecisionReason": reason,
            }
        })
    } else {
        json!({
            "hookSpecificOutput": {
                "hookEventName": event,
                "decision": { "behavior": "deny", "message": reason },
            }
        })
    }
}

fn read_stdin(deadline: Duration) -> Result<Value, String> {
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        let mut buf = Vec::new();
        let res = std::io::stdin()
            .take(MAX_INPUT)
            .read_to_end(&mut buf)
            .map(|_| buf);
        let _ = tx.send(res);
    });
    match rx.recv_timeout(deadline) {
        Ok(Ok(buf)) => serde_json::from_slice(&buf).map_err(|e| e.to_string()),
        Ok(Err(e)) => Err(e.to_string()),
        Err(_) => Err("timed out reading stdin".into()),
    }
}

fn append_log(
    path: &PathBuf,
    agent: Agent,
    input: &Result<Value, String>,
    out: &Value,
) -> std::io::Result<()> {
    let ts = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0);
    let line = json!({
        "ts_ms": ts,
        "pid": std::process::id(),
        "agent": agent.as_str(),
        "input": input.as_ref().ok(),
        "input_error": input.as_ref().err(),
        "output": out,
    });
    let mut f = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)?;
    writeln!(f, "{line}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn permission_request_deny_shape() {
        let out = deny_output("PermissionRequest", "no");
        assert_eq!(
            out["hookSpecificOutput"]["hookEventName"],
            "PermissionRequest"
        );
        assert_eq!(out["hookSpecificOutput"]["decision"]["behavior"], "deny");
    }

    #[test]
    fn pre_tool_use_deny_shape() {
        let out = deny_output("PreToolUse", "no");
        assert_eq!(out["hookSpecificOutput"]["permissionDecision"], "deny");
    }
}
