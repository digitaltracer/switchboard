//! Permission-hook helper. Asks the runner for a decision and prints it in the agent's hook
//! protocol. Any error, timeout or unreachable runner prints a deny; stdout carries nothing else.

pub mod wire;

use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpStream;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::mpsc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde_json::{Value, json};

use crate::agents::HELPER_DEADLINE;
use crate::cli::Agent;
use wire::{HookReply, HookRequest, MAX_LINE, SecretFile};

const READ_DEADLINE: Duration = Duration::from_secs(5);
const CONNECT_TIMEOUT: Duration = Duration::from_secs(2);

pub fn run(agent: Agent, secret_file: Option<PathBuf>, log: Option<PathBuf>) -> ExitCode {
    let started = Instant::now();
    let input = read_stdin(READ_DEADLINE);
    let event = input
        .as_ref()
        .ok()
        .and_then(|v| v.get("hook_event_name"))
        .and_then(Value::as_str)
        .unwrap_or("PermissionRequest")
        .to_owned();
    let reply = match (&input, &secret_file) {
        (Err(err), _) => HookReply::deny(format!("switchboard: bad hook input: {err}")),
        (_, None) => HookReply::deny("switchboard: no secret file configured"),
        (Ok(payload), Some(path)) => {
            let remaining = HELPER_DEADLINE.saturating_sub(started.elapsed());
            ask_runner(path, agent, &event, payload, remaining)
                .unwrap_or_else(|e| HookReply::deny(format!("switchboard: {e}")))
        }
    };
    let out = render(agent, &event, &reply);
    if let Some(path) = log {
        let _ = append_log(&path, agent, &input, &reply);
    }
    if let Some(out) = out {
        let mut stdout = std::io::stdout().lock();
        let _ = writeln!(stdout, "{out}");
        let _ = stdout.flush();
    }
    ExitCode::SUCCESS
}

/// One round trip to the runner, bounded by `deadline`.
pub fn ask_runner(
    secret_path: &Path,
    agent: Agent,
    event: &str,
    payload: &Value,
    deadline: Duration,
) -> Result<HookReply, String> {
    let secret = SecretFile::read(secret_path).map_err(|e| format!("reading secret file: {e}"))?;
    let started = Instant::now();
    let mut stream = TcpStream::connect_timeout(&secret.addr, CONNECT_TIMEOUT.min(deadline))
        .map_err(|e| format!("runner unreachable: {e}"))?;
    let req = HookRequest {
        session_id: secret.session_id,
        token: secret.token,
        agent,
        event: event.to_owned(),
        payload: payload.clone(),
    };
    let mut line = serde_json::to_vec(&req).map_err(|e| e.to_string())?;
    line.push(b'\n');
    stream
        .write_all(&line)
        .map_err(|e| format!("writing to runner: {e}"))?;
    let remaining = deadline.saturating_sub(started.elapsed());
    if remaining.is_zero() {
        return Err("deadline passed".into());
    }
    stream
        .set_read_timeout(Some(remaining))
        .map_err(|e| e.to_string())?;
    let mut reply = Vec::new();
    BufReader::new(stream)
        .take(MAX_LINE)
        .read_until(b'\n', &mut reply)
        .map_err(|e| format!("no answer from runner: {e}"))?;
    serde_json::from_slice(&reply).map_err(|e| format!("bad answer from runner: {e}"))
}

/// The reply in the agent's hook protocol; `None` prints nothing.
pub fn render(agent: Agent, event: &str, reply: &HookReply) -> Option<Value> {
    if event != "PermissionRequest" && event != "PreToolUse" {
        return None;
    }
    Some(match reply {
        HookReply::Allow { updated_input } if event == "PermissionRequest" => {
            let mut decision = json!({ "behavior": "allow" });
            // Codex fails closed when it sees updatedInput, so only Claude gets it.
            if let (Agent::Claude, Some(input)) = (agent, updated_input) {
                decision["updatedInput"] = input.clone();
            }
            json!({ "hookSpecificOutput": { "hookEventName": event, "decision": decision } })
        }
        HookReply::Deny { message } => deny_output(event, message),
        _ => deny_output(event, "switchboard: no decision"),
    })
}

/// The deny answer in each agent's hook protocol; both CLIs share these shapes.
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
            .take(MAX_LINE)
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
    path: &Path,
    agent: Agent,
    input: &Result<Value, String>,
    reply: &HookReply,
) -> std::io::Result<()> {
    let ts = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0);
    let line = json!({
        "ts_ms": ts,
        "pid": std::process::id(),
        "agent": agent.as_str(),
        "event": input.as_ref().ok().and_then(|v| v.get("hook_event_name")),
        "tool_name": input.as_ref().ok().and_then(|v| v.get("tool_name")),
        "input_error": input.as_ref().err(),
        "reply": reply,
    });
    let mut f = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)?;
    writeln!(f, "{line}")
}

#[cfg(test)]
mod tests;
