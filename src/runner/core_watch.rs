//! What runs on this machine's herdr-core: presence by `hello`, a mirror kept from one event
//! subscription, and the agents in it as the runner lists them. The runner only dials the core;
//! it never starts or stops one.

use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use tokio::time::Instant;

use super::backend::Facts;
use crate::agents::codex::screen::{Screen, classify};
use crate::hc::events::{Change, Item};
use crate::hc::types::{LAYOUT_DOC, SpaceView, Terminal, spaces};
use crate::hc::{self, Client};
use crate::herdr::types::AgentStatus;

/// The server name core agents go by: `hc/<agent>`.
pub const SERVER: &str = "hc";
/// How often the core is dialled to see that it is up and still the same core.
pub const PRESENCE: Duration = Duration::from_secs(5);
/// Space for agents with no space of their own or their parent's.
const NO_SPACE: &str = "core";
const SHELLS: &[&str] = &[
    "sh", "bash", "zsh", "fish", "dash", "ksh", "mksh", "tcsh", "csh", "nu", "elvish", "xonsh",
];

/// The mirror of one core.
pub struct Core {
    socket: PathBuf,
    inner: Mutex<Inner>,
}

#[derive(Default)]
struct Inner {
    /// Set while the core answers `hello`.
    core_id: Option<String>,
    synced: bool,
    seq: u64,
    terminals: BTreeMap<String, Terminal>,
    spaces: Vec<SpaceView>,
    /// The last `status.app` each agent reported, by identity: Claude clears its status during
    /// a turn.
    apps: HashMap<String, String>,
}

/// What an event did to the mirror.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Applied {
    Changed,
    /// The layout document changed; read it again.
    Layout,
    /// Events were lost or can't be applied: subscribe again for a fresh snapshot.
    Resync,
    Nothing,
}

impl Core {
    pub fn new(socket: impl Into<PathBuf>) -> Arc<Self> {
        Arc::new(Self {
            socket: socket.into(),
            inner: Mutex::default(),
        })
    }

    pub fn socket(&self) -> &Path {
        &self.socket
    }

    pub fn client(&self) -> Client {
        Client::new(&self.socket)
    }

    /// The id of the core that answered last; none while it's down.
    pub fn core_id(&self) -> Option<String> {
        self.inner.lock().unwrap().core_id.clone()
    }

    /// Records a `hello`; true when it is another core than before, whose mirror is dropped.
    pub fn up(&self, core_id: &str) -> bool {
        let mut st = self.inner.lock().unwrap();
        let changed = st.core_id.as_deref().is_some_and(|id| id != core_id);
        if changed {
            *st = Inner::default();
        }
        st.core_id = Some(core_id.to_owned());
        changed
    }

    pub fn down(&self) {
        let mut st = self.inner.lock().unwrap();
        st.core_id = None;
        st.synced = false;
    }

    /// A terminal as the mirror has it, agent or not.
    pub fn terminal(&self, address: &str) -> Option<Terminal> {
        self.inner.lock().unwrap().terminals.get(address).cloned()
    }

    pub fn is_synced(&self) -> bool {
        self.inner.lock().unwrap().synced
    }

    pub fn mark_lost(&self) {
        self.inner.lock().unwrap().synced = false;
    }

    pub fn set_layout(&self, doc: &serde_json::Value) {
        self.inner.lock().unwrap().spaces = spaces(doc);
    }

    pub fn install(&self, seq: u64, terminals: Vec<Terminal>) {
        let mut st = self.inner.lock().unwrap();
        st.seq = seq;
        st.terminals = terminals
            .into_iter()
            .map(|t| (t.address.clone(), t))
            .collect();
        st.synced = true;
        remember_apps(&mut st);
    }

    pub fn apply(&self, item: Item) -> Applied {
        let mut st = self.inner.lock().unwrap();
        let applied = match item {
            Item::Snapshot { seq, terminals } => {
                drop(st);
                self.install(seq, terminals);
                return Applied::Changed;
            }
            Item::Missed => Applied::Resync,
            Item::Heartbeat(seq) if seq != st.seq => Applied::Resync,
            Item::Heartbeat(_) => Applied::Nothing,
            Item::Delta { seq, .. } if seq <= st.seq => Applied::Nothing,
            Item::Delta { seq, .. } if seq != st.seq + 1 => Applied::Resync,
            Item::Delta {
                seq,
                terminal,
                change,
            } => {
                st.seq = seq;
                match (terminal, change) {
                    (_, Change::Doc { name }) if name == LAYOUT_DOC => Applied::Layout,
                    (_, Change::Doc { .. }) => Applied::Nothing,
                    (_, Change::Created(t)) => {
                        st.terminals.insert(t.address.clone(), *t);
                        Applied::Changed
                    }
                    (None, _) => Applied::Nothing,
                    (Some(a), Change::Finished | Change::Removed | Change::Lost) => {
                        st.terminals.remove(&a);
                        Applied::Changed
                    }
                    // A restart can put another process in the terminal: read it again.
                    (Some(_), Change::Other { kind, .. }) if kind == "restarted" => Applied::Resync,
                    (Some(a), change) => match st.terminals.get_mut(&a) {
                        None => Applied::Nothing,
                        Some(t) => {
                            change_terminal(t, change);
                            Applied::Changed
                        }
                    },
                }
            }
        };
        if applied == Applied::Changed {
            remember_apps(&mut st);
        }
        if applied == Applied::Resync {
            st.synced = false;
        }
        applied
    }

    /// The agents in the mirror. None while it isn't synced.
    pub fn facts(&self) -> Option<Vec<Facts>> {
        let st = self.inner.lock().unwrap();
        let core_id = st.core_id.as_deref()?;
        st.synced.then(|| {
            listing(
                core_id,
                st.terminals.values(),
                &st.spaces,
                &st.apps,
                &st.terminals,
            )
        })
    }

    /// One terminal just read from the core, as the listing would show it; none if it isn't an
    /// agent now.
    pub fn fresh(&self, t: &Terminal) -> Option<Facts> {
        let st = self.inner.lock().unwrap();
        let core_id = st.core_id.clone()?;
        listing(
            &core_id,
            std::iter::once(t),
            &st.spaces,
            &st.apps,
            &st.terminals,
        )
        .pop()
    }

    /// The agents, from the mirror when it's synced, else read from the core now.
    pub async fn agents(&self) -> hc::Result<Vec<Facts>> {
        let facts = match self.facts() {
            Some(f) => f,
            None => {
                let c = self.client();
                let core_id = c.hello().await?.core.id;
                let terminals = c.list().await?;
                let layout = c.doc_get(LAYOUT_DOC).await.map(|d| spaces(&d.value));
                let by: BTreeMap<String, Terminal> = terminals
                    .iter()
                    .map(|t| (t.address.clone(), t.clone()))
                    .collect();
                let apps = self.inner.lock().unwrap().apps.clone();
                listing(
                    &core_id,
                    terminals.iter(),
                    &layout.unwrap_or_default(),
                    &apps,
                    &by,
                )
            }
        };
        Ok(settle_codex(&self.client(), facts).await)
    }
}

fn remember_apps(st: &mut Inner) {
    let Some(core_id) = st.core_id.clone() else {
        return;
    };
    let seen: Vec<(String, String)> = st
        .terminals
        .values()
        .filter_map(|t| {
            let app = t.status.as_ref()?.app.clone()?;
            Some((t.identity(&core_id), app))
        })
        .collect();
    st.apps.extend(seen);
    let live: std::collections::HashSet<String> = st
        .terminals
        .values()
        .map(|t| t.identity(&core_id))
        .collect();
    st.apps.retain(|id, _| live.contains(id));
}

fn change_terminal(t: &mut Terminal, change: Change) {
    match change {
        Change::Foreground(f) => t.foreground = f,
        Change::Title(title) => t.title = title,
        Change::Status {
            status,
            reported,
            status_ms,
        } => {
            t.status = status;
            if let Some(r) = reported {
                t.reported = r;
            }
            if status_ms.is_some() {
                t.status_ms = status_ms;
            }
        }
        Change::Turn(turn) => t.turn = turn,
        Change::Labels(labels) => t.labels = labels,
        _ => {}
    }
}

/// The program `argv` runs, looking through `env` and its options.
pub fn program(argv: &[String]) -> String {
    let base = |s: &str| s.rsplit('/').next().unwrap_or(s).to_owned();
    let Some(first) = argv.first() else {
        return String::new();
    };
    // Switchboard's own launcher: `sh -c <prelude> sb-launch <agent> ...`.
    if argv.get(1).is_some_and(|a| a == "-c")
        && argv.get(3).is_some_and(|a| a == super::sessions::LAUNCHER)
    {
        return argv.get(4).map(|a| base(a)).unwrap_or_default();
    }
    if base(first) != "env" {
        return base(first);
    }
    let mut rest = argv[1..].iter();
    while let Some(a) = rest.next() {
        if a == "-u" || a == "--unset" || a == "-C" || a == "--chdir" || a == "-S" {
            rest.next();
        } else if a.starts_with('-') || a.contains('=') {
            continue;
        } else {
            return base(a);
        }
    }
    String::new()
}

fn is_shell(name: &str) -> bool {
    SHELLS.contains(&name.trim_start_matches('-'))
}

/// The agent kind the runner uses: `claude` and `codex`, else the program's own app name.
pub fn kind_of(t: &Terminal, last_app: Option<&str>) -> Option<String> {
    let app = t
        .status
        .as_ref()
        .and_then(|s| s.app.as_deref())
        .or(last_app);
    match app {
        Some("claude-code") => return Some("claude".into()),
        Some("codex") => return Some("codex".into()),
        Some(other) if t.reported && !other.is_empty() => return Some(other.to_owned()),
        _ => {}
    }
    match program(&t.argv).as_str() {
        k @ ("claude" | "codex") => Some(k.to_owned()),
        _ => None,
    }
}

pub fn status_of(t: &Terminal) -> AgentStatus {
    word(t.status.as_ref().map(|s| s.state.as_str()))
}

/// A program's status word as the runner's: `error` is done, none is unknown.
pub fn word(state: Option<&str>) -> AgentStatus {
    match state {
        Some("idle") => AgentStatus::Idle,
        Some("working") => AgentStatus::Working,
        Some("blocked") => AgentStatus::Blocked,
        Some("done" | "error") => AgentStatus::Done,
        _ => AgentStatus::Unknown,
    }
}

/// `turn` and `status_ms` in one number, so a change to either is a new state.
pub fn state_seq(t: &Terminal) -> u64 {
    (t.turn << 42) | (t.status_ms.unwrap_or_default() & ((1 << 42) - 1))
}

/// A space's name, else what the Herdr app calls an unnamed one: where its first terminal is. The
/// space's saved folder only when none of its terminals is known; it goes stale.
fn space_name(spaces: &[SpaceView], id: &str, all: &BTreeMap<String, Terminal>) -> Option<String> {
    let s = spaces.iter().find(|s| s.id == id)?;
    let base = |p: &str| {
        Path::new(p)
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
    };
    s.name
        .clone()
        .or_else(|| {
            let first = s
                .tabs
                .iter()
                .flat_map(|tab| &tab.terminals)
                .find_map(|address| all.get(address))?;
            let cwd = first
                .foreground
                .as_ref()
                .map(|f| f.cwd.as_str())
                .filter(|c| !c.is_empty())
                .unwrap_or(&first.cwd);
            base(cwd)
        })
        .or_else(|| s.folder.as_deref().and_then(base))
}

/// The space a terminal shows in: its own, else its parent's, else none.
fn space_of(
    t: &Terminal,
    spaces: &[SpaceView],
    all: &BTreeMap<String, Terminal>,
) -> Option<String> {
    let mut cur = Some(t);
    for _ in 0..8 {
        let here = cur?;
        if let Some(id) = here.labels.get("herdr.space") {
            return space_name(spaces, id, all);
        }
        cur = here.parent.as_deref().and_then(|p| all.get(p));
    }
    None
}

fn tab_name(t: &Terminal, spaces: &[SpaceView]) -> Option<String> {
    let id = t.labels.get("herdr.tab")?;
    spaces
        .iter()
        .flat_map(|s| &s.tabs)
        .find(|tab| &tab.id == id)
        .and_then(|tab| tab.name.clone())
}

/// The core's agents: live local terminals running Claude, Codex or another reporting program,
/// with no shell back in front. A name shared by several is listed as `<name>-<address>`.
pub fn listing<'a>(
    core_id: &str,
    terminals: impl Iterator<Item = &'a Terminal>,
    spaces: &[SpaceView],
    apps: &HashMap<String, String>,
    all: &BTreeMap<String, Terminal>,
) -> Vec<Facts> {
    let mut facts: Vec<Facts> = terminals
        .filter(|t| t.is_live() && t.is_local() && t.pid.is_some())
        .filter(|t| !t.foreground.as_ref().is_some_and(|f| is_shell(&f.name)))
        .filter_map(|t| {
            let identity = t.identity(core_id);
            let kind = kind_of(t, apps.get(&identity).map(String::as_str))?;
            let fg = t.foreground.as_ref();
            let status = status_of(t);
            let display = t
                .name
                .clone()
                .filter(|n| !n.is_empty())
                .or_else(|| tab_name(t, spaces))
                .unwrap_or_else(|| t.address.clone());
            Some(Facts {
                server: Some(SERVER.into()),
                target: t.address.clone(),
                name: Some(display),
                kind: Some(kind),
                status,
                cwd: fg
                    .map(|f| f.cwd.clone())
                    .filter(|c| !c.is_empty())
                    .or_else(|| Some(t.cwd.clone()).filter(|c| !c.is_empty())),
                title: Some(t.title.clone()).filter(|s| !s.is_empty()),
                space: Some(space_of(t, spaces, all).unwrap_or_else(|| NO_SPACE.into())),
                tab: tab_name(t, spaces),
                identity: Some(identity),
                reported_session: None,
                seq: state_seq(t),
                ready: status == AgentStatus::Idle,
                pids: fg
                    .and_then(|f| u32::try_from(f.pid).ok())
                    .into_iter()
                    .chain(t.pid)
                    .collect(),
                blocked_kind: t
                    .status
                    .as_ref()
                    .filter(|s| s.state == "blocked")
                    .and_then(|s| s.kind.clone()),
                turn: Some(t.turn),
            })
        })
        .collect();
    let mut counts: HashMap<String, usize> = HashMap::new();
    for f in &facts {
        *counts.entry(f.local_name()).or_default() += 1;
    }
    for f in &mut facts {
        if let Some(n) = f.name.as_mut()
            && counts.get(n.as_str()) > Some(&1)
        {
            *n = format!("{n}-{}", f.target);
        }
    }
    facts
}

/// A new Codex says `working` at its prompt until its first turn; its screen says whether it's
/// really at the composer.
async fn settle_codex(client: &Client, mut facts: Vec<Facts>) -> Vec<Facts> {
    for f in &mut facts {
        if f.kind.as_deref() == Some("codex")
            && f.turn == Some(0)
            && f.status == AgentStatus::Working
            && let Ok(r) = client.read(&f.target, 0).await
            && classify(&r.text) == Screen::Ready
        {
            f.status = AgentStatus::Idle;
            f.ready = true;
        }
    }
    facts
}

/// Follows the core: presence every `PRESENCE`, then one subscription at a time, calling
/// `changed` after each change the mirror takes and `replaced` when another core answers.
pub async fn follow(core: Arc<Core>, changed: impl FnMut() + Send, replaced: impl FnMut() + Send) {
    follow_every(core, PRESENCE, hc::events::SILENCE, changed, replaced).await
}

/// `follow`, with presence every `presence` and a stream silent for `silence` taken as dead.
pub(crate) async fn follow_every(
    core: Arc<Core>,
    presence: Duration,
    silence: Duration,
    mut changed: impl FnMut() + Send,
    mut replaced: impl FnMut() + Send,
) {
    loop {
        let client = core.client();
        match client.hello().await {
            Ok(h) => {
                if core.up(&h.core.id) {
                    tracing::info!(core = %h.core.id, "a different herdr-core answers");
                    replaced();
                }
            }
            Err(e) => {
                if core.core_id().is_some() {
                    tracing::info!(error = %e, "herdr-core is down");
                    core.down();
                    changed();
                }
                tokio::time::sleep(presence).await;
                continue;
            }
        }
        let mut stream = match client.subscribe().await {
            Ok(s) if Some(s.core_id().to_owned()) == core.core_id() => s.with_silence(silence),
            Ok(_) => continue,
            Err(e) => {
                tracing::warn!(error = %e, "herdr-core subscribe failed");
                tokio::time::sleep(presence).await;
                continue;
            }
        };
        if let Ok(doc) = client.doc_get(LAYOUT_DOC).await {
            core.set_layout(&doc.value);
        }
        let mut tick = tokio::time::interval_at(Instant::now() + presence, presence);
        loop {
            tokio::select! {
                item = stream.next() => match item {
                    None => break,
                    Some(Err(e)) => {
                        tracing::warn!(error = %e, "bad herdr-core event");
                        break;
                    }
                    Some(Ok(item)) => match core.apply(item) {
                        Applied::Changed => changed(),
                        Applied::Layout => {
                            if let Ok(doc) = client.doc_get(LAYOUT_DOC).await {
                                core.set_layout(&doc.value);
                            }
                            changed();
                        }
                        Applied::Resync => break,
                        Applied::Nothing => {}
                    },
                },
                _ = tick.tick() => match client.hello().await {
                    Ok(h) if Some(&h.core.id) == core.core_id().as_ref() => {}
                    _ => break,
                },
            }
        }
        core.mark_lost();
    }
}

#[cfg(test)]
#[path = "core_watch_tests.rs"]
mod tests;
