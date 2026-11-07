use std::path::{Path, PathBuf};
use std::time::Duration;

use serde_json::json;

use super::*;

const OLD: &str = "aaaa-1111";
const NEW: &str = "bbbb-2222";
const PID: u32 = 4242;

struct Dirs {
    _t: tempfile::TempDir,
    claude: PathBuf,
    codex: PathBuf,
    work: PathBuf,
}

fn dirs() -> Dirs {
    let t = tempfile::tempdir().unwrap();
    let claude = t.path().join("claude");
    std::fs::create_dir_all(claude.join("sessions")).unwrap();
    std::fs::create_dir_all(claude.join("projects/-repo")).unwrap();
    let codex = t.path().join("codex");
    let work = t.path().join("work");
    std::fs::create_dir_all(&work).unwrap();
    Dirs {
        claude,
        codex,
        work,
        _t: t,
    }
}

impl Dirs {
    fn cache(&self) -> LogCache {
        LogCache::new(self.claude.join("projects"), self.codex.clone())
    }

    fn record(&self, pid: u32, v: serde_json::Value) {
        std::fs::write(
            self.claude.join(format!("sessions/{pid}.json")),
            v.to_string(),
        )
        .unwrap();
    }

    fn claude_log(&self, session: &str) -> PathBuf {
        let p = self.claude.join(format!("projects/-repo/{session}.jsonl"));
        std::fs::write(&p, "{}\n").unwrap();
        p
    }

    fn codex_log(&self, day: &Path, name: &str) -> PathBuf {
        std::fs::create_dir_all(day).unwrap();
        let p = day.join(format!("rollout-{name}.jsonl"));
        let meta = json!({"type": "session_meta", "payload": {"cwd": self.work}});
        std::fs::write(&p, format!("{meta}\n")).unwrap();
        p
    }

    fn today(&self) -> PathBuf {
        logwatch::codex_day(&self.codex, 0)
    }
}

fn claude(terminal: &str) -> Pane {
    Pane {
        server: "/herdr.sock".into(),
        pane_id: "w1:p1".into(),
        terminal_id: Some(terminal.into()),
        kind: "claude".into(),
        reported: Some(OLD.into()),
        cwd: Some("/repo".into()),
    }
}

fn codex(d: &Dirs) -> Pane {
    Pane {
        kind: "codex".into(),
        reported: None,
        cwd: Some(d.work.to_string_lossy().into_owned()),
        ..claude("t1")
    }
}

/// The cached log, else a search's; and whether it was a hit.
fn find(c: &LogCache, pane: &Pane) -> (Option<PathBuf>, bool) {
    match c.hit(pane) {
        Some(f) => (Some(f.log), true),
        None => (c.search(pane.clone(), vec![PID]).map(|f| f.log), false),
    }
}

/// Lets a modification time move on file systems that keep coarse ones.
fn tick() {
    std::thread::sleep(Duration::from_millis(20));
}

#[test]
fn repeated_hits_make_no_search() {
    let d = dirs();
    let log = d.claude_log(OLD);
    d.record(PID, json!({"pid": PID, "sessionId": OLD}));
    let c = d.cache();
    assert_eq!(find(&c, &claude("t1")), (Some(log.clone()), false));
    for _ in 0..5 {
        assert_eq!(find(&c, &claude("t1")), (Some(log.clone()), true));
    }
    assert_eq!(c.searches(), 1);
}

#[test]
fn a_new_process_in_the_pane_misses() {
    let d = dirs();
    d.claude_log(OLD);
    d.record(PID, json!({"pid": PID, "sessionId": OLD}));
    let c = d.cache();
    find(&c, &claude("t1"));
    assert!(c.hit(&claude("t2")).is_none());
    let other_session = Pane {
        reported: Some(NEW.into()),
        ..claude("t1")
    };
    assert!(
        c.hit(&other_session).is_none(),
        "Herdr reports another session"
    );
}

#[test]
fn a_session_handed_to_a_background_job_misses() {
    let d = dirs();
    d.claude_log(OLD);
    let new = d.claude_log(NEW);
    d.record(PID, json!({"pid": PID, "sessionId": OLD}));
    let c = d.cache();
    find(&c, &claude("t1"));
    d.record(
        PID,
        json!({"pid": PID, "sessionId": OLD, "parkedJobId": "6ffb85a5"}),
    );
    d.record(
        5000,
        json!({"pid": 5000, "sessionId": NEW, "jobId": "6ffb85a5"}),
    );
    assert_eq!(find(&c, &claude("t1")), (Some(new), false));
}

#[test]
fn clear_rewriting_the_record_in_place_misses() {
    let d = dirs();
    d.claude_log(OLD);
    let new = d.claude_log(NEW);
    d.record(PID, json!({"pid": PID, "sessionId": OLD}));
    let c = d.cache();
    find(&c, &claude("t1"));
    d.record(PID, json!({"pid": PID, "sessionId": NEW}));
    assert_eq!(find(&c, &claude("t1")), (Some(new.clone()), false));
    assert_eq!(find(&c, &claude("t1")), (Some(new), true));
}

#[test]
fn a_replaced_log_misses() {
    let d = dirs();
    let log = d.claude_log(OLD);
    d.record(PID, json!({"pid": PID, "sessionId": OLD}));
    let c = d.cache();
    find(&c, &claude("t1"));
    let tmp = log.with_extension("tmp");
    std::fs::write(&tmp, "{}\n").unwrap();
    std::fs::rename(&tmp, &log).unwrap();
    assert_eq!(find(&c, &claude("t1")), (Some(log), false));
}

#[test]
fn a_session_from_herdr_alone_is_never_cached() {
    let d = dirs();
    let log = d.claude_log(OLD);
    let c = d.cache();
    assert_eq!(find(&c, &claude("t1")), (Some(log.clone()), false));
    assert_eq!(find(&c, &claude("t1")), (Some(log), false));
}

#[test]
fn no_log_yet_is_not_cached_and_the_log_is_found_once_it_appears() {
    let d = dirs();
    d.record(PID, json!({"pid": PID, "sessionId": OLD}));
    let c = d.cache();
    assert_eq!(find(&c, &claude("t1")), (None, false));
    let log = d.claude_log(OLD);
    assert_eq!(find(&c, &claude("t1")), (Some(log), false));
}

#[test]
fn entries_are_searched_again_after_the_revalidation_interval() {
    let d = dirs();
    let log = d.claude_log(OLD);
    d.record(PID, json!({"pid": PID, "sessionId": OLD}));
    let c = LogCache::revalidating(
        d.claude.join("projects"),
        d.codex.clone(),
        Duration::from_millis(50),
    );
    find(&c, &claude("t1"));
    assert!(c.hit(&claude("t1")).is_some());
    std::thread::sleep(Duration::from_millis(60));
    assert_eq!(find(&c, &claude("t1")), (Some(log), false));
    assert_eq!(c.searches(), 2);
}

#[test]
fn panes_missing_from_the_snapshot_are_dropped() {
    let d = dirs();
    d.claude_log(OLD);
    d.record(PID, json!({"pid": PID, "sessionId": OLD}));
    let c = d.cache();
    find(&c, &claude("t1"));
    c.retain(Path::new("/other.sock"), &HashSet::new());
    assert!(c.hit(&claude("t1")).is_some(), "another server's snapshot");
    c.retain(
        Path::new("/herdr.sock"),
        &HashSet::from(["w1:p1".to_owned()]),
    );
    assert!(c.hit(&claude("t1")).is_some());
    c.retain(Path::new("/herdr.sock"), &HashSet::new());
    assert!(c.hit(&claude("t1")).is_none());
}

#[test]
fn a_new_codex_log_today_misses() {
    let d = dirs();
    let today = d.today();
    let first = d.codex_log(&today, "a");
    let c = d.cache();
    assert_eq!(find(&c, &codex(&d)), (Some(first.clone()), false));
    assert_eq!(find(&c, &codex(&d)), (Some(first), true));
    tick();
    let second = d.codex_log(&today, "b");
    assert_eq!(find(&c, &codex(&d)), (Some(second), false));
}

#[test]
fn writes_to_an_older_days_codex_log_switch_to_it() {
    let d = dirs();
    let old = d.codex_log(&d.codex.join("2020/01/01"), "old");
    tick();
    let today = d.codex_log(&d.today(), "new");
    let c = d.cache();
    assert_eq!(find(&c, &codex(&d)), (Some(today.clone()), false));
    assert_eq!(find(&c, &codex(&d)), (Some(today), true));
    tick();
    // A resumed session writes to the log it started in.
    let mut f = std::fs::OpenOptions::new().append(true).open(&old).unwrap();
    std::io::Write::write_all(&mut f, b"{}\n").unwrap();
    drop(f);
    assert_eq!(find(&c, &codex(&d)), (Some(old), false));
}

#[test]
fn a_codex_rescan_reads_only_new_or_changed_logs() {
    let d = dirs();
    let today = d.today();
    for n in ["a", "b", "c"] {
        d.codex_log(&today, n);
    }
    let c = d.cache();
    c.search(codex(&d), vec![]);
    assert_eq!(c.header_reads(), 3);
    c.search(codex(&d), vec![]);
    assert_eq!(c.header_reads(), 3, "nothing changed");
    tick();
    d.codex_log(&today, "d");
    c.search(codex(&d), vec![]);
    assert_eq!(c.header_reads(), 4);
}

fn tool_use(id: &str) -> String {
    let v = json!({"type": "assistant", "uuid": "a1", "message": {"content": [
        {"type": "tool_use", "id": id, "name": "Bash", "input": {"command": "ls"}}]}});
    format!("{v}\n")
}

/// A cached Claude pane whose log asks for tool call `toolu_A`.
fn pending_rig(d: &Dirs) -> (LogCache, PathBuf) {
    let log = d.claude_log(OLD);
    std::fs::write(&log, tool_use("toolu_A")).unwrap();
    d.record(PID, json!({"pid": PID, "sessionId": OLD}));
    let c = d.cache();
    find(&c, &claude("t1"));
    (c, log)
}

#[test]
fn an_unchanged_log_is_parsed_once_for_the_same_pending_call() {
    let d = dirs();
    let (c, log) = pending_rig(&d);
    let key = claude("t1").key();
    for _ in 0..4 {
        assert_eq!(c.pending_call(&key, &log), transcript::pending_call(&log));
    }
    assert_eq!(c.pending_call(&key, &log).as_deref(), Some("toolu_A"));
    assert_eq!(c.parses(), 1);
}

#[test]
fn a_grown_log_is_parsed_again() {
    let d = dirs();
    let (c, log) = pending_rig(&d);
    let key = claude("t1").key();
    assert_eq!(c.pending_call(&key, &log).as_deref(), Some("toolu_A"));
    tick();
    let result = json!({"type": "user", "uuid": "u1", "message": {"content": [
        {"type": "tool_result", "tool_use_id": "toolu_A", "content": "ok"}]}});
    let mut f = std::fs::OpenOptions::new().append(true).open(&log).unwrap();
    std::io::Write::write_all(&mut f, format!("{result}\n").as_bytes()).unwrap();
    drop(f);
    assert_eq!(c.pending_call(&key, &log), None);
    assert_eq!(c.parses(), 2);
}

#[test]
fn a_same_length_rewrite_is_parsed_again() {
    let d = dirs();
    let (c, log) = pending_rig(&d);
    let key = claude("t1").key();
    assert_eq!(c.pending_call(&key, &log).as_deref(), Some("toolu_A"));
    tick();
    std::fs::write(&log, tool_use("toolu_B")).unwrap();
    assert_eq!(c.pending_call(&key, &log).as_deref(), Some("toolu_B"));
}

#[test]
fn the_pending_call_goes_with_its_pane() {
    let d = dirs();
    let (c, log) = pending_rig(&d);
    let key = claude("t1").key();
    c.pending_call(&key, &log);
    c.retain(Path::new("/herdr.sock"), &HashSet::new());
    // With no entry the call is still read, just not kept.
    assert_eq!(c.pending_call(&key, &log).as_deref(), Some("toolu_A"));
    assert_eq!(c.pending_call(&key, &log).as_deref(), Some("toolu_A"));
    assert_eq!(c.parses(), 3);
}
