//! One `events.subscribe` connection: the reply's snapshot first, then each change as the core
//! sends it. `None` from `next` means events may have been lost (a drop, or 45 s with not even a
//! heartbeat): resubscribe for a fresh snapshot.

use std::collections::BTreeMap;
use std::time::Duration;

use serde_json::Value;
use tokio::io::BufReader;
use tokio::net::UnixStream;
use tokio::sync::mpsc;
use tokio::task::JoinHandle;
use tokio::time::Instant;

use super::types::{Foreground, Status, Terminal};
use super::{HcError, Result, read_line};

/// The core sends a heartbeat every 15 s; three missed in a row is a dead stream.
pub const SILENCE: Duration = Duration::from_secs(45);

const METHOD: &str = "events.subscribe";
/// Lines read ahead of `next`; a full buffer holds the reader back.
const AHEAD: usize = 256;

#[derive(Debug, Clone, PartialEq)]
pub enum Item {
    /// The terminals as of `seq`; always the first item.
    Snapshot {
        seq: u64,
        terminals: Vec<Terminal>,
    },
    /// One fact changed. `terminal` is absent for document changes.
    Delta {
        seq: u64,
        terminal: Option<String>,
        change: Change,
    },
    Heartbeat(u64),
    /// The core dropped events for this subscriber.
    Missed,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Change {
    Created(Box<Terminal>),
    Foreground(Option<Foreground>),
    Title(String),
    Status {
        status: Option<Status>,
        reported: Option<bool>,
        status_ms: Option<u64>,
    },
    Turn(u64),
    Labels(BTreeMap<String, String>),
    Finished,
    Removed,
    Lost,
    /// The layout or another document was written.
    Doc {
        name: String,
    },
    /// Anything else (`restarted`, `keep`, `size`, ...), with its data.
    Other {
        kind: String,
        data: Value,
    },
}

/// Lines are read by a task of its own, so a `next` dropped mid-line loses nothing.
pub struct EventStream {
    lines: mpsc::Receiver<Result<String>>,
    reader: JoinHandle<()>,
    core_id: String,
    first: Option<Item>,
    silence: Duration,
    /// When `next` last gave an item: silence counts from it, across dropped `next` calls.
    last: Instant,
}

impl Drop for EventStream {
    fn drop(&mut self) {
        self.reader.abort();
    }
}

impl EventStream {
    pub(super) fn new(
        reader: BufReader<UnixStream>,
        core_id: String,
        reply: Value,
    ) -> Result<Self> {
        let first = snapshot(&reply).ok_or_else(|| HcError::Protocol {
            method: METHOD,
            detail: "subscribe reply has no seq or terminals".into(),
        })?;
        let (tx, lines) = mpsc::channel(AHEAD);
        let mut reader = reader;
        let reader = tokio::spawn(async move {
            loop {
                let line = read_line(&mut reader, METHOD).await;
                let end = line.is_err();
                if tx.send(line).await.is_err() || end {
                    return;
                }
            }
        });
        Ok(Self {
            lines,
            reader,
            core_id,
            first: Some(first),
            silence: SILENCE,
            last: Instant::now(),
        })
    }

    pub fn with_silence(mut self, silence: Duration) -> Self {
        self.silence = silence;
        self
    }

    /// The id of the core this stream belongs to, from its `hello`.
    pub fn core_id(&self) -> &str {
        &self.core_id
    }

    /// Cancel-safe.
    pub async fn next(&mut self) -> Option<Result<Item>> {
        if let Some(first) = self.first.take() {
            self.last = Instant::now();
            return Some(Ok(first));
        }
        let line = match tokio::time::timeout_at(self.last + self.silence, self.lines.recv()).await
        {
            Ok(Some(Ok(line))) => line,
            _ => return None,
        };
        self.last = Instant::now();
        Some(parse(&line))
    }
}

fn snapshot(v: &Value) -> Option<Item> {
    Some(Item::Snapshot {
        seq: v.get("seq")?.as_u64()?,
        terminals: serde_json::from_value(v.get("terminals")?.clone()).ok()?,
    })
}

fn bad(detail: impl Into<String>) -> HcError {
    HcError::Protocol {
        method: METHOD,
        detail: detail.into(),
    }
}

pub fn parse(line: &str) -> Result<Item> {
    let v: Value = serde_json::from_str(line).map_err(|e| bad(e.to_string()))?;
    let params = v.get("params").cloned().unwrap_or(Value::Null);
    match v.get("method").and_then(Value::as_str) {
        Some("heartbeat") => Ok(Item::Heartbeat(
            params.get("seq").and_then(Value::as_u64).unwrap_or(0),
        )),
        Some("missed") => Ok(Item::Missed),
        Some("event") => {
            let seq = params
                .get("seq")
                .and_then(Value::as_u64)
                .ok_or_else(|| bad("event without seq"))?;
            let kind = params
                .get("type")
                .and_then(Value::as_str)
                .ok_or_else(|| bad("event without type"))?;
            let terminal = params
                .get("terminal")
                .and_then(Value::as_str)
                .map(str::to_owned);
            let data = params.get("data").cloned().unwrap_or(Value::Null);
            Ok(Item::Delta {
                seq,
                terminal,
                change: change(kind, data)?,
            })
        }
        other => Err(bad(format!("unexpected message {other:?}"))),
    }
}

fn change(kind: &str, data: Value) -> Result<Change> {
    let de = |e: serde_json::Error| bad(format!("{kind}: {e}"));
    Ok(match kind {
        "created" => Change::Created(Box::new(serde_json::from_value(data).map_err(de)?)),
        "foreground" => Change::Foreground(if data.is_null() || data.get("pid").is_none() {
            None
        } else {
            Some(serde_json::from_value(data).map_err(de)?)
        }),
        "title" => Change::Title(
            data.get("title")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_owned(),
        ),
        "status" => Change::Status {
            status: match data.get("status") {
                None | Some(Value::Null) => None,
                Some(s) => Some(serde_json::from_value(s.clone()).map_err(de)?),
            },
            reported: data.get("reported").and_then(Value::as_bool),
            status_ms: data.get("status_ms").and_then(Value::as_u64),
        },
        "turn" => Change::Turn(data.get("turn").and_then(Value::as_u64).unwrap_or(0)),
        "labels" => Change::Labels(
            serde_json::from_value(
                data.get("labels")
                    .cloned()
                    .unwrap_or_else(|| Value::Object(Default::default())),
            )
            .map_err(de)?,
        ),
        "finished" => Change::Finished,
        "removed" => Change::Removed,
        "lost" => Change::Lost,
        "doc" => Change::Doc {
            name: data
                .get("name")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_owned(),
        },
        _ => Change::Other {
            kind: kind.to_owned(),
            data,
        },
    })
}
