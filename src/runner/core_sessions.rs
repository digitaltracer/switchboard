//! Sessions started in herdr-core, behind the Herdr app: a git worktree where the app keeps its
//! own, and a terminal the app adopts as a new space and tab. Hooks, gates and the first prompt
//! are the same as in Herdr.

use std::collections::BTreeMap;

use super::*;
use crate::config::NewSessions;
use crate::hc::layout::{self, LayoutError};
use crate::hc::types::{LAYOUT_DOC, RunParams, spaces};
use crate::proto::{BackendOption, StartBackend, StartOptions};

/// The label that ties a core terminal to its session.
pub const SESSION_LABEL: &str = "switchboard.session";
/// The launcher's `$0`, which tells the listing to look past it for the agent.
pub const LAUNCHER: &str = "sb-launch";
/// Drops what the core's environment may carry from a Herdr pane or a Claude session, then runs
/// the agent in its place.
const PRELUDE: &str = r#"for v in $(env | cut -d= -f1); do case "$v" in HERDR_*|CLAUDE_CODE*|CLAUDECODE) unset "$v";; esac; done; exec "$@""#;
const ADOPT_WAIT: Duration = Duration::from_secs(60);
const ADOPT_POLL: Duration = Duration::from_secs(2);
const RENAME_TRIES: u32 = 5;
/// How long `start_options` waits for the core's `hello`.
const OPTIONS_WAIT: Duration = Duration::from_secs(2);

/// How this runner starts sessions on the core.
#[derive(Debug, Clone)]
pub struct CoreStarts {
    pub new_sessions: NewSessions,
    /// The app's own worktree folder, `~/.herdr/worktrees`.
    pub worktrees: PathBuf,
    /// Where agent binaries are looked for after `PATH`.
    pub home: PathBuf,
    /// A folder holding `claude` and `codex`, instead of looking (tests).
    pub bins: Option<PathBuf>,
}

pub(super) fn is_core(s: &RunnerSession) -> bool {
    s.herdr_session.as_deref() == Some(core_watch::SERVER)
}

impl Sessions {
    pub fn set_core_starts(&self, starts: CoreStarts) {
        let _ = self.core_starts.set(starts);
    }

    /// The core a new session starts on, or none for Herdr. `chosen` overrides `new_sessions`;
    /// a chosen Herdr app that doesn't answer fails, never falling back.
    pub(super) async fn choose_core(
        &self,
        chosen: Option<StartBackend>,
    ) -> Result<Option<Arc<Core>>, ProtoError> {
        let mode = match chosen {
            Some(StartBackend::Herdr) => NewSessions::Herdr,
            Some(StartBackend::Hc) => NewSessions::Hc,
            None => self.new_sessions(),
        };
        if mode == NewSessions::Herdr {
            return Ok(None);
        }
        let answer = self.core_answers().await;
        match (mode, answer) {
            (_, Ok(core)) => {
                tracing::info!(?mode, "starting the session in the Herdr app");
                Ok(Some(core))
            }
            (NewSessions::Hc, Err(e)) if chosen.is_some() => {
                tracing::info!(error = %e, "the chosen Herdr app isn't answering");
                Err(err(
                    ErrorCode::BackendUnavailable,
                    "the Herdr app isn't answering on this machine; open it there, or start in the Herdr terminal",
                ))
            }
            (NewSessions::Hc, Err(e)) => Err(err(
                ErrorCode::Herdr,
                format!(
                    "new_sessions is \"hc\" and herdr-core isn't answering ({e}); start the Herdr app"
                ),
            )),
            (_, Err(e)) => {
                tracing::info!(error = %e, "herdr-core isn't answering; starting the session in Herdr");
                Ok(None)
            }
        }
    }

    fn new_sessions(&self) -> NewSessions {
        self.core_starts
            .get()
            .map_or(NewSessions::Herdr, |s| s.new_sessions)
    }

    async fn core_answers(&self) -> Result<Arc<Core>, crate::hc::HcError> {
        match self.core() {
            Some(core) => core.client().hello().await.map(|_| core),
            None => Err(crate::hc::HcError::InvalidArgument(
                "this runner follows no herdr-core".into(),
            )),
        }
    }

    /// Where a start with no backend would go, and whether each backend can take one now.
    pub async fn start_options(&self) -> StartOptions {
        let core = match self.core() {
            None => Err("not_followed"),
            Some(core) => match tokio::time::timeout(OPTIONS_WAIT, core.client().hello()).await {
                Ok(Ok(_)) => Ok(()),
                _ => Err("not_answering"),
            },
        };
        let default = match (self.new_sessions(), &core) {
            (NewSessions::Herdr, _) | (NewSessions::Auto, Err(_)) => StartBackend::Herdr,
            (NewSessions::Hc, _) | (NewSessions::Auto, Ok(())) => StartBackend::Hc,
        };
        StartOptions {
            default,
            backends: vec![
                BackendOption {
                    backend: StartBackend::Hc,
                    available: core.is_ok(),
                    reason: core.err().map(str::to_owned),
                },
                BackendOption {
                    backend: StartBackend::Herdr,
                    available: true,
                    reason: None,
                },
            ],
        }
    }

    /// A start into an existing space needs the Herdr app, and the space in its layout on this
    /// machine now. Checked before anything is created.
    pub(super) async fn check_space(
        &self,
        core: Option<&Core>,
        space: &str,
    ) -> Result<(), ProtoError> {
        let Some(core) = core else {
            return Err(err(
                ErrorCode::BadRequest,
                "a session can only join a space in the Herdr app",
            ));
        };
        let doc = core
            .client()
            .doc_get(LAYOUT_DOC)
            .await
            .map_err(|e| err(ErrorCode::Herdr, format!("herdr-core: {e}")))?;
        if core_watch::local_space(&spaces(&doc.value), space).is_none() {
            return Err(err(
                ErrorCode::NotFound,
                "that space is no longer in the Herdr app on this machine",
            ));
        }
        Ok(())
    }

    fn agent_bin(&self, agent: Agent) -> Result<PathBuf, ProtoError> {
        let starts = self.core_starts.get();
        let name = agent.as_str();
        let found = match starts.and_then(|s| s.bins.as_ref()) {
            Some(dir) => Some(dir.join(name)),
            None => starts.and_then(|s| super::super::find_bin(name, &s.home)),
        };
        found.ok_or_else(|| {
            err(
                ErrorCode::StartupFailed,
                format!("{name} is not installed, or not on PATH, ~/.local/bin or Homebrew's bin"),
            )
        })
    }

    /// The rest of `start` for a session on the core. The secret, hook and arguments are ready.
    /// With `into`, its tab opens in that space; else in a new space of its own.
    pub(super) async fn start_on_core(
        &self,
        mut session: RunnerSession,
        core: Arc<Core>,
        mut args: Vec<String>,
        prompt: String,
        into: Option<String>,
    ) -> Result<Value, ProtoError> {
        let client = core.client();
        let core_id = match client.hello().await {
            Ok(h) => h.core.id,
            Err(e) => {
                let e = err(ErrorCode::Herdr, format!("herdr-core: {e}"));
                self.abort(&session, &e).await;
                return Err(e);
            }
        };
        session.herdr_session = Some(core_watch::SERVER.into());
        session.core_id = Some(core_id.clone());
        self.save(&session)
            .map_err(|e| err(ErrorCode::Internal, format!("saving session: {e}")))?;

        let bin = match self.agent_bin(session.agent) {
            Ok(b) => b,
            Err(e) => {
                self.abort(&session, &e).await;
                return Err(e);
            }
        };
        let worktree = match self.add_worktree(&session).await {
            Ok(w) => w,
            Err(e) => {
                self.abort(&session, &e).await;
                return Err(e);
            }
        };
        session.worktree_path = Some(worktree.to_string_lossy().into_owned());
        session.start_commit = head_commit(&worktree).await;
        self.save(&session)
            .map_err(|e| err(ErrorCode::Internal, format!("saving session: {e}")))?;

        if session.agent == Agent::Codex {
            // Else its commands and hooks run in a shared server outside the terminal.
            args.insert(0, "--no-daemon".into());
        }
        // In a space that exists the app makes the tab itself, as its manual says.
        let (space, tab) = match &into {
            Some(space) => (space.clone(), None),
            None => (
                format!("sp{}", super::super::random_id()),
                Some(format!("tb{}", super::super::random_id())),
            ),
        };
        let mut argv = vec![
            "/bin/sh".to_owned(),
            "-c".into(),
            PRELUDE.into(),
            LAUNCHER.into(),
            bin.to_string_lossy().into_owned(),
        ];
        argv.extend(args);
        let mut labels = BTreeMap::from([
            ("herdr.space".to_owned(), space.clone()),
            ("herdr.seen".to_owned(), "0".to_owned()),
            (SESSION_LABEL.to_owned(), session.session_id.clone()),
        ]);
        if let Some(tab) = &tab {
            labels.insert("herdr.tab".to_owned(), tab.clone());
        }
        session.workspace_id = Some(space.clone());
        let run = RunParams {
            argv,
            cwd: worktree.to_string_lossy().into_owned(),
            name: session.agent_name.clone(),
            labels,
            keep: true,
            no_parent: true,
        };
        let t = match client.run(&run).await {
            Ok(t) => t,
            Err(e) => {
                // A run that may have happened is found again by its label.
                let e = err(ErrorCode::StartupFailed, format!("starting the agent: {e}"));
                self.abort(&session, &e).await;
                return Err(e);
            }
        };
        session.pane_id = Some(t.address.clone());
        session.terminal_identity = Some(t.identity(&core_id));
        self.save(&session)
            .map_err(|e| err(ErrorCode::Internal, format!("saving session: {e}")))?;

        if let Err(e) = self.ready_on_core(&session, &core).await {
            self.abort(&session, &e).await;
            return Err(e);
        }
        session.launched = true;
        self.save(&session)
            .map_err(|e| err(ErrorCode::Internal, format!("saving session: {e}")))?;
        if let Err(e) = self
            .server_for(&session)
            .prompt(Self::target_for(&session), &prompt)
            .await
        {
            let e = backend_err(e);
            if e.code != ErrorCode::OutcomeUnknown {
                self.abort(&session, &e).await;
            }
            return Err(e);
        }
        self.audit(&session.session_id, "prompt", json!({ "initial": true }));
        let tab_name = crate::hub::route::label(&session.project_id, &session.session_id, None);
        tokio::spawn(name_tab_and_space(
            client,
            t.address.clone(),
            tab,
            tab_name,
            into.is_none().then(|| (space, session.project_id.clone())),
        ));
        Ok(started_json(&session))
    }

    /// `git worktree add -b <branch>` under the app's worktree folder, as the app makes its own.
    async fn add_worktree(&self, s: &RunnerSession) -> Result<PathBuf, ProtoError> {
        let root = self
            .core_starts
            .get()
            .map(|c| c.worktrees.clone())
            .ok_or_else(|| err(ErrorCode::Internal, "no worktree folder"))?;
        let repo = s
            .repo_path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| "repo".into());
        let path = root.join(repo).join(s.branch.replace('/', "-"));
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|e| err(ErrorCode::Internal, format!("worktree folder: {e}")))?;
        }
        let out = git(
            &s.repo_path,
            &["worktree", "add", "-b", &s.branch, &path.to_string_lossy()],
        )
        .await
        .map_err(|e| err(ErrorCode::Internal, e))?;
        if !out.status.success() {
            return Err(err(
                ErrorCode::Internal,
                format!(
                    "git worktree add: {}",
                    String::from_utf8_lossy(&out.stderr).trim()
                ),
            ));
        }
        Ok(path)
    }

    /// Claude once its SessionStart hook is live and it reports idle; Codex once its restricted
    /// startup is cleared and it is at its composer.
    async fn ready_on_core(&self, s: &RunnerSession, core: &Arc<Core>) -> Result<(), ProtoError> {
        match s.agent {
            Agent::Claude => {
                let deadline = Instant::now() + self.cfg.liveness_timeout;
                while !self.hooks.state(&s.session_id).is_some_and(|h| h.live) {
                    if Instant::now() >= deadline {
                        return Err(err(
                            ErrorCode::HooksUntrusted,
                            "the SessionStart hook never reached the runner",
                        ));
                    }
                    tokio::time::sleep(Duration::from_millis(200)).await;
                }
            }
            Agent::Codex => {
                let pane = CodexPane::on(Server::Core(core.clone()), s.agent, &s.agent_name)
                    .expect("codex session");
                let pane_id = s.pane_id.as_deref().unwrap_or_default();
                let _lock = self
                    .pane_locks
                    .lock(Some(core_watch::SERVER), pane_id)
                    .await;
                clear_startup(&pane, self.cfg.startup_timeout)
                    .await
                    .map_err(|f| {
                        self.audit(
                            &s.session_id,
                            "startup_failed",
                            json!({ "reason": f.reason, "screen_excerpt": f.screen_excerpt }),
                        );
                        err(
                            ErrorCode::StartupFailed,
                            format!("{}; screen:\n{}", f.reason, f.screen_excerpt),
                        )
                    })?;
                self.audit(
                    &s.session_id,
                    "startup_cleared",
                    json!({ "screen": "folder_access" }),
                );
            }
        }
        self.wait_interactive(s).await
    }

    /// Ends a core session's terminal: only one carrying its label, on the core it started on.
    /// False unless every such terminal was ended.
    pub(super) async fn end_core_terminal(&self, s: &RunnerSession) -> bool {
        let Some(core) = self.core() else {
            return false;
        };
        let client = core.client();
        match client.hello().await {
            Ok(h) if Some(&h.core.id) == s.core_id.as_ref() => {}
            Ok(_) => {
                tracing::warn!(session_id = %s.session_id, "a different herdr-core answers; its terminals are left alone");
                return false;
            }
            Err(e) => {
                tracing::warn!(session_id = %s.session_id, error = %e, "herdr-core isn't answering; the terminal is left");
                return false;
            }
        }
        let label = BTreeMap::from([(SESSION_LABEL.to_owned(), s.session_id.clone())]);
        let terminals = match client.list_labelled(&label).await {
            Ok(ts) => ts,
            Err(e) => {
                tracing::warn!(session_id = %s.session_id, error = %e, "listing the session's terminals");
                return false;
            }
        };
        let mut ended = true;
        for t in terminals.iter().filter(|t| t.is_local()) {
            if t.is_live()
                && let Err(e) = client.kill(&t.address).await
            {
                tracing::warn!(address = %t.address, error = %e, "ending the session's terminal");
                ended = false;
                continue;
            }
            if let Err(e) = client.rm(&t.address).await {
                tracing::warn!(address = %t.address, error = %e, "removing the session's terminal");
            }
        }
        ended
    }

    /// Stops a core session: its terminal ends, and its worktree goes only if clean and merged
    /// and the terminal was ended.
    pub(super) async fn stop_core(
        &self,
        s: &RunnerSession,
        remove_if_clean: bool,
    ) -> (bool, Option<String>) {
        let ended = self.end_core_terminal(s).await;
        match (&s.worktree_path, remove_if_clean) {
            (Some(_), true) if !ended => {
                (false, Some("the agent's terminal couldn't be ended".into()))
            }
            (Some(path), true) => {
                remove_stopped_worktree(&s.repo_path, Path::new(path), &s.branch).await
            }
            _ => (false, None),
        }
    }
}

/// Names the session's tab after it, and a new space after the project, once the app has
/// adopted the terminal: into the tab its label names, or one the app made in an existing space.
/// Without the app running, they stay unnamed.
async fn name_tab_and_space(
    client: crate::hc::Client,
    address: String,
    tab: Option<String>,
    tab_name: String,
    new_space: Option<(String, String)>,
) {
    let deadline = Instant::now() + ADOPT_WAIT;
    let tab = loop {
        let adopted = client.doc_get(LAYOUT_DOC).await.ok().and_then(|d| {
            let all = spaces(&d.value);
            match &tab {
                Some(id) => all
                    .iter()
                    .flat_map(|s| &s.tabs)
                    .any(|t| &t.id == id)
                    .then(|| id.clone()),
                None => super::super::manage::tab_holding(&all, &address, None),
            }
        });
        if let Some(id) = adopted {
            break id;
        }
        if Instant::now() >= deadline {
            tracing::debug!(
                address,
                "the Herdr app didn't adopt the terminal; its tab stays unnamed"
            );
            return;
        }
        tokio::time::sleep(ADOPT_POLL).await;
    };
    let mut names = vec![(layout::Which::Tab(&tab), &tab_name)];
    if let Some((space, name)) = &new_space {
        names.push((layout::Which::Space(space), name));
    }
    for (which, name) in names {
        for _ in 0..RENAME_TRIES {
            match layout::rename(&client, which, name).await {
                Err(LayoutError::Conflict) => continue,
                Ok(()) => break,
                Err(e) => {
                    tracing::debug!(error = %e, "naming the session's tab or space");
                    break;
                }
            }
        }
    }
}
