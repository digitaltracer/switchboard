//! Which agents notify: Switchboard's own `sb-` agents by default, others opted in with
//! `watch`, minus mutes. The hub's store keeps these choices; the bridge's old file beside
//! the WhatsApp store is imported once.

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

/// The bridge's old watch file: `whatsapp.db` kept its choices in `whatsapp-watch.json`.
pub fn legacy_path(store_path: &Path) -> PathBuf {
    let stem = store_path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("whatsapp");
    store_path.with_file_name(format!("{stem}-watch.json"))
}

/// The old file's choices, if it exists. An unreadable one is an error, so mutes aren't lost silently.
pub fn load_legacy(path: &Path) -> io::Result<Option<WatchState>> {
    match std::fs::read(path) {
        Ok(bytes) => serde_json::from_slice(&bytes).map(Some).map_err(|e| {
            io::Error::other(format!("{} is not valid watch state: {e}", path.display()))
        }),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e),
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
    fn the_old_file_is_read_for_import() {
        let dir = tempfile::tempdir().unwrap();
        let path = legacy_path(&dir.path().join("whatsapp.db"));
        assert_eq!(path.file_name().unwrap(), "whatsapp-watch.json");
        assert_eq!(load_legacy(&path).unwrap(), None);
        std::fs::write(
            &path,
            r#"{"watched":["impl"],"muted":["sb-1"],"mute_all":true}"#,
        )
        .unwrap();
        let s = load_legacy(&path).unwrap().unwrap();
        assert!(s.watched.contains("impl") && s.muted.contains("sb-1") && s.mute_all);
        std::fs::write(&path, "{not json").unwrap();
        assert!(load_legacy(&path).is_err());
    }
}
