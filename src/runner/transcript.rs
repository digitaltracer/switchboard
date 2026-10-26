//! An agent's conversation from Claude Code's own log (`~/.claude/projects/*/<session>.jsonl`),
//! for phones: what was said and which tools ran, without the terminal's layout. Logs grow to
//! tens of megabytes, so only a window near the asked-for offset is read.

use std::collections::HashMap;
use std::fs::File;
use std::io::{self, Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};

use serde_json::Value;

use crate::proto::{TranscriptItem, TranscriptPage};

/// The most of the log one page reads.
const WINDOW: u64 = 1 << 20;
/// The most items one page returns; the oldest are dropped.
const MAX_ITEMS: usize = 300;
const MAX_TEXT: usize = 16_000;
const MAX_INPUT: usize = 2_000;
const MAX_OUTPUT: usize = 4_000;

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
