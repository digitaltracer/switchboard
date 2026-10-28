//! An agent's conversation from its own log, for phones: what was said and which tools ran,
//! without the terminal's layout. Claude Code writes `~/.claude/projects/*/<session>.jsonl`,
//! Codex `~/.codex/sessions/<date>/rollout-*.jsonl`. Logs grow to tens of megabytes, so only a
//! window near the asked-for offset is read.

use std::cmp::Reverse;
use std::collections::HashMap;
use std::fs::File;
use std::io::{self, BufRead, BufReader, Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::sync::LazyLock;
use std::time::SystemTime;

use regex::Regex;
use serde_json::Value;

use crate::proto::{TranscriptItem, TranscriptPage};

/// The most of the log one page reads.
const WINDOW: u64 = 1 << 20;
/// The most items one page returns; the oldest are dropped.
const MAX_ITEMS: usize = 300;
const MAX_TEXT: usize = 16_000;
const MAX_INPUT: usize = 2_000;
const MAX_OUTPUT: usize = 4_000;

/// The session a running Claude is on, from the record it keeps at `<claude dir>/sessions/<pid>.json`
/// for the first of `pids` that has one. Herdr's session id can be stale: a background Claude
/// started from the pane reports its own. Only the `.json` records are read.
pub fn live_claude_session(claude_dir: &Path, pids: &[u32]) -> Option<String> {
    pids.iter().find_map(|pid| {
        let text = std::fs::read_to_string(claude_dir.join("sessions").join(format!("{pid}.json")))
            .ok()?;
        let v: Value = serde_json::from_str(&text).ok()?;
        (v.get("pid")?.as_u64()? == u64::from(*pid))
            .then(|| v.get("sessionId")?.as_str().map(str::to_owned))
            .flatten()
    })
}

/// The log of Claude session `id` under `projects` (normally `~/.claude/projects`).
pub fn find_log(projects: &Path, id: &str) -> Option<PathBuf> {
    let valid =
        !id.is_empty() && id.len() <= 64 && id.bytes().all(|b| b.is_ascii_hexdigit() || b == b'-');
    if !valid {
        return None;
    }
    let name = format!("{id}.jsonl");
    std::fs::read_dir(projects)
        .ok()?
        .flatten()
        .map(|d| d.path().join(&name))
        .find(|p| p.is_file())
}

/// The most recent Codex logs looked at for an agent's folder.
const CODEX_CANDIDATES: usize = 200;

/// The newest Codex log started in `cwd`, under `sessions` (normally `~/.codex/sessions`).
/// Herdr reports no session id for Codex, so the folder is the link; a Switchboard session's
/// own worktree makes it exact.
pub fn find_codex_log(sessions: &Path, cwd: &Path) -> Option<PathBuf> {
    let mut logs = Vec::new();
    collect_logs(sessions, 0, &mut logs);
    logs.sort_by_key(|(_, modified)| Reverse(*modified));
    let want = std::fs::canonicalize(cwd).unwrap_or_else(|_| cwd.to_owned());
    logs.into_iter()
        .take(CODEX_CANDIDATES)
        .map(|(p, _)| p)
        .find(|p| codex_cwd(p).is_some_and(|c| std::fs::canonicalize(&c).unwrap_or(c) == want))
}

fn collect_logs(dir: &Path, depth: usize, out: &mut Vec<(PathBuf, SystemTime)>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for e in entries.flatten() {
        let path = e.path();
        let Ok(meta) = e.metadata() else { continue };
        if meta.is_dir() && depth < 4 {
            collect_logs(&path, depth + 1, out);
        } else if path.extension().is_some_and(|x| x == "jsonl")
            && path
                .file_name()
                .is_some_and(|n| n.to_string_lossy().starts_with("rollout-"))
        {
            out.push((path, meta.modified().unwrap_or(SystemTime::UNIX_EPOCH)));
        }
    }
}

/// The folder a Codex log was started in, from its first line.
fn codex_cwd(path: &Path) -> Option<PathBuf> {
    let mut first = String::new();
    BufReader::new(File::open(path).ok()?)
        .take(1 << 20)
        .read_line(&mut first)
        .ok()?;
    let d: Value = serde_json::from_str(&first).ok()?;
    d.pointer("/payload/cwd")
        .and_then(Value::as_str)
        .map(PathBuf::from)
}

/// The page after `after`, before `before`, or else the latest.
pub fn read_page(
    path: &Path,
    after: Option<u64>,
    before: Option<u64>,
) -> io::Result<TranscriptPage> {
    let mut file = File::open(path)?;
    let len = file.metadata()?.len();
    if let Some(a) = after
        && a <= len
        && len - a <= WINDOW
        && line_start(&mut file, a)? == a
    {
        let buf = read_range(&mut file, a, len)?;
        // A last line still being written is left for the next page.
        let end = a + buf
            .iter()
            .rposition(|&b| b == b'\n')
            .map_or(0, |i| i as u64 + 1);
        return Ok(page(a, end, &buf[..(end - a) as usize]));
    }
    let end = line_start(&mut file, before.unwrap_or(len).min(len))?;
    let mut hi = end;
    loop {
        let lo = hi.saturating_sub(WINDOW);
        // One byte early, to tell whether `lo` starts a line.
        let from = lo.saturating_sub(1);
        let buf = read_range(&mut file, from, hi)?;
        let first = match lo {
            0 => Some(0),
            _ => buf
                .iter()
                .position(|&b| b == b'\n')
                .map(|i| from + i as u64 + 1),
        };
        match first {
            Some(s) if s < hi => {
                let mut p = page(s, end, &buf[(s - from) as usize..]);
                p.end = end;
                return Ok(p);
            }
            _ if hi == 0 => return Ok(page(0, end, &[])),
            // The line ending at `hi` is longer than a window, as request copies can be; it's
            // skipped unread and the page reads on from its start.
            _ => hi = line_start(&mut file, lo)?,
        }
    }
}

/// The page made of whole lines `body`, which starts at `start`.
fn page(start: u64, end: u64, body: &[u8]) -> TranscriptPage {
    let mut lines = Vec::new();
    let mut offset = start;
    for line in body.split(|&b| b == b'\n') {
        if let Ok(text) = std::str::from_utf8(line) {
            lines.push((offset, text));
        }
        offset += line.len() as u64 + 1;
    }
    let mut items = parse(lines);
    let mut start = start;
    if items.len() > MAX_ITEMS {
        items.drain(..items.len() - MAX_ITEMS);
        start = items[0].0;
    }
    TranscriptPage {
        items: items.into_iter().map(|(_, i)| i).collect(),
        start,
        end,
    }
}

fn read_range(file: &mut File, lo: u64, hi: u64) -> io::Result<Vec<u8>> {
    file.seek(SeekFrom::Start(lo))?;
    let mut buf = Vec::with_capacity((hi - lo) as usize);
    file.take(hi - lo).read_to_end(&mut buf)?;
    Ok(buf)
}

/// The start of the line `pos` falls in; `pos` itself when a line starts there.
fn line_start(file: &mut File, pos: u64) -> io::Result<u64> {
    let mut hi = pos;
    while hi > 0 {
        let lo = hi.saturating_sub(64 * 1024);
        if let Some(i) = read_range(file, lo, hi)?.iter().rposition(|&b| b == b'\n') {
            return Ok(lo + i as u64 + 1);
        }
        hi = lo;
    }
    Ok(0)
}

/// Items from log lines, each with the offset of the line it came from.
fn parse<'a>(lines: impl IntoIterator<Item = (u64, &'a str)>) -> Vec<(u64, TranscriptItem)> {
    let mut out: Vec<(u64, TranscriptItem)> = Vec::new();
    let mut calls: HashMap<String, usize> = HashMap::new();
    for (offset, line) in lines {
        if line.contains("\"type\":\"response_item\"") {
            if let Ok(d) = serde_json::from_str::<Value>(line) {
                codex(&d, offset, &mut out, &mut calls);
            }
            continue;
        }
        // Request copies and bookkeeping make up most of the log; skip them unparsed.
        if ![
            "\"type\":\"user\"",
            "\"type\":\"assistant\"",
            "\"type\":\"system\"",
        ]
        .iter()
        .any(|t| line.contains(t))
        {
            continue;
        }
        let Ok(d) = serde_json::from_str::<Value>(line) else {
            continue;
        };
        let flag = |k: &str| d.get(k).and_then(Value::as_bool).unwrap_or(false);
        if flag("isSidechain") || flag("isMeta") {
            continue;
        }
        let uuid = d.get("uuid").and_then(Value::as_str).unwrap_or_default();
        let at = d
            .get("timestamp")
            .and_then(Value::as_str)
            .map(str::to_owned);
        let item = |id: String, kind: &str, text: String| TranscriptItem {
            id,
            kind: kind.into(),
            text: Some(text),
            tool: None,
            input: None,
            output: None,
            done: false,
            failed: false,
            at: at.clone(),
        };
        let content = d.pointer("/message/content");
        match d.get("type").and_then(Value::as_str) {
            Some("system") => {
                if d.get("subtype").and_then(Value::as_str) == Some("compact_boundary") {
                    out.push((
                        offset,
                        item(
                            uuid.into(),
                            "note",
                            "Earlier messages were compacted.".into(),
                        ),
                    ));
                }
            }
            Some("user") if flag("isCompactSummary") => {
                out.push((
                    offset,
                    item(
                        uuid.into(),
                        "note",
                        "Earlier messages were summarised.".into(),
                    ),
                ));
            }
            Some("user") => {
                let blocks = match content {
                    Some(Value::String(s)) => vec![serde_json::json!({"type": "text", "text": s})],
                    Some(Value::Array(a)) => a.clone(),
                    _ => continue,
                };
                for (i, b) in blocks.iter().enumerate() {
                    let id = format!("{uuid}:{i}");
                    match b.get("type").and_then(Value::as_str) {
                        Some("text") => {
                            let text = b.get("text").and_then(Value::as_str).unwrap_or_default();
                            if let Some((kind, text)) = said(text) {
                                out.push((offset, item(id, kind, clip(&text, MAX_TEXT))));
                            }
                        }
                        Some("image") => {
                            out.push((offset, item(id, "note", "Attached an image.".into())))
                        }
                        Some("tool_result") => {
                            let call = b
                                .get("tool_use_id")
                                .and_then(Value::as_str)
                                .unwrap_or_default();
                            let output = clip(&result_text(b.get("content")), MAX_OUTPUT);
                            let failed =
                                b.get("is_error").and_then(Value::as_bool).unwrap_or(false);
                            match calls.get(call) {
                                Some(&i) => {
                                    let t = &mut out[i].1;
                                    (t.output, t.done, t.failed) = (Some(output), true, failed);
                                }
                                // Its call is on an earlier page; the phone joins them.
                                None => out.push((
                                    offset,
                                    TranscriptItem {
                                        output: Some(output),
                                        done: true,
                                        failed,
                                        text: None,
                                        ..item(call.into(), "tool", String::new())
                                    },
                                )),
                            }
                        }
                        _ => {}
                    }
                }
            }
            Some("assistant") => {
                let Some(Value::Array(blocks)) = content else {
                    continue;
                };
                for (i, b) in blocks.iter().enumerate() {
                    let id = format!("{uuid}:{i}");
                    match b.get("type").and_then(Value::as_str) {
                        Some("text") => {
                            let text = b
                                .get("text")
                                .and_then(Value::as_str)
                                .unwrap_or_default()
                                .trim();
                            if text.is_empty() {
                                continue;
                            }
                            let kind = if flag("isApiErrorMessage") {
                                "note"
                            } else {
                                "assistant"
                            };
                            out.push((offset, item(id, kind, clip(text, MAX_TEXT))));
                        }
                        Some("tool_use") => {
                            let call = b
                                .get("id")
                                .and_then(Value::as_str)
                                .unwrap_or(&id)
                                .to_owned();
                            let name = b.get("name").and_then(Value::as_str).unwrap_or("tool");
                            let input = b.get("input").cloned().unwrap_or(Value::Null);
                            calls.insert(call.clone(), out.len());
                            out.push((
                                offset,
                                TranscriptItem {
                                    tool: Some(name.into()),
                                    input: Some(clip(
                                        &serde_json::to_string_pretty(&input).unwrap_or_default(),
                                        MAX_INPUT,
                                    )),
                                    ..item(call, "tool", summary(name, &input))
                                },
                            ));
                        }
                        _ => {}
                    }
                }
            }
            _ => {}
        }
    }
    out
}

/// One Codex log entry: a message, or a tool call or its result. Reasoning and the context
/// Codex adds for itself are left out.
fn codex(
    d: &Value,
    offset: u64,
    out: &mut Vec<(u64, TranscriptItem)>,
    calls: &mut HashMap<String, usize>,
) {
    let Some(p) = d.get("payload") else {
        return;
    };
    let at = d
        .get("timestamp")
        .and_then(Value::as_str)
        .map(str::to_owned);
    let s = |k: &str| p.get(k).and_then(Value::as_str);
    let id = s("id").map_or_else(|| format!("codex:{offset}"), str::to_owned);
    let new = |id: String, kind: &str, text: Option<String>| TranscriptItem {
        id,
        kind: kind.into(),
        text,
        tool: None,
        input: None,
        output: None,
        done: false,
        failed: false,
        at: at.clone(),
    };
    match s("type") {
        Some("message") => {
            let Some(Value::Array(blocks)) = p.get("content") else {
                return;
            };
            for (i, b) in blocks.iter().enumerate() {
                let text = b.get("text").and_then(Value::as_str).unwrap_or_default();
                let shown = match s("role") {
                    Some("user") if !text.trim_start().starts_with("# AGENTS.md") => said(text),
                    Some("assistant") if !text.trim().is_empty() => {
                        Some(("assistant", text.trim().to_owned()))
                    }
                    _ => None,
                };
                if let Some((kind, text)) = shown {
                    let item = new(format!("{id}:{i}"), kind, Some(clip(&text, MAX_TEXT)));
                    out.push((offset, item));
                }
            }
        }
        Some(kind @ ("custom_tool_call" | "function_call" | "local_shell_call")) => {
            let call = s("call_id").map_or(id, str::to_owned);
            let name = s("name").unwrap_or("shell");
            let input = match kind {
                "custom_tool_call" => s("input").unwrap_or_default().to_owned(),
                "function_call" => s("arguments").unwrap_or_default().to_owned(),
                _ => p.get("action").map(Value::to_string).unwrap_or_default(),
            };
            calls.insert(call.clone(), out.len());
            let item = TranscriptItem {
                tool: Some(name.into()),
                input: Some(clip(&input, MAX_INPUT)),
                ..new(call, "tool", Some(codex_summary(name, &input)))
            };
            out.push((offset, item));
        }
        Some("custom_tool_call_output" | "function_call_output") => {
            let call = s("call_id").unwrap_or_default().to_owned();
            let output = clip(&codex_output(p.get("output")), MAX_OUTPUT);
            match calls.get(&call) {
                Some(&i) => {
                    let t = &mut out[i].1;
                    (t.output, t.done) = (Some(output), true);
                }
                None => {
                    let item = TranscriptItem {
                        output: Some(output),
                        done: true,
                        ..new(call, "tool", None)
                    };
                    out.push((offset, item));
                }
            }
        }
        _ => {}
    }
}

/// The command a Codex tool call ran: inside its exec script, or a shell call's arguments.
fn codex_summary(name: &str, input: &str) -> String {
    static CMD: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r#""?(?:cmd|command)"?\s*:\s*"((?:[^"\\]|\\.)*)""#).expect("valid")
    });
    let ran = |cmd: &str| {
        let line = cmd.lines().next().unwrap_or_default();
        format!("Ran {}", clip(line, 200))
    };
    if let Some(c) = CMD.captures(input) {
        let cmd: String =
            serde_json::from_str(&format!("\"{}\"", &c[1])).unwrap_or_else(|_| c[1].to_owned());
        return ran(&cmd);
    }
    let parts = serde_json::from_str::<Value>(input).ok().and_then(|v| {
        let a = v
            .get("command")
            .or_else(|| v.get("cmd"))?
            .as_array()?
            .clone();
        Some(
            a.iter()
                .filter_map(Value::as_str)
                .map(str::to_owned)
                .collect::<Vec<_>>(),
        )
    });
    match parts.as_deref() {
        // `bash -lc "<script>"` shows the script.
        Some([.., last]) => ran(last),
        _ => match name {
            "exec" | "shell" | "exec_command" => "Ran a command".into(),
            n => format!("Used {n}"),
        },
    }
}

fn codex_output(o: Option<&Value>) -> String {
    match o {
        Some(Value::String(s)) => s.clone(),
        Some(Value::Array(blocks)) => blocks
            .iter()
            .filter_map(|b| b.get("text").and_then(Value::as_str))
            .collect::<Vec<_>>()
            .join("\n"),
        Some(Value::Object(m)) => m
            .get("content")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_owned(),
        _ => String::new(),
    }
}

/// What a user line shows: a prompt, a `!` command or a slash command. Other tagged lines
/// (command output, reminders, notifications) are Claude's own and shown nowhere.
fn said(text: &str) -> Option<(&'static str, String)> {
    let text = text.trim();
    if text.is_empty() {
        return None;
    }
    if !text.starts_with('<') {
        return Some(("user", text.to_owned()));
    }
    let tag = |name: &str| {
        let open = format!("<{name}>");
        let rest = &text[text.find(&open)? + open.len()..];
        Some(rest[..rest.find(&format!("</{name}>"))?].trim().to_owned())
    };
    if let Some(cmd) = tag("bash-input") {
        return Some(("note", format!("! {cmd}")));
    }
    let name = tag("command-name")?;
    let args = tag("command-args").unwrap_or_default();
    Some(("note", format!("{name} {args}").trim().to_owned()))
}

fn result_text(content: Option<&Value>) -> String {
    match content {
        Some(Value::String(s)) => s.clone(),
        Some(Value::Array(blocks)) => blocks
            .iter()
            .map(|b| match b.get("type").and_then(Value::as_str) {
                Some("text") => b
                    .get("text")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_owned(),
                Some("image") => "[image]".into(),
                _ => String::new(),
            })
            .filter(|s| !s.is_empty())
            .collect::<Vec<_>>()
            .join("\n"),
        _ => String::new(),
    }
}

/// One line saying what a tool call did, like Claude's own transcript.
fn summary(name: &str, input: &Value) -> String {
    let s = |k: &str| input.get(k).and_then(Value::as_str).unwrap_or_default();
    let first_line = |t: &str| clip(t.lines().next().unwrap_or_default(), 200);
    match name {
        "Bash" => format!("Ran {}", first_line(s("command"))),
        "Read" => format!("Read {}", s("file_path")),
        "Edit" | "MultiEdit" => format!("Edited {}", s("file_path")),
        "Write" => format!("Wrote {}", s("file_path")),
        "NotebookEdit" => format!("Edited {}", s("notebook_path")),
        "Grep" => format!("Searched for {}", first_line(s("pattern"))),
        "Glob" => format!("Listed {}", s("pattern")),
        "WebFetch" => format!("Fetched {}", s("url")),
        "WebSearch" => format!("Searched the web for {}", s("query")),
        "Task" | "Agent" => format!("Ran an agent: {}", first_line(s("description"))),
        "TodoWrite" => "Updated the to-do list".into(),
        "ExitPlanMode" => "Proposed a plan".into(),
        "AskUserQuestion" => {
            let q = input
                .pointer("/questions/0/question")
                .and_then(Value::as_str);
            format!("Asked: {}", first_line(q.unwrap_or_default()))
        }
        n => match n.strip_prefix("mcp__").and_then(|r| r.split_once("__")) {
            Some((server, tool)) => format!("Used {server} · {tool}"),
            None => format!("Used {n}"),
        },
    }
}

/// At most `max` bytes, cut on a character boundary.
fn clip(s: &str, max: usize) -> String {
    if s.len() <= max {
        return s.to_owned();
    }
    let mut cut = max;
    while !s.is_char_boundary(cut) {
        cut -= 1;
    }
    format!("{}…", &s[..cut])
}

#[cfg(test)]
#[path = "transcript_tests.rs"]
mod tests;
