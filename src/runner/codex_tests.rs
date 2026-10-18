use std::collections::VecDeque;
use std::sync::Mutex;

use super::*;

const V: &str = "0.163.0";
const STARTUP: Duration = Duration::from_secs(20);

fn fixture(name: &str) -> String {
    std::fs::read_to_string(format!(
        "{}/tests/fixtures/codex-{V}/{name}.txt",
        env!("CARGO_MANIFEST_DIR")
    ))
    .unwrap()
}

/// Plays screens in order (repeating the last) and records every key sent.
struct FakeIo {
    screens: Mutex<VecDeque<String>>,
    sent: Mutex<Vec<Vec<&'static str>>>,
}

impl FakeIo {
    fn new(screens: &[String]) -> Self {
        Self {
            screens: Mutex::new(screens.iter().cloned().collect()),
            sent: Mutex::default(),
        }
    }

    fn sent(&self) -> Vec<Vec<&'static str>> {
        self.sent.lock().unwrap().clone()
    }
}

impl CodexIo for FakeIo {
    async fn read_screen(&self) -> Result<String, String> {
        let mut q = self.screens.lock().unwrap();
        if q.len() > 1 {
            Ok(q.pop_front().unwrap())
        } else {
            Ok(q.front().cloned().unwrap_or_default())
        }
    }

    async fn send(&self, keys: &Keys) -> Result<(), String> {
        self.sent.lock().unwrap().push(keys.as_slice().to_vec());
        Ok(())
    }
}

fn facts(now: Instant) -> Facts<'static> {
    Facts {
        agent: Agent::Codex,
        codex_version: Some(V),
        blocked_since: Some(now - GRACE - Duration::from_secs(1)),
        hooks: HookState {
            live: true,
            ..Default::default()
        },
        now,
    }
}

#[tokio::test(start_paused = true)]
async fn gate_allows_reading_only_when_every_condition_holds() {
    let now = Instant::now() + Duration::from_secs(60);
    assert_eq!(gate(&facts(now)), Gate::ReadScreen);
}

#[tokio::test(start_paused = true)]
async fn refuses_claude_pane() {
    let now = Instant::now() + Duration::from_secs(60);
    assert_eq!(
        gate(&Facts {
            agent: Agent::Claude,
            ..facts(now)
        }),
        Gate::Refuse("not_codex")
    );
    let herdr = Client::new("/nonexistent.sock");
    assert!(CodexPane::new(&herdr, Agent::Claude, "w2:p1").is_none());
    assert!(CodexPane::new(&herdr, Agent::Codex, "w2:p1").is_some());
}

#[tokio::test(start_paused = true)]
async fn refuses_while_an_approval_is_pending() {
    let now = Instant::now() + Duration::from_secs(60);
    let f = Facts {
        hooks: HookState {
            pending: 1,
            ..facts(now).hooks
        },
        ..facts(now)
    };
    assert_eq!(gate(&f), Gate::Wait("approval_pending"));
}

#[tokio::test(start_paused = true)]
async fn refuses_unknown_codex_version() {
    let now = Instant::now() + Duration::from_secs(60);
    for v in [Some("0.164.0"), None] {
        assert_eq!(
            gate(&Facts {
                codex_version: v,
                ..facts(now)
            }),
            Gate::HookFailed("unsupported_codex_version")
        );
    }
    // The adapter itself recognises nothing on an unknown version, so no key is sent.
    let io = FakeIo::new(&[fixture("permission-command")]);
    let out = deny_dialog(&io, "0.164.0").await;
    assert!(matches!(
        out,
        FallbackOutcome::HookFailed {
            reason: "unrecognised_screen",
            ..
        }
    ));
    assert!(io.sent().is_empty());
}

#[tokio::test(start_paused = true)]
async fn waits_for_running_hook_and_grace_and_stops_after_failure() {
    let now = Instant::now() + Duration::from_secs(60);
    let base = facts(now);
    let running = Facts {
        hooks: HookState {
            open_calls: 1,
            ..base.hooks
        },
        ..base
    };
    assert_eq!(gate(&running), Gate::Wait("hook_running"));
    let fresh = Facts {
        blocked_since: Some(now - Duration::from_secs(1)),
        ..base
    };
    assert_eq!(gate(&fresh), Gate::Wait("grace"));
    assert_eq!(
        gate(&Facts {
            blocked_since: None,
            ..base
        }),
        Gate::Wait("not_blocked")
    );
    let answered = Facts {
        hooks: HookState {
            last_answered: base.blocked_since,
            ..base.hooks
        },
        ..base
    };
    assert_eq!(
        gate(&answered),
        Gate::HookFailed("hook_answered_but_dialog_shown")
    );
    let failed = Facts {
        hooks: HookState {
            hook_failed: true,
            ..base.hooks
        },
        ..base
    };
    assert_eq!(gate(&failed), Gate::Refuse("already_hook_failed"));
}

#[tokio::test(start_paused = true)]
async fn unrecognised_screen_sends_nothing() {
    let io = FakeIo::new(&["  something we have never seen\n  continue? [y/n]\n".to_owned()]);
    let out = deny_dialog(&io, V).await;
    let FallbackOutcome::HookFailed {
        reason,
        screen_excerpt,
    } = out
    else {
        panic!("{out:?}")
    };
    assert_eq!(reason, "unrecognised_screen");
    assert!(screen_excerpt.contains("continue? [y/n]"));
    assert!(io.sent().is_empty());
}

#[tokio::test(start_paused = true)]
async fn stale_blocked_status_with_composer_showing_sends_nothing() {
    let io = FakeIo::new(&[fixture("ready")]);
    assert_eq!(deny_dialog(&io, V).await, FallbackOutcome::NothingToDo);
    assert!(io.sent().is_empty());
}

#[tokio::test(start_paused = true)]
async fn dialog_still_present_after_deny_sends_nothing_more() {
    let io = FakeIo::new(&[fixture("permission-command")]);
    let out = deny_dialog(&io, V).await;
    assert!(
        matches!(
            out,
            FallbackOutcome::HookFailed {
                reason: "dialog_still_present",
                ..
            }
        ),
        "{out:?}"
    );
    assert_eq!(io.sent(), vec![vec!["esc"]], "exactly one deny, no retries");
}

#[tokio::test(start_paused = true)]
async fn recognised_dialog_is_denied_once_and_confirmed_gone() {
    for (dialog, after) in [
        ("permission-command", "after-deny-command"),
        ("permission-edit", "after-deny-edit"),
    ] {
        let io = FakeIo::new(&[fixture(dialog), fixture(dialog), fixture(after)]);
        let out = deny_dialog(&io, V).await;
        let FallbackOutcome::Denied { screen_excerpt } = out else {
            panic!("{out:?}")
        };
        assert!(screen_excerpt.contains("3. No, and tell Codex what to do differently (esc)"));
        assert_eq!(io.sent(), vec![vec!["esc"]]);
    }
}

#[tokio::test(start_paused = true)]
async fn startup_presses_enter_once_on_recognised_folder_access() {
    let io = FakeIo::new(&[
        String::new(),
        fixture("folder-access"),
        fixture("folder-access"),
        fixture("ready"),
    ]);
    clear_startup(&io, V, STARTUP).await.unwrap();
    assert_eq!(io.sent(), vec![vec!["enter"]]);
}

#[tokio::test(start_paused = true)]
async fn startup_refusals_send_nothing() {
    let quit_selected = fixture("folder-access")
        .replace("› 1. Open restricted", "  1. Open restricted")
        .replace("  2. Quit", "› 2. Quit");
    let hooks_review =
        "  Hooks need review\n› 1. Review hooks\n  enter confirm · esc skip\n".to_owned();
    let cases = [
        (quit_selected, "open_restricted_not_selected"),
        (hooks_review, "hooks_review_shown"),
        (fixture("permission-command"), "dialog_at_startup"),
        ("  loading…\n".to_owned(), "restricted_screen_not_seen"),
        (fixture("folder-trust"), "restricted_screen_not_seen"),
    ];
    for (screen, reason) in cases {
        let io = FakeIo::new(&[screen]);
        let err = clear_startup(&io, V, STARTUP).await.unwrap_err();
        assert_eq!(err.reason, reason);
        assert!(io.sent().is_empty(), "{reason}");
    }
    // Unknown version: the startup screen is not recognised, so Enter is never sent.
    let io = FakeIo::new(&[fixture("folder-access")]);
    assert_eq!(
        clear_startup(&io, "9.9.9", STARTUP)
            .await
            .unwrap_err()
            .reason,
        "restricted_screen_not_seen"
    );
    assert!(io.sent().is_empty());
}

#[tokio::test(start_paused = true)]
async fn composer_before_the_restricted_screen_is_not_ready() {
    // Live run: Codex drew its composer, then a startup modal the override should have prevented.
    let io = FakeIo::new(&[fixture("ready"), fixture("folder-trust")]);
    let err = clear_startup(&io, V, STARTUP).await.unwrap_err();
    assert_eq!(err.reason, "restricted_screen_not_seen");
    assert!(err.screen_excerpt.contains("Trust this folder?"));
    assert!(
        io.sent().is_empty(),
        "never presses Enter on the trust screen"
    );
    let only_ready = FakeIo::new(&[fixture("ready")]);
    assert_eq!(
        clear_startup(&only_ready, V, STARTUP)
            .await
            .unwrap_err()
            .reason,
        "restricted_screen_not_seen"
    );
}

#[tokio::test(start_paused = true)]
async fn a_single_composer_read_after_enter_is_not_enough() {
    let io = FakeIo::new(&[
        fixture("folder-access"),
        fixture("ready"),
        fixture("folder-access"),
    ]);
    let err = clear_startup(&io, V, STARTUP).await.unwrap_err();
    assert_eq!(err.reason, "startup_not_cleared");
    assert_eq!(io.sent(), vec![vec!["enter"]], "Enter is sent once");
}

#[test]
fn only_this_module_sends_keys() {
    let dir = concat!(env!("CARGO_MANIFEST_DIR"), "/src");
    let mut offenders = Vec::new();
    let mut stack = vec![std::path::PathBuf::from(dir)];
    while let Some(p) = stack.pop() {
        for entry in std::fs::read_dir(&p).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                stack.push(path);
            } else if path.extension().is_some_and(|e| e == "rs") {
                let text = std::fs::read_to_string(&path).unwrap();
                let calls = text.matches(concat!(".agent_send", "_keys(")).count();
                let rel = path
                    .strip_prefix(dir)
                    .unwrap()
                    .to_string_lossy()
                    .into_owned();
                if calls > 0 && rel != "runner/codex.rs" && !rel.starts_with("herdr/") {
                    offenders.push(rel);
                }
            }
        }
    }
    assert!(
        offenders.is_empty(),
        "keys sent outside runner/codex.rs: {offenders:?}"
    );
}
