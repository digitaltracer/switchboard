//! Which log each pane's agent writes, remembered between requests and checked cheaply on each
//! use against the snapshot the request already took. Anything that doesn't match exactly is a
//! miss and a full search, so a stale entry never returns another process's or session's log.

use std::collections::{HashMap, HashSet};
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant, SystemTime};

use super::logwatch::{self, Found, Switch};
use super::transcript;

/// How long an entry is trusted before a full search checks it again.
pub const REVALIDATE: Duration = Duration::from_secs(60);
/// The most logs kept per Codex folder for the hit check.
const KNOWN_CODEX_LOGS: usize = 32;

/// A pane in one Herdr server: its socket and pane id.
pub type Key = (PathBuf, String);

/// What the snapshot says about the agent in a pane; a hit needs all of it unchanged.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Pane {
    pub server: PathBuf,
    pub pane_id: String,
    pub terminal_id: Option<String>,
    pub kind: String,
    /// The session Herdr reports for the agent.
    pub reported: Option<String>,
    /// For Codex, the folder its log is matched on.
    pub cwd: Option<String>,
}

impl Pane {
    pub fn key(&self) -> Key {
        (self.server.clone(), self.pane_id.clone())
    }
}

#[derive(Debug, Clone)]
enum Resolved {
    /// The session `pid`'s record named, of the pane's foreground `pids`.
    Claude {
        pid: u32,
        pids: Vec<u32>,
        session: String,
    },
    /// The logs started in the pane's folder, and today's folder as it was.
    Codex {
        known: Vec<PathBuf>,
        day: PathBuf,
        day_modified: Option<SystemTime>,
    },
}

#[derive(Debug, Clone)]
struct Entry {
    pane: Pane,
    resolved: Resolved,
    log: PathBuf,
    ino: u64,
    searched: Instant,
}

pub struct LogCache {
    claude_projects: PathBuf,
    codex_sessions: PathBuf,
    revalidate: Duration,
    entries: Mutex<HashMap<Key, Entry>>,
    headers: transcript::CodexHeaders,
    searches: AtomicU64,
}

impl LogCache {
    pub fn new(claude_projects: PathBuf, codex_sessions: PathBuf) -> Self {
        Self::revalidating(claude_projects, codex_sessions, REVALIDATE)
    }

    pub fn revalidating(
        claude_projects: PathBuf,
        codex_sessions: PathBuf,
        revalidate: Duration,
    ) -> Self {
        Self {
            claude_projects,
            codex_sessions,
            revalidate,
            entries: Mutex::default(),
            headers: Default::default(),
            searches: AtomicU64::new(0),
        }
    }

    /// Full searches so far.
    pub fn searches(&self) -> u64 {
        self.searches.load(Ordering::Relaxed)
    }

    /// Codex first lines read so far.
    pub fn header_reads(&self) -> u64 {
        self.headers.reads()
    }

    /// Forgets panes of `server` that its latest snapshot no longer has.
    pub fn retain(&self, server: &Path, panes: &HashSet<String>) {
        self.entries
            .lock()
            .unwrap()
            .retain(|(s, p), _| s != server || panes.contains(p));
    }

    fn claude_dir(&self) -> Option<&Path> {
        self.claude_projects.parent()
    }

    /// The cached log, if everything it was found from still holds. Reads a few files.
    pub fn hit(&self, pane: &Pane) -> Option<Found> {
        let e = self.entries.lock().unwrap().get(&pane.key()).cloned()?;
        if e.pane != *pane || e.searched.elapsed() >= self.revalidate {
            return None;
        }
        if std::fs::metadata(&e.log).ok()?.ino() != e.ino {
            return None;
        }
        match e.resolved {
            Resolved::Claude { pid, pids, session } => {
                let dir = self.claude_dir()?;
                // `/clear` and `/resume` rewrite the record in place; a parked job is followed.
                let now = transcript::live_claude_record(dir, &[pid])?;
                (now == (pid, session.clone())).then(|| Found {
                    log: e.log,
                    switch: Switch::Claude {
                        records: dir.join("sessions"),
                        pids,
                        session,
                    },
                })
            }
            Resolved::Codex {
                known,
                day,
                day_modified,
            } => {
                let today = logwatch::codex_day(&self.codex_sessions, 0);
                if today != day || modified(&day) != day_modified {
                    return None;
                }
                // A resumed older log for this folder is written again and becomes the newest.
                let mine = modified(&e.log)?;
                let newer = known
                    .iter()
                    .filter(|p| **p != e.log)
                    .any(|p| modified(p).is_some_and(|m| m > mine));
                (!newer).then(|| Found {
                    log: e.log,
                    switch: codex_switch(&self.codex_sessions, today),
                })
            }
        }
    }

    /// A full search, remembered when it can be checked cheaply later. "No log yet" isn't.
    pub fn search(&self, pane: Pane, pids: Vec<u32>) -> Option<Found> {
        self.searches.fetch_add(1, Ordering::Relaxed);
        tracing::debug!(pane = %pane.pane_id, kind = %pane.kind, "searching for a conversation log");
        let key = pane.key();
        let found = match pane.kind.as_str() {
            "claude" => self.search_claude(&pane, pids),
            "codex" => self.search_codex(&pane),
            _ => None,
        };
        let mut entries = self.entries.lock().unwrap();
        let Some((found, resolved)) = found else {
            entries.remove(&key);
            return None;
        };
        let ino = std::fs::metadata(&found.log).ok().map(|m| m.ino());
        match (resolved, ino, &pane.terminal_id) {
            (Some(resolved), Some(ino), Some(_)) => {
                entries.insert(
                    key,
                    Entry {
                        pane,
                        resolved,
                        log: found.log.clone(),
                        ino,
                        searched: Instant::now(),
                    },
                );
            }
            _ => drop(entries.remove(&key)),
        }
        Some(found)
    }

    fn search_claude(&self, pane: &Pane, pids: Vec<u32>) -> Option<(Found, Option<Resolved>)> {
        let dir = self.claude_dir()?;
        let live = transcript::live_claude_record(dir, &pids);
        let session = live
            .as_ref()
            .map(|(_, s)| s.clone())
            .or_else(|| pane.reported.clone())?;
        let log = transcript::find_log(&self.claude_projects, &session)?;
        // A session taken from Herdr's report alone has no record to check a hit against.
        let resolved = live.map(|(pid, session)| Resolved::Claude {
            pid,
            pids: pids.clone(),
            session,
        });
        let found = Found {
            log,
            switch: Switch::Claude {
                records: dir.join("sessions"),
                pids,
                session,
            },
        };
        Some((found, resolved))
    }

    fn search_codex(&self, pane: &Pane) -> Option<(Found, Option<Resolved>)> {
        let cwd = pane.cwd.as_deref()?;
        let day = logwatch::codex_day(&self.codex_sessions, 0);
        // Taken before the scan, so a log made during it fails the next hit.
        let day_modified = modified(&day);
        let mut known = transcript::codex_logs(&self.codex_sessions, Path::new(cwd), &self.headers);
        known.truncate(KNOWN_CODEX_LOGS);
        let log = known.first()?.clone();
        let found = Found {
            log,
            switch: codex_switch(&self.codex_sessions, day.clone()),
        };
        let resolved = Resolved::Codex {
            known,
            day,
            day_modified,
        };
        Some((found, Some(resolved)))
    }
}

fn modified(path: &Path) -> Option<SystemTime> {
    std::fs::metadata(path).ok()?.modified().ok()
}

fn codex_switch(sessions: &Path, day: PathBuf) -> Switch {
    Switch::Codex {
        day,
        next: logwatch::codex_day(sessions, 1),
    }
}

#[cfg(test)]
#[path = "logcache_tests.rs"]
mod tests;
