//! Every Herdr session on this machine. Each new task gets a Herdr session of its own; agents
//! in any running session can be listed, read and prompted, named `<session>/<agent>`.
//! Single mode, for tests and older setups, keeps everything in one Herdr server.

use std::future::Future;
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;

use crate::herdr::{Client, session_socket};

const CALL_TIMEOUT: Duration = Duration::from_secs(30);
const PING_TIMEOUT: Duration = Duration::from_secs(2);
const START_TIMEOUT: Duration = Duration::from_secs(20);

type StartFn = Arc<
    dyn Fn(String, PathBuf) -> Pin<Box<dyn Future<Output = Result<(), String>> + Send>>
        + Send
        + Sync,
>;

/// How a new Herdr session's server is started.
#[derive(Clone)]
pub enum Starter {
    /// `herdr --session <name> server`, detached, with the caller's Herdr and Claude variables
    /// removed so panes start clean. `xdg` points Herdr at another config folder (tests).
    Herdr {
        bin: PathBuf,
        xdg: Option<PathBuf>,
        log_dir: PathBuf,
    },
    /// Called with the session name and its socket path; for tests with a scripted Herdr.
    Func(StartFn),
}

#[derive(Clone)]
struct Multi {
    /// Normally `~/.config/herdr`; sessions live under `sessions/<name>/`.
    config_dir: PathBuf,
    starter: Starter,
}

/// Herdr sessions: one per task, plus whatever the user runs.
#[derive(Clone)]
pub struct Herdrs {
    /// The server tasks went to before they had sessions of their own, and single mode's only one.
    legacy: PathBuf,
    multi: Option<Multi>,
}

/// Herdr session names: `[A-Za-z0-9_-]{1,64}`.
pub fn valid_session_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 64
        && name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
}

impl Herdrs {
    /// Everything in one Herdr server, as before.
    pub fn single(socket: impl Into<PathBuf>) -> Self {
        Self {
            legacy: socket.into(),
            multi: None,
        }
    }

    pub fn multi(config_dir: PathBuf, legacy: PathBuf, starter: Starter) -> Self {
        Self {
            legacy,
            multi: Some(Multi {
                config_dir,
                starter,
            }),
        }
    }

    pub fn is_multi(&self) -> bool {
        self.multi.is_some()
    }

    /// The client for a task's Herdr session, or the legacy server when it has none.
    pub fn client(&self, session: Option<&str>) -> Client {
        let socket = match (&self.multi, session) {
            (Some(m), Some(name)) => session_socket(&m.config_dir, name),
            _ => self.legacy.clone(),
        };
        Client::new(socket).with_timeout(CALL_TIMEOUT)
    }

    /// Every Herdr session that has a folder, running or not; `None` alone in single mode.
    pub fn known(&self) -> Vec<Option<String>> {
        let Some(m) = &self.multi else {
            return vec![None];
        };
        let mut names = Vec::new();
        if m.config_dir.join("herdr.sock").exists() || m.config_dir.join("session.json").exists() {
            names.push(Some("default".to_owned()));
        }
        if let Ok(entries) = std::fs::read_dir(m.config_dir.join("sessions")) {
            let mut found: Vec<String> = entries
                .flatten()
                .filter(|e| e.file_type().is_ok_and(|t| t.is_dir()))
                .map(|e| e.file_name().to_string_lossy().into_owned())
                .filter(|n| valid_session_name(n))
                .collect();
            found.sort();
            names.extend(found.into_iter().map(Some));
        }
        names
    }

    /// The sessions whose server answers, each with its client. Single mode has just its one.
    pub async fn running(&self) -> Vec<(Option<String>, Client)> {
        if self.multi.is_none() {
            return vec![(None, self.client(None))];
        }
        let mut out = Vec::new();
        for name in self.known() {
            let probe = self.client(name.as_deref()).with_timeout(PING_TIMEOUT);
            if probe.ping().await.is_ok() {
                out.push((name.clone(), self.client(name.as_deref())));
            }
        }
        out
    }

    /// A free session name: `base`, else `base` with more of `id`.
    pub fn free_name(&self, base: &str, id: &str) -> String {
        let taken = |n: &str| match &self.multi {
            Some(m) => m.config_dir.join("sessions").join(n).exists(),
            None => false,
        };
        if !taken(base) {
            return base.to_owned();
        }
        let mut name = format!("{base}-{id}");
        while taken(&name) {
            name.push('x');
        }
        name
    }

    /// Starts a new Herdr session for a task. `None` in single mode, where tasks share one server.
    pub async fn create(&self, name: &str) -> Result<Option<String>, String> {
        let Some(m) = &self.multi else {
            return Ok(None);
        };
        if !valid_session_name(name) {
            return Err(format!("{name:?} is not a valid Herdr session name"));
        }
        let socket = session_socket(&m.config_dir, name);
        let pid = match &m.starter {
            Starter::Func(f) => {
                f(name.to_owned(), socket.clone()).await?;
                None
            }
            Starter::Herdr { bin, xdg, log_dir } => {
                Some(spawn_server(bin, xdg.as_deref(), log_dir, name)?)
            }
        };
        let client = Client::new(&socket).with_timeout(PING_TIMEOUT);
        let deadline = tokio::time::Instant::now() + START_TIMEOUT;
        let failed = loop {
            match client.check_protocol().await {
                Ok(_) => return Ok(Some(name.to_owned())),
                Err(e @ crate::herdr::HerdrError::Incompatible { .. }) => break e.to_string(),
                Err(_) if tokio::time::Instant::now() >= deadline => {
                    break format!("Herdr session {name} didn't start");
                }
                Err(_) => tokio::time::sleep(Duration::from_millis(200)).await,
            }
        };
        // A server that never became usable isn't left running where no task can stop it.
        if let Some(pid) = pid {
            // SAFETY: a plain syscall on the server this call spawned.
            unsafe { libc::kill(pid as libc::pid_t, libc::SIGTERM) };
        }
        let log = match &m.starter {
            Starter::Herdr { log_dir, .. } => log_tail(&log_dir.join(format!("herdr-{name}.log"))),
            Starter::Func(_) => String::new(),
        };
        Err(if log.is_empty() {
            failed
        } else {
            format!("{failed}: {log}")
        })
    }

    /// Stops a task's Herdr session. It stays listed, and `herdr session attach` restarts it.
    pub async fn stop(&self, name: &str) -> Result<(), String> {
        let Some(m) = &self.multi else {
            return Ok(());
        };
        match &m.starter {
            Starter::Func(_) => Ok(()),
            Starter::Herdr { bin, xdg, .. } => {
                let mut cmd = tokio::process::Command::new(bin);
                cmd.args(["--session", name, "server", "stop"]);
                clean_env(&mut cmd);
                if let Some(x) = xdg {
                    cmd.env("XDG_CONFIG_HOME", x);
                }
                let out = cmd
                    .output()
                    .await
                    .map_err(|e| format!("running herdr: {e}"))?;
                if out.status.success() {
                    Ok(())
                } else {
                    Err(String::from_utf8_lossy(&out.stderr).trim().to_owned())
                }
            }
        }
    }

    /// Deletes a task's Herdr session: stops its server if it's still up, then `herdr session
    /// delete`. One already gone counts as deleted.
    pub async fn delete(&self, name: &str) -> Result<(), String> {
        if !valid_session_name(name) {
            return Err(format!("{name:?} is not a valid Herdr session name"));
        }
        let Some(m) = &self.multi else {
            return Ok(());
        };
        let dir = m.config_dir.join("sessions").join(name);
        let (bin, xdg) = match &m.starter {
            Starter::Func(_) => {
                let _ = std::fs::remove_dir_all(&dir);
                return Ok(());
            }
            Starter::Herdr { bin, xdg, .. } => (bin, xdg),
        };
        // A stopped server is already down; if it isn't, delete says so below.
        let _ = self.stop(name).await;
        let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
        loop {
            if !dir.exists() {
                return Ok(());
            }
            let mut cmd = tokio::process::Command::new(bin);
            cmd.args(["session", "delete", name]);
            clean_env(&mut cmd);
            if let Some(x) = xdg {
                cmd.env("XDG_CONFIG_HOME", x);
            }
            let out = cmd
                .output()
                .await
                .map_err(|e| format!("running herdr: {e}"))?;
            if out.status.success() {
                return Ok(());
            }
            let why = String::from_utf8_lossy(&out.stderr).trim().to_owned();
            // The server can take a moment to exit after `server stop` returns.
            if !why.contains("running") || tokio::time::Instant::now() >= deadline {
                return Err(why);
            }
            tokio::time::sleep(Duration::from_millis(300)).await;
        }
    }
}

/// An agent name as the hub sees it: `<session>/<agent>` in multi mode, else plain.
pub fn qualified(session: Option<&str>, agent: &str) -> String {
    match session {
        Some(s) => format!("{s}/{agent}"),
        None => agent.to_owned(),
    }
}

/// Splits `<session>/<agent>`; a plain name has no session.
pub fn split_qualified(name: &str) -> (Option<&str>, &str) {
    match name.split_once('/') {
        Some((s, a)) if valid_session_name(s) => (Some(s), a),
        _ => (None, name),
    }
}

/// Variables a new Herdr server must not inherit: they would point its panes at the caller's
/// own Herdr session or Claude Code process.
fn clean_env<C: CommandEnv>(cmd: &mut C) {
    for (k, _) in std::env::vars_os() {
        let k = k.to_string_lossy();
        if k.starts_with("HERDR_") || k.starts_with("CLAUDE_CODE") || k == "CLAUDECODE" {
            cmd.remove(&k);
        }
    }
}

trait CommandEnv {
    fn remove(&mut self, key: &str);
}

impl CommandEnv for std::process::Command {
    fn remove(&mut self, key: &str) {
        self.env_remove(key);
    }
}

impl CommandEnv for tokio::process::Command {
    fn remove(&mut self, key: &str) {
        self.env_remove(key);
    }
}

/// The last few lines of a server log, for an error message.
fn log_tail(path: &Path) -> String {
    let text = std::fs::read_to_string(path).unwrap_or_default();
    let lines: Vec<&str> = text.lines().rev().take(5).collect();
    lines.into_iter().rev().collect::<Vec<_>>().join(" | ")
}

/// `herdr --session <name> server` in a session of its own, so neither the runner exiting nor a
/// service manager stopping the runner's process group takes it down. Returns its pid.
fn spawn_server(bin: &Path, xdg: Option<&Path>, log_dir: &Path, name: &str) -> Result<u32, String> {
    use std::os::unix::process::CommandExt;
    std::fs::create_dir_all(log_dir).map_err(|e| format!("log folder: {e}"))?;
    let log = std::fs::File::create(log_dir.join(format!("herdr-{name}.log")))
        .map_err(|e| format!("log file: {e}"))?;
    let mut cmd = std::process::Command::new(bin);
    cmd.args(["--session", name, "server"])
        .stdin(std::process::Stdio::null())
        .stdout(log.try_clone().map_err(|e| e.to_string())?)
        .stderr(log);
    // SAFETY: setsid is async-signal-safe, as pre_exec requires.
    unsafe {
        cmd.pre_exec(|| {
            if libc::setsid() == -1 {
                return Err(std::io::Error::last_os_error());
            }
            Ok(())
        });
    }
    clean_env(&mut cmd);
    if let Some(x) = xdg {
        cmd.env("XDG_CONFIG_HOME", x);
    }
    let mut child = cmd.spawn().map_err(|e| format!("starting herdr: {e}"))?;
    let pid = child.id();
    // Reaps the server whenever it exits, so it never lingers as a zombie.
    std::thread::spawn(move || {
        let _ = child.wait();
    });
    Ok(pid)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_are_qualified_by_session() {
        assert_eq!(qualified(Some("mono1"), "claude"), "mono1/claude");
        assert_eq!(qualified(None, "sb-1a2b"), "sb-1a2b");
        assert_eq!(split_qualified("mono1/claude"), (Some("mono1"), "claude"));
        assert_eq!(split_qualified("sb-1a2b"), (None, "sb-1a2b"));
        assert_eq!(split_qualified("../x/y"), (None, "../x/y"));
        assert!(
            valid_session_name("switchboard-1a2b")
                && !valid_session_name("a b")
                && !valid_session_name("")
        );
    }

    #[test]
    fn sessions_are_found_by_folder_and_new_names_avoid_them() {
        let dir = tempfile::tempdir().unwrap();
        let h = Herdrs::multi(
            dir.path().to_owned(),
            dir.path().join("herdr.sock"),
            Starter::Func(Arc::new(|_, _| Box::pin(async { Ok(()) }))),
        );
        assert!(h.known().is_empty());
        for s in ["mono1", "pca-udp", "bad name"] {
            std::fs::create_dir_all(dir.path().join("sessions").join(s)).unwrap();
        }
        std::fs::write(dir.path().join("herdr.sock"), "").unwrap();
        assert_eq!(
            h.known(),
            [
                Some("default".into()),
                Some("mono1".into()),
                Some("pca-udp".into())
            ]
        );
        assert_eq!(
            h.free_name("switchboard-1a2b", "1a2b3c4d5e6f"),
            "switchboard-1a2b"
        );
        std::fs::create_dir_all(dir.path().join("sessions/switchboard-1a2b")).unwrap();
        assert_eq!(
            h.free_name("switchboard-1a2b", "1a2b3c4d5e6f"),
            "switchboard-1a2b-1a2b3c4d5e6f"
        );
        assert_eq!(
            h.client(Some("mono1")).socket(),
            dir.path().join("sessions/mono1/herdr.sock")
        );
        assert_eq!(h.client(None).socket(), dir.path().join("herdr.sock"));
        assert_eq!(Herdrs::single("/x.sock").known(), [None]);
    }
}
