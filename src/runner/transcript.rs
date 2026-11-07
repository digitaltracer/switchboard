//! An agent's conversation from its own log, for phones: what was said and which tools ran,
//! without the terminal's layout. Claude Code writes `~/.claude/projects/*/<session>.jsonl`,
//! Codex `~/.codex/sessions/<date>/rollout-*.jsonl`. Logs grow to tens of megabytes, so only a
//! window near the asked-for offset is read.

use std::cmp::Reverse;
use std::collections::HashMap;
use std::fs::File;
use std::io::{self, BufRead, BufReader, Read, Seek, SeekFrom};
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{LazyLock, Mutex};
use std::time::SystemTime;

use regex::Regex;
use serde_json::Value;

use crate::proto::{TranscriptItem, TranscriptPage};

/// The most of the log one page reads.
pub const WINDOW: u64 = 1 << 20;
/// The most of a log's end read for its last line.
pub const TAIL: u64 = 256 * 1024;
/// The most characters of a last line kept.
pub const MAX_LINE: usize = 200;
/// The most items one page returns; the oldest are dropped.
const MAX_ITEMS: usize = 300;
/// The most of a message's text kept; a longer one is cut there and ends in "…".
pub const MAX_TEXT: usize = 16_000;
const MAX_INPUT: usize = 2_000;
const MAX_OUTPUT: usize = 4_000;

/// The session a running Claude is on, from the record it keeps at `<claude dir>/sessions/<pid>.json`
/// for the first of `pids` that has one. Herdr's session id can be stale: a background Claude
/// started from the pane reports its own. A pane whose session was handed to a background job
/// (a resumed or forked session) names that job, and the job's record has the session it is on
/// now. Only the `.json` records are read.
pub fn live_claude_session(claude_dir: &Path, pids: &[u32]) -> Option<String> {
    live_claude_record(claude_dir, pids).map(|(_, s)| s)
}

/// As `live_claude_session`, with the pid whose record named it.
pub fn live_claude_record(claude_dir: &Path, pids: &[u32]) -> Option<(u32, String)> {
    let sessions = claude_dir.join("sessions");
    let record = |path: &Path| -> Option<Value> {
        serde_json::from_str(&std::fs::read_to_string(path).ok()?).ok()
    };
    let id = |v: &Value| v.get("sessionId")?.as_str().map(str::to_owned);
    pids.iter().find_map(|pid| {
        let v = record(&sessions.join(format!("{pid}.json")))?;
        if v.get("pid")?.as_u64()? != u64::from(*pid) {
            return None;
        }
        let parked = v
            .get("parkedJobId")
            .and_then(Value::as_str)
            .filter(|j| !j.is_empty());
        let job = parked.and_then(|job| {
            std::fs::read_dir(&sessions)
                .ok()?
                .flatten()
                .map(|e| e.path())
                .filter(|p| p.extension().is_some_and(|x| x == "json"))
                .filter_map(|p| record(&p))
                .find(|r| r.get("jobId").and_then(Value::as_str) == Some(job))
                .and_then(|r| id(&r))
        });
        job.or_else(|| id(&v)).map(|s| (*pid, s))
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
    codex_logs(sessions, cwd, &CodexHeaders::default())
        .into_iter()
        .next()
}

/// Every Codex log among the latest started in `cwd`, newest-modified first. `headers` remembers
/// each log's folder, so only new or changed logs are opened.
pub fn codex_logs(sessions: &Path, cwd: &Path, headers: &CodexHeaders) -> Vec<PathBuf> {
    let mut logs = Vec::new();
    collect_logs(sessions, 0, &mut logs);
    logs.sort_by_key(|(_, modified, _)| Reverse(*modified));
    logs.truncate(CODEX_CANDIDATES);
    let want = std::fs::canonicalize(cwd).unwrap_or_else(|_| cwd.to_owned());
    let mut memo = headers.0.lock().unwrap();
    let mut seen = HashMap::with_capacity(logs.len());
    let mut out = Vec::new();
    for (path, modified, ino) in logs {
        let stamp = (ino, modified);
        let cwd = match memo.remove(&path) {
            Some((s, cwd)) if s == stamp => cwd,
            _ => {
                headers.1.fetch_add(1, Ordering::Relaxed);
                codex_cwd(&path).map(|c| std::fs::canonicalize(&c).unwrap_or(c))
            }
        };
        if cwd.as_deref() == Some(want.as_path()) {
            out.push(path.clone());
        }
        seen.insert(path, (stamp, cwd));
    }
    // Only the logs just looked at are kept, so the memo stays as small as the candidates.
    *memo = seen;
    out
}

/// A log's inode and modification time, and the folder its first line names.
type Header = ((u64, SystemTime), Option<PathBuf>);

/// Each Codex log's folder, by path, kept while its inode and modification time stay the same.
#[derive(Default)]
pub struct CodexHeaders(Mutex<HashMap<PathBuf, Header>>, AtomicU64);

impl CodexHeaders {
    /// First lines read so far.
    pub fn reads(&self) -> u64 {
        self.1.load(Ordering::Relaxed)
    }
}

fn collect_logs(dir: &Path, depth: usize, out: &mut Vec<(PathBuf, SystemTime, u64)>) {
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
            let modified = meta.modified().unwrap_or(SystemTime::UNIX_EPOCH);
            out.push((path, modified, meta.ino()));
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

/// The id of the latest tool call if it has no result yet: Claude's tool_use id or Codex's
/// call_id, for the dialog that call is waiting at.
pub fn pending_call(path: &Path) -> Option<String> {
    let page = read_page(path, None, None).ok()?;
    let last = page.items.iter().rev().find(|i| i.kind == "tool")?;
    (!last.done).then(|| last.id.clone())
}

/// A log's last line and when it was written.
pub type LastLine = (String, Option<String>);

/// The first line of the last thing the agent said or did, as a phone's tile shows it, with
/// when it was written, from the last `TAIL` bytes. Masked on the whole text before it's cut.
/// Also the bytes read.
pub fn last_line(path: &Path) -> io::Result<(Option<LastLine>, u64)> {
    let mut file = File::open(path)?;
    let len = file.metadata()?.len();
    let lo = len.saturating_sub(TAIL);
    // One byte early, to tell whether `lo` starts a line.
    let from = lo.saturating_sub(1);
    let buf = read_range(&mut file, from, len)?;
    let read = buf.len() as u64;
    let first = match lo {
        0 => 0,
        _ => buf
            .iter()
            .position(|&b| b == b'\n')
            .map_or(buf.len(), |i| i + 1),
    };
    // A last line still being written is left out.
    let end = buf.iter().rposition(|&b| b == b'\n').map_or(0, |i| i + 1);
    let body = buf.get(first..end.max(first)).unwrap_or_default();
    let p = page(from + first as u64, from + end as u64, body);
    let last = p
        .items
        .into_iter()
        .rev()
        .find(|i| i.kind != "user" && i.text.is_some());
    let line = last.and_then(|i| {
        let text = crate::redact::redact_str(i.text.as_deref()?);
        let line = text.lines().find(|l| !l.trim().is_empty())?;
        Some((clip_chars(line, MAX_LINE), i.at))
    });
    Ok((line, read))
}

/// At most `max` characters, ending in "…" when cut.
fn clip_chars(s: &str, max: usize) -> String {
    match s.char_indices().nth(max) {
        Some((cut, _)) => format!("{}…", &s[..cut]),
        None => s.to_owned(),
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
            added: None,
            removed: None,
            ends_turn: false,
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
                            let stop = d.pointer("/message/stop_reason").and_then(Value::as_str);
                            out.push((
                                offset,
                                TranscriptItem {
                                    ends_turn: kind == "assistant"
                                        && matches!(stop, Some("end_turn" | "stop_sequence")),
                                    ..item(id, kind, clip(text, MAX_TEXT))
                                },
                            ));
                        }
                        Some("tool_use") => {
                            let call = b
                                .get("id")
                                .and_then(Value::as_str)
                                .unwrap_or(&id)
                                .to_owned();
                            let name = b.get("name").and_then(Value::as_str).unwrap_or("tool");
                            let input = b.get("input").cloned().unwrap_or(Value::Null);
                            let (added, removed) = edit_lines(name, &input).unzip();
                            calls.insert(call.clone(), out.len());
                            out.push((
                                offset,
                                TranscriptItem {
                                    tool: Some(name.into()),
                                    added,
                                    removed,
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
        added: None,
        removed: None,
        ends_turn: false,
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
                    let item = TranscriptItem {
                        ends_turn: kind == "assistant" && s("phase") == Some("final_answer"),
                        ..new(format!("{id}:{i}"), kind, Some(clip(&text, MAX_TEXT)))
                    };
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
            let (summary, lines) = match name {
                "apply_patch" => patch_summary(&input),
                _ => (codex_summary(name, &input), None),
            };
            let (added, removed) = lines.unzip();
            let item = TranscriptItem {
                tool: Some(name.into()),
                input: Some(clip(&input, MAX_INPUT)),
                added,
                removed,
                ..new(call, "tool", Some(summary))
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

/// What a user line shows: a prompt, a `!` command or a slash command, all the person's own
/// messages. Other tagged lines (command output, reminders, notifications) are Claude's own and
/// shown nowhere.
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
        return Some(("user", format!("! {cmd}")));
    }
    let name = tag("command-name")?;
    let args = tag("command-args").unwrap_or_default();
    Some(("user", format!("{name} {args}").trim().to_owned()))
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

/// Lines a Claude edit adds and removes, from its whole input before it's shortened.
fn edit_lines(name: &str, input: &Value) -> Option<(u32, u32)> {
    let lines = |k: &str, v: &Value| {
        v.get(k)
            .and_then(Value::as_str)
            .map_or(0, |t| t.lines().count() as u32)
    };
    match name {
        "Edit" => Some((lines("new_string", input), lines("old_string", input))),
        "MultiEdit" => input.get("edits")?.as_array().map(|edits| {
            edits.iter().fold((0, 0), |(a, r), e| {
                (a + lines("new_string", e), r + lines("old_string", e))
            })
        }),
        "Write" => Some((lines("content", input), 0)),
        _ => None,
    }
}

/// A Codex patch: the files it touches and the lines it adds and removes.
fn patch_summary(patch: &str) -> (String, Option<(u32, u32)>) {
    let mut files = Vec::new();
    let (mut added, mut removed) = (0, 0);
    for line in patch.lines() {
        if let Some(rest) = line.strip_prefix("*** ") {
            for verb in ["Update File: ", "Add File: ", "Delete File: "] {
                if let Some(f) = rest.strip_prefix(verb) {
                    files.push(f.trim().to_owned());
                }
            }
        } else if line.starts_with('+') {
            added += 1;
        } else if line.starts_with('-') {
            removed += 1;
        }
    }
    let summary = match files.as_slice() {
        [] => "Edited files".to_owned(),
        [one] => format!("Edited {one}"),
        [first, rest @ ..] => format!("Edited {first} and {} more", rest.len()),
    };
    (summary, Some((added, removed)))
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
