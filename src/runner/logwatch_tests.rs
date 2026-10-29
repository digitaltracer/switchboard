use std::io::Write;
use std::sync::atomic::{AtomicUsize, Ordering};

use super::*;

const PID: u32 = 4242;
const AGENT: &str = "mono1/claude";

fn watcher() -> Arc<LogWatch> {
    LogWatch::lingering(Duration::ZERO)
}

/// Waits for released folders to be swept.
async fn released(watch: &LogWatch) -> usize {
    for _ in 0..100 {
        if watch.folders() == 0 {
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    watch.folders()
}

/// A Claude agent's world: its record, its log, and a second log it could move to.
struct World {
    _dir: tempfile::TempDir,
    claude: PathBuf,
    log: PathBuf,
    other: PathBuf,
}

impl World {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let claude = dir.path().join("claude");
        let project = claude.join("projects/-Users-me-mono1");
        std::fs::create_dir_all(&project).unwrap();
        std::fs::create_dir_all(claude.join("sessions")).unwrap();
        let w = Self {
            log: project.join("aaaa.jsonl"),
            other: project.join("bbbb.jsonl"),
            claude,
            _dir: dir,
        };
        std::fs::write(&w.log, "{\"n\":1}\n").unwrap();
        std::fs::write(&w.other, "{\"n\":1}\n{\"n\":2}\n").unwrap();
        w.record("aaaa");
        w
    }

    fn record(&self, session: &str) {
        std::fs::write(
            self.claude.join(format!("sessions/{PID}.json")),
            format!("{{\"pid\": {PID}, \"sessionId\": \"{session}\"}}"),
        )
        .unwrap();
    }

    fn found(&self, log: &Path, session: &str) -> Found {
        Found {
            log: log.to_owned(),
            switch: Switch::Claude {
                records: self.claude.join("sessions"),
                pids: vec![PID],
                session: session.into(),
            },
        }
    }

    fn len(&self) -> u64 {
        std::fs::metadata(&self.log).unwrap().len()
    }

    /// The lookup a wait runs: which log the record names. Counts its runs.
    fn lookup(&self, runs: &Arc<AtomicUsize>) -> impl FnMut() -> std::future::Ready<Option<Found>> {
        let (claude, runs) = (self.claude.clone(), runs.clone());
        let found = |log: &Path, s: &str| self.found(log, s);
        let (a, b) = (found(&self.log, "aaaa"), found(&self.other, "bbbb"));
        move || {
            runs.fetch_add(1, Ordering::SeqCst);
            let s = transcript::live_claude_session(&claude, &[PID]);
            std::future::ready(Some(if s.as_deref() == Some("bbbb") {
                b.clone()
            } else {
                a.clone()
            }))
        }
    }
}

fn append(path: &Path, text: &str) {
    let mut f = std::fs::OpenOptions::new().append(true).open(path).unwrap();
    f.write_all(text.as_bytes()).unwrap();
}

fn later(after: Duration, f: impl FnOnce() + Send + 'static) {
    tokio::spawn(async move {
        tokio::time::sleep(after).await;
        f();
    });
}

#[tokio::test]
async fn an_appended_line_ends_the_wait_within_a_second() {
    let _turn = TIMED.lock().await;
    let w = World::new();
    let watch = watcher();
    let runs = Arc::new(AtomicUsize::new(0));
    let log = w.log.clone();
    later(Duration::from_millis(300), move || {
        append(&log, "{\"n\":2}\n")
    });
    let start = Instant::now();
    let got = wait(
        &watch,
        None,
        AGENT,
        w.len(),
        start + MAX_WAIT,
        w.found(&w.log, "aaaa"),
        w.lookup(&runs),
    )
    .await;
    assert_eq!(got, w.log);
    let took = start.elapsed();
    assert!(
        took >= Duration::from_millis(300) && took < Duration::from_secs(1),
        "{took:?}"
    );
    assert_eq!(runs.load(Ordering::SeqCst), 0, "no lookups");
    assert_eq!(released(&watch).await, 0, "every folder released");
}

#[tokio::test]
async fn a_line_written_before_the_watch_is_set_up_is_still_seen() {
    let _turn = TIMED.lock().await;
    let w = World::new();
    let watch = watcher();
    let after = w.len();
    append(&w.log, "{\"n\":2}\n");
    let start = Instant::now();
    let runs = Arc::new(AtomicUsize::new(0));
    wait(
        &watch,
        None,
        AGENT,
        after,
        start + MAX_WAIT,
        w.found(&w.log, "aaaa"),
        w.lookup(&runs),
    )
    .await;
    assert!(
        start.elapsed() < Duration::from_secs(1),
        "{:?}",
        start.elapsed()
    );
}

#[tokio::test]
async fn nothing_new_or_only_part_of_a_line_waits_until_the_deadline() {
    let _turn = TIMED.lock().await;
    let w = World::new();
    let watch = watcher();
    let runs = Arc::new(AtomicUsize::new(0));
    let log = w.log.clone();
    later(Duration::from_millis(100), move || append(&log, "{\"n\":"));
    let after = w.len();
    let start = Instant::now();
    let got = wait(
        &watch,
        None,
        AGENT,
        after,
        start + Duration::from_millis(700),
        w.found(&w.log, "aaaa"),
        w.lookup(&runs),
    )
    .await;
    assert!(start.elapsed() >= Duration::from_millis(700));
    let page = transcript::read_page(&got, Some(after), None).unwrap();
    assert!(page.items.is_empty() && page.end == after, "{page:?}");
    assert_eq!(runs.load(Ordering::SeqCst), 0);
    assert_eq!(released(&watch).await, 0);
}

#[tokio::test]
async fn a_claude_record_replaced_or_rewritten_with_another_session_ends_the_wait() {
    let _turn = TIMED.lock().await;
    let w = World::new();
    let watch = watcher();
    let runs = Arc::new(AtomicUsize::new(0));

    // `/clear` as an atomic replacement of the record.
    let claude = w.claude.clone();
    later(Duration::from_millis(300), move || {
        let tmp = claude.join("sessions/.tmp");
        std::fs::write(&tmp, format!("{{\"pid\": {PID}, \"sessionId\": \"bbbb\"}}")).unwrap();
        std::fs::rename(&tmp, claude.join(format!("sessions/{PID}.json"))).unwrap();
    });
    let start = Instant::now();
    let got = wait(
        &watch,
        None,
        AGENT,
        w.len(),
        start + MAX_WAIT,
        w.found(&w.log, "aaaa"),
        w.lookup(&runs),
    )
    .await;
    assert_eq!(got, w.other);
    assert!(
        start.elapsed() < Duration::from_secs(1),
        "{:?}",
        start.elapsed()
    );
    assert_eq!(runs.load(Ordering::SeqCst), 1);

    // The same record written in place, back to the first session.
    let runs = Arc::new(AtomicUsize::new(0));
    let claude = w.claude.clone();
    later(Duration::from_millis(300), move || {
        std::fs::write(
            claude.join(format!("sessions/{PID}.json")),
            format!("{{\"pid\": {PID}, \"sessionId\": \"aaaa\"}}"),
        )
        .unwrap();
    });
    let start = Instant::now();
    let got = wait(
        &watch,
        None,
        AGENT,
        std::fs::metadata(&w.other).unwrap().len(),
        start + MAX_WAIT,
        w.found(&w.other, "bbbb"),
        w.lookup(&runs),
    )
    .await;
    assert_eq!(got, w.log);
    assert!(
        start.elapsed() < Duration::from_secs(1),
        "{:?}",
        start.elapsed()
    );
    assert_eq!(released(&watch).await, 0);
}

#[tokio::test]
async fn another_claudes_record_updating_its_status_looks_for_nothing() {
    let _turn = TIMED.lock().await;
    let w = World::new();
    std::fs::write(
        w.claude.join("sessions/777.json"),
        "{\"pid\": 777, \"sessionId\": \"cccc\", \"status\": \"idle\"}",
    )
    .unwrap();
    let watch = watcher();
    let runs = Arc::new(AtomicUsize::new(0));
    let claude = w.claude.clone();
    later(Duration::from_millis(200), move || {
        std::fs::write(
            claude.join("sessions/777.json"),
            "{\"pid\": 777, \"sessionId\": \"cccc\", \"status\": \"busy\"}",
        )
        .unwrap();
    });
    let start = Instant::now();
    wait(
        &watch,
        None,
        AGENT,
        w.len(),
        start + Duration::from_millis(700),
        w.found(&w.log, "aaaa"),
        w.lookup(&runs),
    )
    .await;
    assert_eq!(runs.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn a_new_codex_log_in_todays_folder_ends_the_wait() {
    let _turn = TIMED.lock().await;
    let dir = tempfile::tempdir().unwrap();
    let day = codex_day(dir.path());
    std::fs::create_dir_all(&day).unwrap();
    let log = day.join("rollout-2026-10-29T10-00-00-old.jsonl");
    let new = day.join("rollout-2026-10-29T10-05-00-new.jsonl");
    std::fs::write(&log, "{}\n").unwrap();
    let found = |log: &Path| Found {
        log: log.to_owned(),
        switch: Switch::Codex { day: day.clone() },
    };
    let runs = Arc::new(AtomicUsize::new(0));
    let (n, next) = (runs.clone(), found(&new));
    let lookup = move || {
        n.fetch_add(1, Ordering::SeqCst);
        std::future::ready(Some(next.clone()))
    };
    let path = new.clone();
    later(Duration::from_millis(300), move || {
        std::fs::write(&path, "{}\n").unwrap()
    });
    let watch = watcher();
    let start = Instant::now();
    let got = wait(
        &watch,
        None,
        "codex",
        3,
        start + MAX_WAIT,
        found(&log),
        lookup,
    )
    .await;
    assert_eq!(got, new);
    assert!(
        start.elapsed() < Duration::from_secs(1),
        "{:?}",
        start.elapsed()
    );
    assert_eq!(runs.load(Ordering::SeqCst), 1);
    assert_eq!(released(&watch).await, 0);
}

#[tokio::test]
async fn a_status_change_looks_again_and_a_new_log_ends_the_wait() {
    let _turn = TIMED.lock().await;
    let w = World::new();
    let watch = watcher();
    let (tx, rx) = broadcast::channel(8);
    let runs = Arc::new(AtomicUsize::new(0));
    // Another agent's change looks for nothing.
    let _ = tx.send("someone-else".to_owned());
    // The record changed without an event, as a missed one would.
    w.record("bbbb");
    let t = tx.clone();
    later(Duration::from_millis(300), move || {
        drop(t.send(AGENT.to_owned()))
    });
    let start = Instant::now();
    let got = wait(
        &watch,
        Some(rx),
        AGENT,
        w.len(),
        start + MAX_WAIT,
        w.found(&w.log, "aaaa"),
        w.lookup(&runs),
    )
    .await;
    assert_eq!(got, w.other);
    assert!(start.elapsed() < Duration::from_secs(1));
    assert_eq!(runs.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn a_slow_lookup_still_answers_by_the_deadline() {
    let _turn = TIMED.lock().await;
    let w = World::new();
    let watch = watcher();
    let (tx, rx) = broadcast::channel(8);
    let _ = tx.send(AGENT.to_owned());
    let start = Instant::now();
    let got = wait(
        &watch,
        Some(rx),
        AGENT,
        w.len(),
        start + Duration::from_millis(500),
        w.found(&w.log, "aaaa"),
        std::future::pending::<Option<Found>>,
    )
    .await;
    assert_eq!(got, w.log);
    let took = start.elapsed();
    assert!(
        took >= Duration::from_millis(500) && took < Duration::from_millis(900),
        "{took:?}"
    );
    assert_eq!(released(&watch).await, 0);
}

#[tokio::test]
async fn without_a_watcher_the_log_is_checked_each_second() {
    let _turn = TIMED.lock().await;
    let w = World::new();
    let watch = LogWatch::unavailable();
    let log = w.log.clone();
    later(Duration::from_millis(200), move || {
        append(&log, "{\"n\":2}\n")
    });
    let runs = Arc::new(AtomicUsize::new(0));
    let start = Instant::now();
    wait(
        &watch,
        None,
        AGENT,
        w.len(),
        start + MAX_WAIT,
        w.found(&w.log, "aaaa"),
        w.lookup(&runs),
    )
    .await;
    let took = start.elapsed();
    assert!(
        took >= Duration::from_millis(900) && took < Duration::from_millis(1500),
        "{took:?}"
    );
}

#[tokio::test]
async fn waits_on_one_folder_share_its_watch_and_a_quick_next_wait_reuses_it() {
    let _turn = TIMED.lock().await;
    let w = World::new();
    let project = w.log.parent().unwrap().to_owned();
    let watch = watcher();
    let a = watch.watch(&[project.clone(), w.claude.join("sessions")]);
    let b = watch.watch(&[project.clone(), w.claude.join("nowhere")]);
    assert!(a.is_some() && b.is_some());
    assert_eq!(watch.folders(), 2);
    drop(a);
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert_eq!(watch.folders(), 1, "the project folder is still b's");
    drop(b);
    assert_eq!(released(&watch).await, 0);
    assert!(
        watch.watch(&[w.claude.join("nowhere")]).is_none(),
        "the log's own folder can't be watched"
    );
    assert_eq!(watch.folders(), 0);

    // Within the linger, the next wait finds its folder still watched: no restart.
    let watch = LogWatch::lingering(Duration::from_secs(5));
    drop(watch.watch(std::slice::from_ref(&project)));
    let mut changes = watch.subscribe();
    let again = watch.watch(std::slice::from_ref(&project));
    assert!(again.is_some());
    assert!(changes.try_recv().is_err(), "nothing was set up again");
    assert_eq!(watch.folders(), 1);
}

#[cfg(target_os = "macos")]
#[tokio::test]
async fn on_macos_an_append_to_a_log_held_open_ends_the_wait_within_a_second() {
    let _turn = TIMED.lock().await;
    let w = World::new();
    let watch = watcher();
    let runs = Arc::new(AtomicUsize::new(0));
    // Codex keeps its log open, and FSEvents reports such a write only on close.
    let mut held = std::fs::OpenOptions::new()
        .append(true)
        .open(&w.log)
        .unwrap();
    let after = w.len();
    let writer = tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(600)).await;
        held.write_all(b"{\"n\":2}\n").unwrap();
        held.flush().unwrap();
        held
    });
    let start = Instant::now();
    let got = wait(
        &watch,
        None,
        AGENT,
        after,
        start + MAX_WAIT,
        w.found(&w.log, "aaaa"),
        w.lookup(&runs),
    )
    .await;
    assert_eq!(got, w.log);
    let took = start.elapsed();
    let _still_open = writer.await.unwrap();
    assert!(
        took >= Duration::from_millis(600) && took < Duration::from_millis(1600),
        "{took:?}"
    );
}

#[tokio::test]
async fn a_watch_that_fails_to_start_holds_nothing_so_the_wait_checks_each_second() {
    let _turn = TIMED.lock().await;
    let w = World::new();
    let watch = watcher();
    watch
        .fail_start
        .store(true, std::sync::atomic::Ordering::SeqCst);
    let project = w.log.parent().unwrap().to_owned();
    assert!(watch.watch(&[project, w.claude.join("sessions")]).is_none());
    assert_eq!(
        watch.folders(),
        0,
        "a later wait would take the folders as watched"
    );
}

#[tokio::test]
async fn a_setup_stuck_behind_a_stalled_restart_still_answers_by_the_deadline() {
    let _turn = TIMED.lock().await;
    let w = World::new();
    let watch = watcher();
    // A sweep restarting the stream holds the watcher's lock; this one doesn't let go for 3 s.
    let (held, locked) = std::sync::mpsc::channel();
    let stalled = watch.clone();
    let sweep = std::thread::spawn(move || {
        let _restarting = stalled.watcher.lock().unwrap();
        held.send(()).unwrap();
        std::thread::sleep(Duration::from_secs(3));
    });
    locked.recv().unwrap();
    let runs = Arc::new(AtomicUsize::new(0));
    let start = Instant::now();
    let got = wait(
        &watch,
        None,
        AGENT,
        w.len(),
        start + Duration::from_millis(500),
        w.found(&w.log, "aaaa"),
        w.lookup(&runs),
    )
    .await;
    assert_eq!(got, w.log);
    let took = start.elapsed();
    assert!(took < Duration::from_millis(900), "{took:?}");
    sweep.join().unwrap();
}

#[tokio::test]
async fn the_first_codex_log_of_a_day_ends_the_wait_before_its_folder_existed() {
    let _turn = TIMED.lock().await;
    let dir = tempfile::tempdir().unwrap();
    let sessions = dir.path().join("sessions");
    let yesterday = sessions.join("2026/10/29");
    std::fs::create_dir_all(&yesterday).unwrap();
    let log = yesterday.join("rollout-2026-10-29T23-50-00-old.jsonl");
    std::fs::write(&log, "{}\n").unwrap();
    // Not made until Codex starts its first session of the day.
    let day = sessions.join("2030/01/01");
    let new = day.join("rollout-2030-01-01T00-01-00-new.jsonl");
    let found = |log: &Path| Found {
        log: log.to_owned(),
        switch: Switch::Codex { day: day.clone() },
    };
    let runs = Arc::new(AtomicUsize::new(0));
    let (n, path, old) = (runs.clone(), new.clone(), log.clone());
    let lookup = move || {
        n.fetch_add(1, Ordering::SeqCst);
        let log = if path.exists() {
            path.clone()
        } else {
            old.clone()
        };
        std::future::ready(Some(Found {
            log,
            switch: Switch::Codex {
                day: path.parent().unwrap().to_owned(),
            },
        }))
    };
    let (made, file) = (day.clone(), new.clone());
    // Well after the watch is set up, which a missing folder is otherwise noticed by.
    later(Duration::from_millis(1500), move || {
        std::fs::create_dir_all(&made).unwrap();
        std::fs::write(&file, "{}\n").unwrap();
    });
    let watch = watcher();
    let start = Instant::now();
    let got = wait(
        &watch,
        None,
        "codex",
        3,
        start + MAX_WAIT,
        found(&log),
        lookup,
    )
    .await;
    assert_eq!(got, new);
    let took = start.elapsed();
    assert!(took < Duration::from_millis(2500), "{took:?}");
    assert_eq!(released(&watch).await, 0);
}

/// Folder watches only, as on Linux, where no file watch backs them up.
fn folders_only() -> Arc<LogWatch> {
    Arc::new(LogWatch {
        watcher: Mutex::new(State::NotYet),
        folders: Mutex::default(),
        linger: Duration::ZERO,
        off: true,
        fail_start: Default::default(),
        changes: broadcast::channel(1024).0,
    })
}

#[tokio::test]
async fn a_restart_that_fails_moves_every_running_wait_to_checking_each_second() {
    let _turn = TIMED.lock().await;
    let w = World::new();
    let watch = folders_only();
    let (breaker, log, other) = (watch.clone(), w.log.clone(), w.claude.join("projects"));
    tokio::spawn(async move {
        while breaker.folders() == 0 {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        // Another wait's folder restarts the stream, and the restart fails.
        breaker
            .fail_start
            .store(true, std::sync::atomic::Ordering::SeqCst);
        let b = breaker.clone();
        let started = tokio::task::spawn_blocking(move || b.watch(&[other]).is_some());
        assert!(!started.await.unwrap());
        assert_eq!(breaker.folders(), 0, "no folder counts as watched");
        tokio::time::sleep(Duration::from_millis(300)).await;
        append(&log, "{\"n\":2}\n");
    });
    let runs = Arc::new(AtomicUsize::new(0));
    let start = Instant::now();
    let got = wait(
        &watch,
        None,
        AGENT,
        w.len(),
        start + MAX_WAIT,
        w.found(&w.log, "aaaa"),
        w.lookup(&runs),
    )
    .await;
    assert_eq!(got, w.log);
    let took = start.elapsed();
    assert!(took < Duration::from_secs(4), "{took:?}");
    let project = w.log.parent().unwrap().to_owned();
    assert!(
        watch.watch(&[project]).is_none(),
        "later waits check each second"
    );
}
