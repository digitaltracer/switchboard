//! Which agents notify: Switchboard's own `sb-` agents by default, others opted in with
//! `watch`, minus mutes. Saved next to the WhatsApp store so choices survive restarts.

use std::collections::BTreeSet;
use std::io;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// The runner names every session's agent `sb-<id>`.
pub const SWITCHBOARD_PREFIX: &str = "sb-";

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct WatchState {
    /// Agents outside Switchboard that were opted in.
    #[serde(default)]
    pub watched: BTreeSet<String>,
    #[serde(default)]
    pub muted: BTreeSet<String>,
    /// Silences every agent, including ones created later, until `unmute all`.
    #[serde(default)]
    pub mute_all: bool,
}

impl WatchState {
    pub fn is_watched(&self, agent: &str) -> bool {
        agent.starts_with(SWITCHBOARD_PREFIX) || self.watched.contains(agent)
    }

    pub fn notifies(&self, agent: &str) -> bool {
        self.is_watched(agent) && !self.mute_all && !self.muted.contains(agent)
    }

    /// How `status` and `list` describe an agent.
    pub fn describe(&self, agent: &str) -> &'static str {
        match (
            self.is_watched(agent),
            self.mute_all,
            self.muted.contains(agent),
        ) {
            (false, _, _) => "not watched",
            (true, true, _) => "muted (all)",
            (true, false, true) => "muted",
            (true, false, false) if agent.starts_with(SWITCHBOARD_PREFIX) => "watching",
            (true, false, false) => "watching (opted in)",
        }
    }
}

pub struct WatchStore {
    path: Option<PathBuf>,
    pub state: WatchState,
}

impl WatchStore {
    /// Not saved anywhere; for tests.
    pub fn in_memory() -> Self {
        Self {
            path: None,
            state: WatchState::default(),
        }
    }

    /// `whatsapp.db` keeps its choices in `whatsapp-watch.json` beside it.
    pub fn path_for(store_path: &Path) -> PathBuf {
        let stem = store_path
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("whatsapp");
        store_path.with_file_name(format!("{stem}-watch.json"))
    }

    /// A missing file is a fresh start; an unreadable one is an error, so mutes aren't lost silently.
    pub fn load(path: &Path) -> io::Result<Self> {
        let state = match std::fs::read(path) {
            Ok(bytes) => serde_json::from_slice(&bytes).map_err(|e| {
                io::Error::other(format!("{} is not valid watch state: {e}", path.display()))
            })?,
            Err(e) if e.kind() == io::ErrorKind::NotFound => WatchState::default(),
            Err(e) => return Err(e),
        };
        Ok(Self {
            path: Some(path.to_owned()),
            state,
        })
    }

    /// Applies `change` and saves it; on a failed save the change is undone.
    pub fn update(&mut self, change: impl FnOnce(&mut WatchState)) -> io::Result<()> {
        let before = self.state.clone();
        change(&mut self.state);
        if let Err(e) = self.save() {
            self.state = before;
            return Err(e);
        }
        Ok(())
    }

    fn save(&self) -> io::Result<()> {
        use std::io::Write;
        use std::os::unix::fs::OpenOptionsExt;
        let Some(path) = &self.path else {
            return Ok(());
        };
        let tmp = path.with_extension("json.tmp");
        let mut f = std::fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .mode(0o600)
            .open(&tmp)?;
        f.write_all(&serde_json::to_vec_pretty(&self.state).map_err(io::Error::other)?)?;
        f.sync_all()?;
        std::fs::rename(&tmp, path)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn switchboard_agents_are_watched_by_default() {
        let s = WatchState::default();
        assert!(s.notifies("sb-3f9a"));
        assert!(!s.notifies("switchboard-impl"));
        assert_eq!(s.describe("sb-3f9a"), "watching");
        assert_eq!(s.describe("switchboard-impl"), "not watched");
    }

    #[test]
    fn mutes_and_opt_ins() {
        let mut s = WatchState::default();
        s.watched.insert("switchboard-impl".into());
        assert!(s.notifies("switchboard-impl"));
        assert_eq!(s.describe("switchboard-impl"), "watching (opted in)");
        s.muted.insert("sb-1".into());
        assert!(!s.notifies("sb-1"));
        assert_eq!(s.describe("sb-1"), "muted");
        s.mute_all = true;
        assert!(!s.notifies("sb-created-later") && !s.notifies("switchboard-impl"));
        assert_eq!(s.describe("sb-created-later"), "muted (all)");
        assert_eq!(s.describe("reviewer"), "not watched");
    }

    #[test]
    fn saved_choices_survive_reload_with_private_file() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let path = WatchStore::path_for(&dir.path().join("whatsapp.db"));
        assert_eq!(path.file_name().unwrap(), "whatsapp-watch.json");
        let mut store = WatchStore::load(&path).unwrap();
        assert_eq!(store.state, WatchState::default());
        store
            .update(|s| {
                s.muted.insert("sb-1".into());
                s.mute_all = true;
            })
            .unwrap();
        let again = WatchStore::load(&path).unwrap();
        assert!(again.state.muted.contains("sb-1") && again.state.mute_all);
        assert_eq!(
            std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o600
        );
    }

    #[test]
    fn failed_save_undoes_the_change_and_corrupt_file_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        let mut store = WatchStore::load(&dir.path().join("missing-dir/w.json")).unwrap();
        assert!(store.update(|s| s.mute_all = true).is_err());
        assert!(!store.state.mute_all, "state matches what is on disk");

        let bad = dir.path().join("bad.json");
        std::fs::write(&bad, "{not json").unwrap();
        assert!(WatchStore::load(&bad).is_err());
    }
}
