//! herdr-core (`hc`) client: JSON-RPC 2.0, one message per line on a Unix socket. Every
//! connection starts with `hello` at generation 1, then carries one request. The client only
//! ever dials: it never starts, stops or restarts a core.
//! This module imports nothing else from the crate.

pub mod events;
pub mod layout;
pub mod types;

#[cfg(test)]
pub(crate) mod fake;

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use serde::de::DeserializeOwned;
use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, AsyncRead, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::net::UnixStream;

use types::*;

/// The protocol generation this client speaks.
pub const GENERATION: u32 = 1;

const MAX_LINE: u64 = 16 << 20;
const DEFAULT_TIMEOUT: Duration = Duration::from_secs(15);
/// `terminal.send` holds Enter while a person at the desktop has a draft, for up to 60 s.
const SEND_WAIT: Duration = Duration::from_secs(65);
/// `terminal.keys` waits for ~3 s of quiet; `terminal.kill` escalates hangup to kill.
const KEYS_WAIT: Duration = Duration::from_secs(10);

#[derive(Debug, thiserror::Error)]
pub enum HcError {
    /// Nothing answered on the socket; the request was never sent.
    #[error("connecting to {path}: {source}")]
    Connect {
        path: PathBuf,
        source: std::io::Error,
    },
    /// `hello` failed, so the request was never sent.
    #[error("hello: {detail}")]
    Handshake { detail: String },
    /// The request was written and may or may not have been applied.
    #[error("{method}: connection failed after sending: {source}")]
    Io {
        method: &'static str,
        source: std::io::Error,
    },
    /// The request was written and may or may not have been applied.
    #[error("{method}: timed out")]
    Timeout { method: &'static str },
    /// A definite refusal, with the core's stable `data.kind`.
    #[error("{method}: {kind}: {message}")]
    Server {
        method: &'static str,
        kind: String,
        message: String,
    },
    #[error("{method}: malformed response: {detail}")]
    Protocol {
        method: &'static str,
        detail: String,
    },
    #[error("herdr-core does not speak protocol generation {GENERATION}: {detail}")]
    Incompatible { detail: String },
    #[error("invalid argument: {0}")]
    InvalidArgument(String),
}

impl HcError {
    /// A mutation that failed this way must be reconciled, not retried blindly.
    pub fn outcome_unknown(&self) -> bool {
        matches!(self, HcError::Io { .. } | HcError::Timeout { .. })
    }

    /// No core is listening: it isn't running, as opposed to slow.
    pub fn server_gone(&self) -> bool {
        matches!(self, HcError::Connect { .. })
    }

    pub fn kind(&self) -> Option<&str> {
        match self {
            HcError::Server { kind, .. } => Some(kind),
            _ => None,
        }
    }
}

pub type Result<T> = std::result::Result<T, HcError>;

/// The core session name: `HC_SESSION`, else `default`.
pub fn session_name(var: &impl Fn(&str) -> Option<String>) -> String {
    var("HC_SESSION")
        .filter(|v| !v.is_empty())
        .unwrap_or_else(|| "default".into())
}

/// The socket `hc` would dial: `HC_SOCKET_PATH`, then `HC_SOCKET`, then
/// `$XDG_RUNTIME_DIR/herdr-core/<s>.sock`, then `/run/user/<uid>/herdr-core/<s>.sock` when that
/// folder exists, else `<XDG_STATE_HOME or ~/.local/state>/herdr-core/<s>/<s>.sock`.
pub fn find_socket(
    var: impl Fn(&str) -> Option<String>,
    home: &Path,
    uid: u32,
    dir_exists: impl Fn(&Path) -> bool,
) -> PathBuf {
    let set = |k: &str| var(k).filter(|v| !v.is_empty());
    if let Some(p) = set("HC_SOCKET_PATH").or_else(|| set("HC_SOCKET")) {
        return PathBuf::from(p);
    }
    let s = session_name(&var);
    let file = format!("{s}.sock");
    if let Some(run) = set("XDG_RUNTIME_DIR") {
        return Path::new(&run).join("herdr-core").join(file);
    }
    let run_user = PathBuf::from(format!("/run/user/{uid}/herdr-core"));
    if dir_exists(&run_user) {
        return run_user.join(file);
    }
    set("XDG_STATE_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| home.join(".local/state"))
        .join("herdr-core")
        .join(&s)
        .join(file)
}

/// `find_socket` from this process's environment.
pub fn socket_from_env(home: &Path) -> PathBuf {
    // SAFETY: getuid has no preconditions and cannot fail.
    let uid = unsafe { libc::getuid() };
    find_socket(|k| std::env::var(k).ok(), home, uid, Path::is_dir)
}

pub struct Client {
    socket: PathBuf,
    timeout: Duration,
    next_id: Arc<AtomicU64>,
}

impl Clone for Client {
    fn clone(&self) -> Self {
        Self {
            socket: self.socket.clone(),
            timeout: self.timeout,
            next_id: self.next_id.clone(),
        }
    }
}

impl Client {
    pub fn new(socket: impl Into<PathBuf>) -> Self {
        Self {
            socket: socket.into(),
            timeout: DEFAULT_TIMEOUT,
            next_id: Arc::new(AtomicU64::new(1)),
        }
    }

    pub fn with_timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }

    pub fn socket(&self) -> &Path {
        &self.socket
    }

    fn id(&self) -> u64 {
        self.next_id.fetch_add(1, Ordering::Relaxed)
    }

    /// Dials and does `hello`; nothing else has been sent when this fails.
    async fn open(&self) -> Result<(BufReader<UnixStream>, Hello)> {
        let fut = async {
            let stream =
                UnixStream::connect(&self.socket)
                    .await
                    .map_err(|source| HcError::Connect {
                        path: self.socket.clone(),
                        source,
                    })?;
            let mut stream = BufReader::new(stream);
            let id = self.id();
            write_request(
                &mut stream,
                id,
                "hello",
                json!({ "generation": GENERATION }),
            )
            .await
            .map_err(|e| HcError::Handshake {
                detail: e.to_string(),
            })?;
            let line = read_line(&mut stream, "hello")
                .await
                .map_err(|e| HcError::Handshake {
                    detail: e.to_string(),
                })?;
            let hello: Hello = match parse_reply("hello", id, &line) {
                Ok(v) => serde_json::from_value(v).map_err(|e| HcError::Handshake {
                    detail: e.to_string(),
                })?,
                Err(e) if e.kind() == Some("unsupported_generation") => {
                    return Err(HcError::Incompatible {
                        detail: e.to_string(),
                    });
                }
                Err(e) => {
                    return Err(HcError::Handshake {
                        detail: e.to_string(),
                    });
                }
            };
            if hello.generation != GENERATION {
                return Err(HcError::Incompatible {
                    detail: format!("it answered generation {}", hello.generation),
                });
            }
            Ok((stream, hello))
        };
        tokio::time::timeout(self.timeout, fut)
            .await
            .map_err(|_| HcError::Handshake {
                detail: "timed out".into(),
            })?
    }

    /// The core's identity and what it offers, from a fresh `hello`.
    pub async fn hello(&self) -> Result<Hello> {
        self.open().await.map(|(_, h)| h)
    }

    /// One request on a fresh connection, after `hello`.
    pub async fn call(&self, method: &'static str, params: Value) -> Result<Value> {
        self.call_waiting(method, params, Duration::ZERO).await
    }

    async fn call_waiting(
        &self,
        method: &'static str,
        params: Value,
        extra: Duration,
    ) -> Result<Value> {
        let (mut stream, _) = self.open().await?;
        let fut = async {
            let id = self.id();
            write_request(&mut stream, id, method, params)
                .await
                .map_err(|source| HcError::Io { method, source })?;
            let line = read_line(&mut stream, method).await?;
            parse_reply(method, id, &line)
        };
        tokio::time::timeout(self.timeout + extra, fut)
            .await
            .map_err(|_| HcError::Timeout { method })?
    }

    async fn typed<T: DeserializeOwned>(&self, method: &'static str, params: Value) -> Result<T> {
        self.typed_waiting(method, params, Duration::ZERO).await
    }

    async fn typed_waiting<T: DeserializeOwned>(
        &self,
        method: &'static str,
        params: Value,
        extra: Duration,
    ) -> Result<T> {
        let v = self.call_waiting(method, params, extra).await?;
        serde_json::from_value(v).map_err(|e| HcError::Protocol {
            method,
            detail: e.to_string(),
        })
    }

    /// Live terminals, this machine's and connected machines'.
    pub async fn list(&self) -> Result<Vec<Terminal>> {
        #[derive(serde::Deserialize)]
        struct Listed {
            terminals: Vec<Terminal>,
        }
        let l: Listed = self.typed("terminal.list", json!({})).await?;
        Ok(l.terminals)
    }

    /// Terminals carrying every one of `labels`, finished ones included.
    pub async fn list_labelled(&self, labels: &BTreeMap<String, String>) -> Result<Vec<Terminal>> {
        #[derive(serde::Deserialize)]
        struct Listed {
            terminals: Vec<Terminal>,
        }
        let l: Listed = self
            .typed("terminal.list", json!({ "all": true, "labels": labels }))
            .await?;
        Ok(l.terminals)
    }

    pub async fn get(&self, terminal: &str) -> Result<Terminal> {
        check_local(terminal)?;
        self.typed("terminal.get", json!({ "terminal": terminal }))
            .await
    }

    /// The screen, plus `lines` of history above it.
    pub async fn read(&self, terminal: &str, lines: u32) -> Result<ReadResult> {
        check_local(terminal)?;
        self.typed(
            "terminal.read",
            json!({ "terminal": terminal, "lines": lines }),
        )
        .await
    }

    /// Pastes `text` and presses Enter. Never forced: the core refuses a blocked program.
    pub async fn send(&self, terminal: &str, text: &str) -> Result<SendResult> {
        check_local(terminal)?;
        self.typed_waiting(
            "terminal.send",
            json!({ "terminal": terminal, "text": text }),
            SEND_WAIT,
        )
        .await
    }

    /// Named keys; the core refuses an unknown name before sending any byte.
    pub async fn keys(&self, terminal: &str, keys: &[String]) -> Result<KeysResult> {
        check_local(terminal)?;
        self.typed_waiting(
            "terminal.keys",
            json!({ "terminal": terminal, "keys": keys }),
            KEYS_WAIT,
        )
        .await
    }

    pub async fn run(&self, params: &RunParams) -> Result<Terminal> {
        self.typed(
            "terminal.run",
            serde_json::to_value(params).expect("run params serialize"),
        )
        .await
    }

    pub async fn kill(&self, terminal: &str) -> Result<Terminal> {
        check_local(terminal)?;
        self.typed_waiting("terminal.kill", json!({ "terminal": terminal }), KEYS_WAIT)
            .await
    }

    /// Removes a finished terminal.
    pub async fn rm(&self, terminal: &str) -> Result<()> {
        check_local(terminal)?;
        self.call("terminal.rm", json!({ "terminal": terminal }))
            .await
            .map(drop)
    }

    pub async fn label(
        &self,
        terminal: &str,
        labels: &BTreeMap<String, String>,
    ) -> Result<Terminal> {
        check_local(terminal)?;
        self.typed(
            "terminal.label",
            json!({ "terminal": terminal, "labels": labels }),
        )
        .await
    }

    pub async fn doc_get(&self, name: &str) -> Result<Doc> {
        self.typed("doc.get", json!({ "name": name })).await
    }

    /// Writes `value` only if the document is still at `version`; `conflict` otherwise.
    pub async fn doc_set(&self, name: &str, value: &Value, version: u64) -> Result<u64> {
        #[derive(serde::Deserialize)]
        struct Set {
            version: u64,
        }
        let s: Set = self
            .typed(
                "doc.set",
                json!({ "name": name, "value": value, "version": version }),
            )
            .await?;
        Ok(s.version)
    }

    /// Opens an event subscription; see `events`.
    pub async fn subscribe(&self) -> Result<events::EventStream> {
        let (mut stream, hello) = self.open().await?;
        const METHOD: &str = "events.subscribe";
        let fut = async {
            let id = self.id();
            write_request(&mut stream, id, METHOD, json!({}))
                .await
                .map_err(|source| HcError::Io {
                    method: METHOD,
                    source,
                })?;
            let line = read_line(&mut stream, METHOD).await?;
            parse_reply(METHOD, id, &line)
        };
        let reply = tokio::time::timeout(self.timeout, fut)
            .await
            .map_err(|_| HcError::Timeout { method: METHOD })??;
        events::EventStream::new(stream, hello.core.id, reply)
    }
}

/// Only this machine's terminals are ever targeted.
pub fn check_local(terminal: &str) -> Result<()> {
    if terminal.is_empty() || terminal.contains('/') {
        return Err(HcError::InvalidArgument(format!("terminal {terminal:?}")));
    }
    Ok(())
}

/// A core address: `t` then letters and digits.
pub fn is_address(s: &str) -> bool {
    s.strip_prefix('t')
        .is_some_and(|r| !r.is_empty() && r.bytes().all(|b| b.is_ascii_alphanumeric()))
}

async fn write_request<W: AsyncWriteExt + Unpin>(
    w: &mut W,
    id: u64,
    method: &str,
    params: Value,
) -> std::io::Result<()> {
    let mut line = serde_json::to_vec(&json!({
        "jsonrpc": "2.0", "id": id, "method": method, "params": params,
    }))
    .expect("requests serialize");
    line.push(b'\n');
    w.write_all(&line).await?;
    w.flush().await
}

pub(crate) async fn read_line<R: AsyncRead + Unpin>(
    reader: &mut BufReader<R>,
    method: &'static str,
) -> Result<String> {
    let mut buf = Vec::new();
    let n = (&mut *reader)
        .take(MAX_LINE)
        .read_until(b'\n', &mut buf)
        .await
        .map_err(|source| HcError::Io { method, source })?;
    if n == 0 {
        return Err(HcError::Io {
            method,
            source: std::io::ErrorKind::UnexpectedEof.into(),
        });
    }
    if buf.last() != Some(&b'\n') {
        return Err(HcError::Protocol {
            method,
            detail: "line too long or truncated".into(),
        });
    }
    String::from_utf8(buf).map_err(|e| HcError::Protocol {
        method,
        detail: e.to_string(),
    })
}

fn parse_reply(method: &'static str, id: u64, line: &str) -> Result<Value> {
    let v: Value = serde_json::from_str(line).map_err(|e| HcError::Protocol {
        method,
        detail: e.to_string(),
    })?;
    if let Some(err) = v.get("error") {
        return Err(HcError::Server {
            method,
            kind: err
                .pointer("/data/kind")
                .and_then(Value::as_str)
                .unwrap_or("unknown")
                .to_owned(),
            message: err
                .get("message")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_owned(),
        });
    }
    if v.get("id").and_then(Value::as_u64) != Some(id) {
        return Err(HcError::Protocol {
            method,
            detail: format!("response id {:?} does not match {id}", v.get("id")),
        });
    }
    v.get("result").cloned().ok_or_else(|| HcError::Protocol {
        method,
        detail: "no result or error".into(),
    })
}

#[cfg(test)]
mod tests;
