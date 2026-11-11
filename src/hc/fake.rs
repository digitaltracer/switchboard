//! A scripted herdr-core for tests: JSON-RPC lines on a Unix socket in a temp folder, with
//! terminals held as raw records and an event stream the test drives. It never runs anything.
#![allow(dead_code)]

use std::collections::{HashMap, VecDeque};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::UnixListener;
use tokio::sync::mpsc;

use super::Client;

/// A canned answer for the next call of a method.
#[derive(Debug, Clone)]
pub enum Script {
    Result(Value),
    /// An error with this `data.kind`.
    Error(&'static str),
    /// Read the request, then close without answering.
    Drop,
    /// Read the request and never answer.
    Hang,
}

#[derive(Default)]
struct State {
    core_id: String,
    seq: u64,
    terminals: Vec<Value>,
    screens: HashMap<String, VecDeque<String>>,
    doc: (Value, u64),
    requests: Vec<Value>,
    scripted: HashMap<String, VecDeque<Script>>,
    subscribers: Vec<mpsc::UnboundedSender<String>>,
    next: u32,
    hellos: u32,
}

pub struct FakeCore {
    _dir: tempfile::TempDir,
    pub socket: PathBuf,
    state: Arc<Mutex<State>>,
}

const KEYS: &[&str] = &[
    "enter", "esc", "up", "down", "left", "right", "tab", "space",
];

impl FakeCore {
    pub fn start() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let socket = dir.path().join("core.sock");
        let listener = UnixListener::bind(&socket).unwrap();
        let state = Arc::new(Mutex::new(State {
            core_id: "core-a".into(),
            doc: (Value::Null, 0),
            next: 1,
            ..Default::default()
        }));
        let shared = state.clone();
        tokio::spawn(async move {
            while let Ok((stream, _)) = listener.accept().await {
                tokio::spawn(serve(stream, shared.clone()));
            }
        });
        Self {
            _dir: dir,
            socket,
            state,
        }
    }

    pub fn client(&self) -> Client {
        Client::new(&self.socket).with_timeout(std::time::Duration::from_secs(2))
    }

    /// Every request but `hello`, in order.
    pub fn requests(&self) -> Vec<Value> {
        self.state.lock().unwrap().requests.clone()
    }

    pub fn methods(&self) -> Vec<String> {
        self.requests()
            .iter()
            .map(|r| r["method"].as_str().unwrap_or_default().to_owned())
            .collect()
    }

    pub fn hellos(&self) -> u32 {
        self.state.lock().unwrap().hellos
    }

    pub fn script(&self, method: &str, s: Script) {
        self.state
            .lock()
            .unwrap()
            .scripted
            .entry(method.to_owned())
            .or_default()
            .push_back(s);
    }

    pub fn set_core_id(&self, id: &str) {
        self.state.lock().unwrap().core_id = id.to_owned();
    }

    pub fn set_screen(&self, address: &str, text: &str) {
        self.queue_screens(address, vec![text.to_owned()]);
    }

    /// Screens one read each, the last one staying.
    pub fn queue_screens(&self, address: &str, screens: Vec<String>) {
        self.state
            .lock()
            .unwrap()
            .screens
            .insert(address.to_owned(), screens.into());
    }

    pub fn set_doc(&self, value: Value) {
        let mut st = self.state.lock().unwrap();
        st.doc = (value, st.doc.1 + 1);
    }

    pub fn doc(&self) -> (Value, u64) {
        self.state.lock().unwrap().doc.clone()
    }

    /// Bumps the document's version as if someone else wrote it, keeping its value.
    pub fn touch_doc(&self) {
        self.state.lock().unwrap().doc.1 += 1;
    }

    pub fn terminal(&self, address: &str) -> Option<Value> {
        let st = self.state.lock().unwrap();
        st.terminals
            .iter()
            .find(|t| t["address"] == address)
            .cloned()
    }

    /// Adds a live terminal (no event) and returns its address.
    pub fn add(&self, record: Value) -> String {
        let mut st = self.state.lock().unwrap();
        let t = complete(&mut st, record);
        let address = t["address"].as_str().unwrap().to_owned();
        st.terminals.push(t);
        address
    }

    /// Adds a terminal and announces it with `created`.
    pub fn create(&self, record: Value) -> String {
        let address = self.add(record);
        let t = self.terminal(&address).unwrap();
        self.emit("created", Some(&address), t);
        address
    }

    /// Changes fields of a terminal without any event.
    pub fn patch(&self, address: &str, fields: Value) {
        let mut st = self.state.lock().unwrap();
        if let Some(t) = st.terminals.iter_mut().find(|t| t["address"] == address) {
            for (k, v) in fields.as_object().unwrap() {
                t[k] = v.clone();
            }
        }
    }

    /// Applies a change to a terminal and sends it as an event.
    pub fn change(&self, address: &str, kind: &str, data: Value) {
        {
            let mut st = self.state.lock().unwrap();
            apply(&mut st, address, kind, &data);
        }
        self.emit(kind, Some(address), data);
    }

    /// Sends one `event` with the next `seq`.
    pub fn emit(&self, kind: &str, terminal: Option<&str>, data: Value) {
        let mut st = self.state.lock().unwrap();
        st.seq += 1;
        let mut params = json!({ "type": kind, "seq": st.seq, "data": data });
        if let Some(t) = terminal {
            params["terminal"] = json!(t);
        }
        broadcast(
            &mut st,
            json!({ "jsonrpc": "2.0", "method": "event", "params": params }),
        );
    }

    /// Advances `seq` without telling subscribers, so the next event shows a gap.
    pub fn skip_seq(&self) {
        self.state.lock().unwrap().seq += 1;
    }

    pub fn heartbeat(&self) {
        let mut st = self.state.lock().unwrap();
        let seq = st.seq;
        broadcast(
            &mut st,
            json!({ "jsonrpc": "2.0", "method": "heartbeat", "params": { "seq": seq } }),
        );
    }

    pub fn missed(&self) {
        let mut st = self.state.lock().unwrap();
        let (seq, terminals) = (st.seq, st.terminals.clone());
        broadcast(
            &mut st,
            json!({ "jsonrpc": "2.0", "method": "missed", "params": { "seq": seq, "terminals": terminals } }),
        );
    }

    /// Closes every subscription connection.
    pub fn drop_subscribers(&self) {
        self.state.lock().unwrap().subscribers.clear();
    }

    pub fn subscribers(&self) -> usize {
        let mut st = self.state.lock().unwrap();
        st.subscribers.retain(|s| !s.is_closed());
        st.subscribers.len()
    }
}

fn broadcast(st: &mut State, msg: Value) {
    let line = msg.to_string();
    st.subscribers.retain(|s| s.send(line.clone()).is_ok());
}

/// Fills in what the core always has, so tests give only what they care about.
fn complete(st: &mut State, mut t: Value) -> Value {
    let n = st.next;
    st.next += 1;
    let defaults = json!({
        "address": format!("t{n}"), "pid": 1000 + n, "created_ms": 1_000_000 + u64::from(n),
        "state": "live", "argv": ["/bin/sh"], "cwd": "/tmp", "name": null, "title": "",
        "labels": {}, "reported": false, "status": null, "status_ms": null, "turn": 0,
        "screen": "exact", "cols": 200, "rows": 120,
    });
    for (k, v) in defaults.as_object().unwrap() {
        if t.get(k).is_none() {
            t[k] = v.clone();
        }
    }
    if t.get("foreground").is_none() {
        let argv = t["argv"].clone();
        let name = argv[0]
            .as_str()
            .and_then(|a| a.rsplit('/').next())
            .unwrap_or("sh")
            .to_owned();
        t["foreground"] = json!({ "pid": t["pid"], "name": name, "argv": argv, "cwd": t["cwd"] });
    }
    t
}

fn apply(st: &mut State, address: &str, kind: &str, data: &Value) {
    if kind == "removed" {
        st.terminals.retain(|t| t["address"] != address);
        return;
    }
    let Some(t) = st.terminals.iter_mut().find(|t| t["address"] == address) else {
        return;
    };
    match kind {
        "status" => {
            for k in ["status", "reported", "status_ms"] {
                if let Some(v) = data.get(k) {
                    t[k] = v.clone();
                }
            }
        }
        "foreground" => t["foreground"] = data.clone(),
        "title" | "turn" | "labels" => t[kind] = data[kind].clone(),
        "finished" => {
            t["state"] = json!("finished");
            t["foreground"] = Value::Null;
            t["status"] = Value::Null;
        }
        _ => {}
    }
}

fn find<'a>(st: &'a mut State, target: &str) -> Option<&'a mut Value> {
    st.terminals
        .iter_mut()
        .find(|t| t["address"] == target || t["name"] == target)
}

fn error(kind: &str) -> Value {
    json!({ "code": -32000, "message": kind, "data": { "kind": kind } })
}

enum Answer {
    Reply(Value),
    Error(Value),
    Drop,
    Hang,
    Subscribe(Value, mpsc::UnboundedReceiver<String>),
}

fn handle(st: &mut State, req: &Value) -> Answer {
    let method = req["method"].as_str().unwrap_or_default();
    let p = &req["params"];
    if method == "hello" {
        st.hellos += 1;
        if p["generation"] != 1 {
            return Answer::Error(error("unsupported_generation"));
        }
        return Answer::Reply(json!({
            "generation": 1, "version": "0.1.20", "framed": false, "features": ["events", "status"],
            "core": { "id": st.core_id, "name": "fake" },
            "methods": ["terminal.list", "terminal.get", "terminal.read", "terminal.send",
                        "terminal.keys", "terminal.run", "terminal.kill", "terminal.rm",
                        "terminal.label", "doc.get", "doc.set", "events.subscribe"],
        }));
    }
    st.requests.push(req.clone());
    if let Some(s) = st.scripted.get_mut(method).and_then(VecDeque::pop_front) {
        return match s {
            Script::Result(v) => Answer::Reply(v),
            Script::Error(kind) => Answer::Error(error(kind)),
            Script::Drop => Answer::Drop,
            Script::Hang => Answer::Hang,
        };
    }
    let target = p["terminal"].as_str().unwrap_or_default().to_owned();
    match method {
        "terminal.list" => {
            let all = p["all"].as_bool().unwrap_or(false);
            let want = p["labels"].as_object().cloned().unwrap_or_default();
            let terminals: Vec<Value> = st
                .terminals
                .iter()
                .filter(|t| all || t["state"] == "live")
                .filter(|t| want.iter().all(|(k, v)| t["labels"][k] == *v))
                .cloned()
                .collect();
            Answer::Reply(json!({ "terminals": terminals, "hidden_finished": 0, "machines": [] }))
        }
        "terminal.get" => match find(st, &target) {
            Some(t) => Answer::Reply(t.clone()),
            None => Answer::Error(error("not_found")),
        },
        "terminal.read" => {
            let Some(address) = find(st, &target).map(|t| t["address"].clone()) else {
                return Answer::Error(error("not_found"));
            };
            let text = match st.screens.get_mut(address.as_str().unwrap()) {
                Some(q) if q.len() > 1 => q.pop_front().unwrap_or_default(),
                Some(q) => q.front().cloned().unwrap_or_default(),
                None => String::new(),
            };
            Answer::Reply(json!({ "text": text, "complete": true, "how": "screen" }))
        }
        "terminal.send" | "terminal.keys" => {
            let Some(t) = find(st, &target) else {
                return Answer::Error(error("not_found"));
            };
            if t["state"] != "live" {
                return Answer::Error(error("finished"));
            }
            if method == "terminal.send" && t["status"]["state"] == "blocked" {
                return Answer::Error(error("blocked"));
            }
            if method == "terminal.keys"
                && p["keys"]
                    .as_array()
                    .is_none_or(|ks| ks.iter().any(|k| !KEYS.contains(&k.as_str().unwrap_or(""))))
            {
                return Answer::Error(json!({ "code": -32602, "message": "unknown key",
                                             "data": { "kind": "invalid_params" } }));
            }
            Answer::Reply(json!({ "turn": t["turn"], "status": t["status"]["state"] }))
        }
        "terminal.run" => {
            if let Some(name) = p["name"].as_str()
                && st
                    .terminals
                    .iter()
                    .any(|t| t["name"] == name && t["state"] == "live")
            {
                return Answer::Error(error("name_taken"));
            }
            let record = json!({
                "argv": p["argv"], "cwd": p["cwd"], "name": p["name"],
                "labels": p.get("labels").cloned().unwrap_or(json!({})),
            });
            let t = complete(st, record);
            st.terminals.push(t.clone());
            st.seq += 1;
            let seq = st.seq;
            let address = t["address"].clone();
            broadcast(
                st,
                json!({ "jsonrpc": "2.0", "method": "event",
                        "params": { "type": "created", "seq": seq, "terminal": address, "data": t } }),
            );
            Answer::Reply(t)
        }
        "terminal.kill" => {
            let Some(t) = find(st, &target) else {
                return Answer::Error(error("not_found"));
            };
            t["state"] = json!("finished");
            t["foreground"] = Value::Null;
            Answer::Reply(t.clone())
        }
        "terminal.rm" => {
            let Some(t) = find(st, &target) else {
                return Answer::Error(error("not_found"));
            };
            if t["state"] == "live" {
                return Answer::Error(error("live"));
            }
            let address = t["address"].clone();
            st.terminals.retain(|t| t["address"] != address);
            Answer::Reply(json!({ "removed": address }))
        }
        "terminal.label" => {
            let Some(t) = find(st, &target) else {
                return Answer::Error(error("not_found"));
            };
            for (k, v) in p["labels"].as_object().cloned().unwrap_or_default() {
                if v == "" {
                    t["labels"].as_object_mut().unwrap().remove(&k);
                } else {
                    t["labels"][k] = v;
                }
            }
            Answer::Reply(t.clone())
        }
        "doc.get" => Answer::Reply(json!({ "value": st.doc.0, "version": st.doc.1 })),
        "doc.set" => {
            if p["version"].as_u64() != Some(st.doc.1) {
                return Answer::Error(error("conflict"));
            }
            st.doc = (p["value"].clone(), st.doc.1 + 1);
            Answer::Reply(json!({ "version": st.doc.1 }))
        }
        "events.subscribe" => {
            let (tx, rx) = mpsc::unbounded_channel();
            st.subscribers.push(tx);
            let terminals = st.terminals.clone();
            Answer::Subscribe(
                json!({ "seq": st.seq, "terminals": terminals, "channel": null }),
                rx,
            )
        }
        _ => Answer::Error(error("not_found")),
    }
}

async fn serve(stream: tokio::net::UnixStream, state: Arc<Mutex<State>>) {
    let (read, mut write) = stream.into_split();
    let mut lines = BufReader::new(read).lines();
    while let Ok(Some(line)) = lines.next_line().await {
        let Ok(req) = serde_json::from_str::<Value>(&line) else {
            return;
        };
        let answer = handle(&mut state.lock().unwrap(), &req);
        let id = req["id"].clone();
        let reply = |body: (&str, Value)| {
            let mut v = json!({ "jsonrpc": "2.0", "id": id });
            v[body.0] = body.1;
            format!("{v}\n")
        };
        match answer {
            Answer::Reply(v) => {
                if write
                    .write_all(reply(("result", v)).as_bytes())
                    .await
                    .is_err()
                {
                    return;
                }
            }
            Answer::Error(e) => {
                if write
                    .write_all(reply(("error", e)).as_bytes())
                    .await
                    .is_err()
                {
                    return;
                }
            }
            Answer::Drop => return,
            Answer::Hang => {
                std::future::pending::<()>().await;
            }
            Answer::Subscribe(v, mut rx) => {
                if write
                    .write_all(reply(("result", v)).as_bytes())
                    .await
                    .is_err()
                {
                    return;
                }
                loop {
                    tokio::select! {
                        msg = rx.recv() => match msg {
                            Some(l) => {
                                if write.write_all(format!("{l}\n").as_bytes()).await.is_err() {
                                    return;
                                }
                            }
                            None => return,
                        },
                        line = lines.next_line() => if !matches!(line, Ok(Some(_))) {
                            return;
                        },
                    }
                }
            }
        }
    }
}
