//! Following an agent's conversation log without polling: one folder watcher for the whole
//! runner wakes a waiting transcript read when the OS reports the log grew, or that the agent
//! may have moved to another log. Folders are watched, since a file replaced by rename would
//! silently end a watch on it; on macOS the log itself is watched too, because FSEvents reports
//! a write to a file still open, as Codex keeps its log, only on close. Events only invalidate:
//! a waiter re-checks the file.

use std::collections::HashMap;
use std::future::Future;
use std::io::Read;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use notify::{EventKind, RecursiveMode, Watcher};
use tokio::sync::broadcast;
use tokio::time::Instant;

use super::transcript;

/// The longest one transcript read waits.
pub const MAX_WAIT: Duration = Duration::from_secs(20);
/// Transcript reads one runner holds waiting at once; more are answered at once.
pub const MAX_WAITS: usize = 8;
/// How often a wait checks the log when the OS can't tell it.
const FALLBACK_EVERY: Duration = Duration::from_secs(1);
/// How long a folder stays watched after its last wait, longer than a follower takes to wait again.
const LINGER: Duration = Duration::from_secs(30);

#[derive(Debug, Clone, PartialEq)]
pub enum Change {
    /// Something in a watched folder changed.
    Path(PathBuf),
    /// Changes may have been missed, as when the watch was set up again.
    All,
}

enum State {
    NotYet,
    Ready(notify::RecommendedWatcher),
    Failed,
}

struct Folder {
    waits: usize,
    /// When its last wait ended.
    idle_since: Option<std::time::Instant>,
}

/// The runner's one watcher, made on first use. Folders are counted by the waits watching them,
/// and released a while after the last ends, so a follower's next wait finds them still watched.
pub struct LogWatch {
    /// Changing what is watched restarts the stream on macOS, which takes a moment, so this is
    /// only locked off the async threads.
    watcher: Mutex<State>,
    folders: Mutex<HashMap<PathBuf, Folder>>,
    linger: Duration,
    /// No OS notifications at all, files included.
    off: bool,
    changes: broadcast::Sender<Change>,
}

impl LogWatch {
    pub fn new() -> Arc<Self> {
        Self::with(State::NotYet, LINGER)
    }

    /// As `new`, releasing folders `linger` after their last wait.
    pub fn lingering(linger: Duration) -> Arc<Self> {
        Self::with(State::NotYet, linger)
    }

    /// A watcher that can't be set up, as on a system out of inotify watches.
    pub fn unavailable() -> Arc<Self> {
        Self::with(State::Failed, LINGER)
    }

    fn with(state: State, linger: Duration) -> Arc<Self> {
        Arc::new(Self {
            off: matches!(state, State::Failed),
            watcher: Mutex::new(state),
            folders: Mutex::default(),
            linger,
            changes: broadcast::channel(1024).0,
        })
    }

    /// Watches one file where folder watches miss writes to it; none where they don't.
    fn watch_file(&self, path: &Path) -> Option<FileWatch> {
        (!self.off).then(|| FileWatch::new(path)).flatten()
    }

    pub fn subscribe(&self) -> broadcast::Receiver<Change> {
        self.changes.subscribe()
    }

    /// Folders being watched, released or not.
    pub fn folders(&self) -> usize {
        self.folders.lock().unwrap().len()
    }

    /// Watches `folders` until the result drops; none when the first can't be watched. Others
    /// that can't are left out. Blocks while the watch is set up.
    pub fn watch(self: &Arc<Self>, folders: &[PathBuf]) -> Option<Watching> {
        let mut state = self.watcher.lock().unwrap();
        if matches!(*state, State::NotYet) {
            let tx = self.changes.clone();
            *state = match notify::recommended_watcher(move |ev: notify::Result<notify::Event>| {
                match ev {
                    Ok(ev) if ev.need_rescan() => drop(tx.send(Change::All)),
                    Ok(ev) if matches!(ev.kind, EventKind::Access(_)) => {}
                    Ok(ev) => ev
                        .paths
                        .into_iter()
                        .for_each(|p| drop(tx.send(Change::Path(p)))),
                    Err(_) => drop(tx.send(Change::All)),
                }
            }) {
                Ok(w) => State::Ready(w),
                Err(e) => {
                    tracing::warn!(error = %e, "can't watch conversation logs; checking each second instead");
                    State::Failed
                }
            };
        }
        let State::Ready(watcher) = &mut *state else {
            return None;
        };
        let canonical: Vec<Option<PathBuf>> = folders
            .iter()
            .map(|f| std::fs::canonicalize(f).ok())
            .collect();
        let mut fresh: Vec<&PathBuf> = Vec::new();
        {
            let known = self.folders.lock().unwrap();
            for f in canonical.iter().flatten() {
                if !known.contains_key(f) && !fresh.contains(&f) {
                    fresh.push(f);
                }
            }
        }
        let mut failed = Vec::new();
        if !fresh.is_empty() {
            // Opening the set stops the stream, so it is always committed to start it again.
            let mut paths = watcher.paths_mut();
            for f in fresh {
                if let Err(e) = paths.add(f, RecursiveMode::NonRecursive) {
                    tracing::debug!(folder = %f.display(), error = %e, "can't watch a folder");
                    failed.push(f.clone());
                }
            }
            if let Err(e) = paths.commit() {
                tracing::warn!(error = %e, "can't start watching conversation logs");
            }
            // Other waits' events may have been dropped while the stream restarted.
            let _ = self.changes.send(Change::All);
        }
        let held: Vec<PathBuf> = canonical
            .iter()
            .flatten()
            .filter(|f| !failed.contains(f))
            .cloned()
            .collect();
        {
            let mut known = self.folders.lock().unwrap();
            for f in &held {
                let e = known.entry(f.clone()).or_insert(Folder {
                    waits: 0,
                    idle_since: None,
                });
                e.waits += 1;
                e.idle_since = None;
            }
        }
        drop(state);
        let first = canonical.first().cloned().flatten();
        let watching = Watching {
            watch: self.clone(),
            folders: held,
        };
        // Without the log's own folder there is nothing to wait on.
        first.filter(|f| !failed.contains(f)).map(|_| watching)
    }

    fn release(self: &Arc<Self>, folders: &[PathBuf]) {
        let mut idle = false;
        {
            let mut known = self.folders.lock().unwrap();
            for f in folders {
                if let Some(e) = known.get_mut(f) {
                    e.waits -= 1;
                    if e.waits == 0 {
                        e.idle_since = Some(std::time::Instant::now());
                        idle = true;
                    }
                }
            }
        }
        if let (true, Ok(rt)) = (idle, tokio::runtime::Handle::try_current()) {
            let (me, linger) = (self.clone(), self.linger);
            rt.spawn(async move {
                tokio::time::sleep(linger).await;
                let _ = tokio::task::spawn_blocking(move || me.sweep()).await;
            });
        }
    }

    /// Stops watching folders no wait has used for `linger`.
    fn sweep(&self) {
        let mut state = self.watcher.lock().unwrap();
        let gone: Vec<PathBuf> = {
            let mut known = self.folders.lock().unwrap();
            let gone: Vec<PathBuf> = known
                .iter()
                .filter(|(_, e)| {
                    e.waits == 0 && e.idle_since.is_some_and(|t| t.elapsed() >= self.linger)
                })
                .map(|(f, _)| f.clone())
                .collect();
            for f in &gone {
                known.remove(f);
            }
            gone
        };
        if gone.is_empty() {
            return;
        }
        if let State::Ready(watcher) = &mut *state {
            let mut paths = watcher.paths_mut();
            for f in &gone {
                let _ = paths.remove(f);
            }
            let _ = paths.commit();
        }
        let _ = self.changes.send(Change::All);
    }
}

/// Folders watched for one wait.
pub struct Watching {
    watch: Arc<LogWatch>,
    folders: Vec<PathBuf>,
}

impl Drop for Watching {
    fn drop(&mut self) {
        self.watch.release(&self.folders);
    }
}

/// An agent's log, and where a move to another log would show.
#[derive(Debug, Clone)]
pub struct Found {
    pub log: PathBuf,
    pub switch: Switch,
}

#[derive(Debug, Clone)]
pub enum Switch {
    /// Claude's records at `<claude dir>/sessions/<pid>.json`: one created or replaced, or one of
    /// the pane's naming another session.
    Claude {
        records: PathBuf,
        pids: Vec<u32>,
        session: String,
    },
    /// Codex's folder for today's logs: a new log there.
    Codex { day: PathBuf },
}

impl Switch {
    fn folder(&self) -> &Path {
        match self {
            Switch::Claude { records, .. } => records,
            Switch::Codex { day } => day,
        }
    }

    fn counts(&self, name: &str) -> bool {
        match self {
            Switch::Claude { .. } => name.ends_with(".json"),
            Switch::Codex { .. } => name.starts_with("rollout-") && name.ends_with(".jsonl"),
        }
    }
}

/// Codex's folder for logs started today, `<sessions>/YYYY/MM/DD` in local time.
pub fn codex_day(sessions: &Path) -> PathBuf {
    // SAFETY: `localtime_r` only writes the `tm` it is given.
    let tm = unsafe {
        let now = libc::time(std::ptr::null_mut());
        let mut tm: libc::tm = std::mem::zeroed();
        libc::localtime_r(&now, &mut tm);
        tm
    };
    sessions
        .join(format!("{:04}", tm.tm_year + 1900))
        .join(format!("{:02}", tm.tm_mon + 1))
        .join(format!("{:02}", tm.tm_mday))
}

/// Whether the log has a whole line past `after`, or no longer continues from it.
fn grown(log: &Path, after: u64) -> bool {
    let Ok(mut f) = std::fs::File::open(log) else {
        return true;
    };
    let Ok(len) = f.metadata().map(|m| m.len()) else {
        return true;
    };
    if len <= after {
        return len < after;
    }
    use std::io::Seek;
    if f.seek(std::io::SeekFrom::Start(after)).is_err() {
        return true;
    }
    let mut buf = [0u8; 64 * 1024];
    let mut f = f.take(len - after);
    loop {
        match f.read(&mut buf) {
            Ok(0) | Err(_) => return false,
            Ok(n) if buf[..n].contains(&b'\n') => return true,
            Ok(_) => {}
        }
    }
}

/// Files in the switch folder that count, by inode, so a created or replaced one stands out.
fn inodes(switch: &Switch) -> HashMap<std::ffi::OsString, u64> {
    let Ok(entries) = std::fs::read_dir(switch.folder()) else {
        return HashMap::new();
    };
    entries
        .flatten()
        .filter(|e| switch.counts(&e.file_name().to_string_lossy()))
        .filter_map(|e| Some((e.file_name(), e.metadata().ok()?.ino())))
        .collect()
}

/// Whether a change to `path` in the switch folder may mean the agent moved to another log.
fn switched(switch: &Switch, known: &mut HashMap<std::ffi::OsString, u64>, path: &Path) -> bool {
    let Some(name) = path.file_name() else {
        return false;
    };
    if !switch.counts(&name.to_string_lossy()) {
        return false;
    }
    let Ok(meta) = std::fs::metadata(path) else {
        return false;
    };
    if known.insert(name.to_owned(), meta.ino()) != Some(meta.ino()) {
        return true;
    }
    // Rewritten in place: only a record of the pane's own Claude naming another session counts.
    let Switch::Claude {
        records,
        pids,
        session,
    } = switch
    else {
        return false;
    };
    let pid = name
        .to_string_lossy()
        .strip_suffix(".json")
        .and_then(|p| p.parse::<u32>().ok());
    let Some(pid) = pid.filter(|p| pids.contains(p)) else {
        return false;
    };
    let dir = records.parent().unwrap_or(records);
    transcript::live_claude_session(dir, &[pid]).is_some_and(|s| s != *session)
}

/// Whether the switch folder changed since `known` was taken, which the watch set up after it
/// may not have seen.
fn moved(switch: &Switch, known: &mut HashMap<std::ffi::OsString, u64>) -> bool {
    let now = inodes(switch);
    let changed = now.iter().any(|(n, i)| known.get(n) != Some(i));
    *known = now;
    changed
        || match switch {
            Switch::Claude {
                records,
                pids,
                session,
            } => transcript::live_claude_session(records.parent().unwrap_or(records), pids)
                .is_some_and(|s| s != *session),
            Switch::Codex { .. } => false,
        }
}

/// Waits until the log has a whole line past `after`, the agent may have moved to another log,
/// or `deadline`, and returns the log to read. `status` carries agents whose Herdr status
/// changed; `lookup` finds the agent's log again, and runs only when something signals a move.
pub async fn wait<L, F>(
    watch: &Arc<LogWatch>,
    mut status: Option<broadcast::Receiver<String>>,
    agent: &str,
    after: u64,
    deadline: Instant,
    mut found: Found,
    mut lookup: L,
) -> PathBuf
where
    L: FnMut() -> F,
    F: Future<Output = Option<Found>>,
{
    // Subscribed and watching before the checks, so nothing written in between is lost.
    let mut changes = watch.subscribe();
    let mut file = watch.watch_file(&found.log);
    let mut known = inodes(&found.switch);
    let folders = [
        found.log.parent().unwrap_or(Path::new("/")).to_owned(),
        found.switch.folder().to_owned(),
    ];
    let watching = {
        let (watch, folders) = (watch.clone(), folders.clone());
        tokio::task::spawn_blocking(move || watch.watch(&folders))
            .await
            .ok()
            .flatten()
    };
    let canonical = |p: &Path| {
        let dir = p.parent().and_then(|d| std::fs::canonicalize(d).ok());
        dir.zip(p.file_name()).map(|(d, n)| d.join(n))
    };
    let mut log = canonical(&found.log);
    let mut switch_dir = std::fs::canonicalize(found.switch.folder()).ok();
    if grown(&found.log, after) {
        return found.log;
    }
    let mut fallback = watching
        .is_none()
        .then(|| tokio::time::interval_at(Instant::now() + FALLBACK_EVERY, FALLBACK_EVERY));
    let mut look = moved(&found.switch, &mut known);
    loop {
        if std::mem::take(&mut look) {
            match tokio::time::timeout_at(deadline, lookup()).await {
                Ok(Some(now)) if now.log != found.log => return now.log,
                Ok(Some(now)) => {
                    found = now;
                    file = watch.watch_file(&found.log);
                    log = canonical(&found.log);
                    switch_dir = std::fs::canonicalize(found.switch.folder()).ok();
                    known = inodes(&found.switch);
                    if grown(&found.log, after) {
                        return found.log;
                    }
                }
                Ok(None) | Err(_) => return found.log,
            }
        }
        tokio::select! {
            _ = tokio::time::sleep_until(deadline) => return found.log,
            _ = async { file.as_ref().unwrap().changed().await }, if file.is_some() => {
                if grown(&found.log, after) {
                    return found.log;
                }
            }
            _ = async { fallback.as_mut().unwrap().tick().await }, if fallback.is_some() => {
                if grown(&found.log, after) {
                    return found.log;
                }
            }
            change = changes.recv(), if fallback.is_none() => match change {
                Ok(Change::Path(p)) => {
                    if log.as_deref() == Some(p.as_path()) {
                        if grown(&found.log, after) {
                            return found.log;
                        }
                    } else if p.parent().is_some() && p.parent() == switch_dir.as_deref() {
                        look = switched(&found.switch, &mut known, &p);
                    }
                }
                Ok(Change::All) | Err(broadcast::error::RecvError::Lagged(_)) => {
                    if grown(&found.log, after) {
                        return found.log;
                    }
                }
                Err(broadcast::error::RecvError::Closed) => {
                    fallback = Some(tokio::time::interval(FALLBACK_EVERY));
                }
            },
            changed = async { status.as_mut().unwrap().recv().await }, if status.is_some() => match changed {
                Ok(name) => look = name == agent,
                Err(broadcast::error::RecvError::Lagged(_)) => look = true,
                Err(broadcast::error::RecvError::Closed) => status = None,
            },
        }
    }
}

/// A kqueue watch on one file, for writes FSEvents holds back until the file is closed.
#[cfg(target_os = "macos")]
struct FileWatch {
    _file: std::fs::File,
    kq: tokio::io::unix::AsyncFd<std::os::fd::OwnedFd>,
}

#[cfg(target_os = "macos")]
impl FileWatch {
    fn new(path: &Path) -> Option<Self> {
        use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
        use std::os::unix::fs::OpenOptionsExt;
        // Opened for events only, so the watch never holds the volume busy.
        let file = std::fs::OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_EVTONLY)
            .open(path)
            .ok()?;
        // SAFETY: `kqueue` takes no arguments; a non-negative result is a descriptor we own.
        let kq = unsafe { libc::kqueue() };
        if kq < 0 {
            return None;
        }
        // SAFETY: `kq` was just returned by `kqueue` and nothing else owns it.
        let kq = unsafe { OwnedFd::from_raw_fd(kq) };
        let ev = libc::kevent {
            ident: file.as_raw_fd() as usize,
            filter: libc::EVFILT_VNODE,
            flags: libc::EV_ADD | libc::EV_CLEAR,
            fflags: libc::NOTE_WRITE | libc::NOTE_EXTEND | libc::NOTE_DELETE | libc::NOTE_RENAME,
            data: 0,
            udata: std::ptr::null_mut(),
        };
        // SAFETY: one valid change, no event buffer, and both descriptors are open.
        let added = unsafe {
            libc::kevent(
                kq.as_raw_fd(),
                &ev,
                1,
                std::ptr::null_mut(),
                0,
                std::ptr::null(),
            )
        };
        if added < 0 {
            return None;
        }
        // SAFETY: the `AsyncFd` owns `kq`, so it stays open and the same for the `AsyncFd`'s life.
        let kq = unsafe {
            tokio::io::unix::AsyncFd::register_with_interest(kq, tokio::io::Interest::READABLE)
        }
        .ok()?;
        Some(Self { _file: file, kq })
    }

    /// The file was written to, renamed or removed since the last call.
    async fn changed(&self) {
        use std::os::fd::AsRawFd;
        let Ok(mut ready) = self.kq.readable().await else {
            return std::future::pending().await;
        };
        // SAFETY: an all-zero `kevent` is a valid value to overwrite.
        let mut got: [libc::kevent; 4] = unsafe { std::mem::zeroed() };
        let none = libc::timespec {
            tv_sec: 0,
            tv_nsec: 0,
        };
        // SAFETY: the buffer holds `got.len()` events and the timeout makes this return at once.
        unsafe {
            libc::kevent(
                self.kq.get_ref().as_raw_fd(),
                std::ptr::null(),
                0,
                got.as_mut_ptr(),
                got.len() as libc::c_int,
                &none,
            )
        };
        ready.clear_ready();
    }
}

/// Elsewhere inotify reports writes to open files, so folder watches suffice.
#[cfg(not(target_os = "macos"))]
struct FileWatch;

#[cfg(not(target_os = "macos"))]
impl FileWatch {
    fn new(_: &Path) -> Option<Self> {
        None
    }

    async fn changed(&self) {
        std::future::pending().await
    }
}

/// Tests that time OS notifications take turns: macOS sets up watch streams one at a time.
#[cfg(test)]
pub(crate) static TIMED: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

#[cfg(test)]
#[path = "logwatch_tests.rs"]
mod tests;
