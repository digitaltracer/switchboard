//! Every keystroke the runner sends goes through here, and only to Codex panes:
//! the "Open restricted" startup choice and the keystroke-deny fallback for a failed hook.
//! Claude panes have no key path at all, so a pending Claude approval can't be answered by keys.

use std::future::Future;
use std::time::Duration;

use tokio::time::Instant;

use super::hooks::HookState;
use crate::agents::codex::check_version;
use crate::agents::codex::screen::{Keys, Screen, classify, deny_keys, startup_keys};
use crate::cli::Agent;
use crate::herdr::Client;
use crate::herdr::types::ReadSource;

/// A Codex pane must stay blocked this long, with no hook activity, before the fallback reads it.
pub const GRACE: Duration = Duration::from_secs(5);
pub const CONFIRM_ATTEMPTS: u32 = 6;
pub const CONFIRM_INTERVAL: Duration = Duration::from_millis(500);
pub const STARTUP_POLL: Duration = Duration::from_millis(500);
const EXCERPT_LINES: usize = 20;

pub trait CodexIo {
    fn read_screen(&self) -> impl Future<Output = Result<String, String>> + Send;
    fn send(&self, keys: &Keys) -> impl Future<Output = Result<(), String>> + Send;
}

/// A Herdr agent target known to be a Codex session.
pub struct CodexPane<'a> {
    herdr: &'a Client,
    target: String,
}

impl<'a> CodexPane<'a> {
    /// `None` for any agent other than Codex.
    pub fn new(herdr: &'a Client, agent: Agent, target: &str) -> Option<Self> {
        (agent == Agent::Codex).then(|| Self {
            herdr,
            target: target.to_owned(),
        })
    }
}

impl CodexIo for CodexPane<'_> {
    async fn read_screen(&self) -> Result<String, String> {
        self.herdr
            .agent_read(&self.target, ReadSource::Visible, None)
            .await
            .map(|r| r.text)
            .map_err(|e| e.to_string())
    }

    async fn send(&self, keys: &Keys) -> Result<(), String> {
        let keys = keys.as_slice().iter().map(|k| (*k).to_owned()).collect();
        self.herdr
            .agent_send_keys(&self.target, keys)
            .await
            .map_err(|e| e.to_string())
    }
}

pub fn excerpt(screen: &str) -> String {
    let lines: Vec<&str> = screen.lines().filter(|l| !l.trim().is_empty()).collect();
    lines[lines.len().saturating_sub(EXCERPT_LINES)..].join("\n")
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StartupFailure {
    pub reason: &'static str,
    pub screen_excerpt: String,
}

/// Reads in a row that must show the composer after Enter before the prompt may be sent.
const READY_READS: u32 = 2;

/// Clears the restricted-mode "Folder access" screen and returns once the composer is showing.
/// The screen must be seen: it proves the untrusted override applied, and Codex can draw its
/// composer briefly before a startup modal. Enter is sent at most once, and only when the
/// adapter recognises "Open restricted" as selected.
pub async fn clear_startup<I: CodexIo>(
    io: &I,
    version: &str,
    timeout: Duration,
) -> Result<(), StartupFailure> {
    let fail = |reason, screen: &str| StartupFailure {
        reason,
        screen_excerpt: excerpt(screen),
    };
    let deadline = Instant::now() + timeout;
    let mut sent = false;
    let mut ready_reads = 0;
    let mut last = String::new();
    loop {
        last = match io.read_screen().await {
            Ok(screen) => screen,
            Err(_) => return Err(fail("screen_read_failed", &last)),
        };
        let screen = classify(version, &last);
        ready_reads = if sent && screen == Screen::Ready {
            ready_reads + 1
        } else {
            0
        };
        if ready_reads >= READY_READS {
            return Ok(());
        }
        match screen {
            s @ Screen::FolderAccess {
                open_restricted_selected: true,
            } if !sent => {
                let keys = startup_keys(s).expect("recognised startup screen has a key");
                io.send(&keys)
                    .await
                    .map_err(|_| fail("send_failed", &last))?;
                sent = true;
            }
            Screen::FolderAccess {
                open_restricted_selected: false,
            } => {
                return Err(fail("open_restricted_not_selected", &last));
            }
            Screen::HooksReview => return Err(fail("hooks_review_shown", &last)),
            Screen::PermissionDialog(_) => return Err(fail("dialog_at_startup", &last)),
            Screen::Ready | Screen::FolderAccess { .. } | Screen::Unknown => {}
        }
        if Instant::now() >= deadline {
            let reason = if sent {
                "startup_not_cleared"
            } else {
                "restricted_screen_not_seen"
            };
            return Err(fail(reason, &last));
        }
        tokio::time::sleep(STARTUP_POLL).await;
    }
}

/// What the runner knows about one session without reading its screen.
#[derive(Debug, Clone, Copy)]
pub struct Facts<'a> {
    pub agent: Agent,
    pub codex_version: Option<&'a str>,
    pub blocked_since: Option<Instant>,
    pub hooks: HookState,
    pub now: Instant,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Gate {
    /// All conditions hold; read the screen and judge it.
    ReadScreen,
    Wait(&'static str),
    /// Never act on this session.
    Refuse(&'static str),
    /// Surface as `hook_failed` without sending anything.
    HookFailed(&'static str),
}

pub fn gate(f: &Facts<'_>) -> Gate {
    if f.agent != Agent::Codex {
        return Gate::Refuse("not_codex");
    }
    if f.hooks.hook_failed {
        return Gate::Refuse("already_hook_failed");
    }
    let Some(since) = f.blocked_since else {
        return Gate::Wait("not_blocked");
    };
    if !f.codex_version.is_some_and(|v| check_version(v).is_ok()) {
        return Gate::HookFailed("unsupported_codex_version");
    }
    if f.hooks.pending > 0 {
        return Gate::Wait("approval_pending");
    }
    if f.hooks.open_calls > 0 {
        return Gate::Wait("hook_running");
    }
    if f.now.saturating_duration_since(since) < GRACE {
        return Gate::Wait("grace");
    }
    // A hook that delivered an answer around the block didn't fail; don't guess with keys.
    if f.hooks.last_answered.is_some_and(|t| t + GRACE >= since) {
        return Gate::HookFailed("hook_answered_but_dialog_shown");
    }
    Gate::ReadScreen
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FallbackOutcome {
    Denied {
        screen_excerpt: String,
    },
    /// The composer is showing, so Herdr's blocked status was stale; nothing was sent.
    NothingToDo,
    HookFailed {
        reason: &'static str,
        screen_excerpt: String,
    },
}

/// Sends the deny choice once if the adapter recognises a permission dialog, then confirms
/// from the screen that the dialog is gone. Never sends anything else.
pub async fn deny_dialog<I: CodexIo>(io: &I, version: &str) -> FallbackOutcome {
    let failed = |reason, screen: &str| FallbackOutcome::HookFailed {
        reason,
        screen_excerpt: excerpt(screen),
    };
    let Ok(before) = io.read_screen().await else {
        return failed("screen_read_failed", "");
    };
    let screen = classify(version, &before);
    if screen == Screen::Ready {
        return FallbackOutcome::NothingToDo;
    }
    let Some(keys) = deny_keys(screen) else {
        return failed("unrecognised_screen", &before);
    };
    if io.send(&keys).await.is_err() {
        return failed("send_failed", &before);
    }
    let mut after = before.clone();
    for _ in 0..CONFIRM_ATTEMPTS {
        tokio::time::sleep(CONFIRM_INTERVAL).await;
        let Ok(screen) = io.read_screen().await else {
            continue;
        };
        after = screen;
        if !matches!(classify(version, &after), Screen::PermissionDialog(_)) {
            return FallbackOutcome::Denied {
                screen_excerpt: excerpt(&before),
            };
        }
    }
    failed("dialog_still_present", &after)
}

#[cfg(test)]
#[path = "codex_tests.rs"]
mod tests;
