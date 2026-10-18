//! Version-pinned recogniser for the Codex screens Switchboard may answer with keys.
//! Built from Codex 0.163.0 captures in `tests/fixtures/codex-0.163.0/`. It only ever yields
//! the "Open restricted" startup key and the permission dialog's deny key; nothing approves.

use super::SUPPORTED_VERSIONS;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Screen {
    /// Restricted-mode startup prompt; esc on this screen quits Codex.
    FolderAccess {
        open_restricted_selected: bool,
    },
    /// The composer is showing: no modal is up.
    Ready,
    PermissionDialog(DialogKind),
    /// Must never appear with the bypass flag; treated as a failed startup.
    HooksReview,
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DialogKind {
    Command,
    Edits,
}

/// Keys the runner may send to a Codex pane. Only this module can construct them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Keys(&'static [&'static str]);

impl Keys {
    pub fn as_slice(&self) -> &'static [&'static str] {
        self.0
    }
}

const COMMAND_TITLE: &str = "Would you like to run the following command?";
const EDITS_TITLE: &str = "Would you like to make the following edits?";
const DIALOG_YES: &str = "1. Yes, proceed (y)";
const DIALOG_NO: &str = "3. No, and tell Codex what to do differently (esc)";
const DIALOG_FOOTER: &str = "Press enter to confirm or esc to cancel";
const FOLDER_TITLE: &str = "Folder access";
const FOLDER_BODY: &str = "Config, hooks, and exec policies from untrusted folders stay disabled.";
const FOLDER_OPEN: &str = "1. Open restricted";
const FOLDER_QUIT: &str = "2. Quit";
const FOLDER_FOOTER: &str = "enter continue · esc quit";
const READY_FOOTER: &str = "? for shortcuts";
const HOOKS_REVIEW: &str = "Hooks need review";
/// How far above the footer a dialog's title may sit.
const DIALOG_SPAN: usize = 30;

/// Classifies a Herdr `visible` read. Unsupported versions always classify as `Unknown`.
pub fn classify(version: &str, text: &str) -> Screen {
    if !SUPPORTED_VERSIONS.contains(&version) {
        return Screen::Unknown;
    }
    let lines: Vec<&str> = text
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .collect();
    let Some(&bottom) = lines.last() else {
        return Screen::Unknown;
    };
    if lines.contains(&HOOKS_REVIEW) {
        return Screen::HooksReview;
    }
    if bottom == DIALOG_FOOTER {
        return dialog(&lines);
    }
    if bottom == FOLDER_FOOTER {
        return folder_access(&lines);
    }
    if bottom.starts_with(READY_FOOTER) {
        return Screen::Ready;
    }
    Screen::Unknown
}

fn option(line: &str) -> (&str, bool) {
    match line.strip_prefix('›') {
        Some(rest) => (rest.trim_start(), true),
        None => (line, false),
    }
}

fn dialog(lines: &[&str]) -> Screen {
    let tail = &lines[lines.len().saturating_sub(DIALOG_SPAN)..];
    let Some(title_at) = tail
        .iter()
        .rposition(|l| *l == COMMAND_TITLE || *l == EDITS_TITLE)
    else {
        return Screen::Unknown;
    };
    let kind = if tail[title_at] == COMMAND_TITLE {
        DialogKind::Command
    } else {
        DialogKind::Edits
    };
    let body = &tail[title_at + 1..tail.len() - 1];
    let yes = body.iter().position(|l| option(l).0 == DIALOG_YES);
    let no = body.iter().position(|l| option(l).0 == DIALOG_NO);
    match (yes, no) {
        (Some(y), Some(n)) if y < n => Screen::PermissionDialog(kind),
        _ => Screen::Unknown,
    }
}

fn folder_access(lines: &[&str]) -> Screen {
    let Some(title_at) = lines.iter().rposition(|l| *l == FOLDER_TITLE) else {
        return Screen::Unknown;
    };
    let body = &lines[title_at + 1..lines.len() - 1];
    if !body.iter().any(|l| l.starts_with(FOLDER_BODY)) {
        return Screen::Unknown;
    }
    let open = body
        .iter()
        .map(|l| option(l))
        .find(|(t, _)| *t == FOLDER_OPEN);
    let quit = body.iter().any(|l| option(l).0 == FOLDER_QUIT);
    match open {
        Some((_, selected)) if quit => Screen::FolderAccess {
            open_restricted_selected: selected,
        },
        _ => Screen::Unknown,
    }
}

/// Enter, only while "Open restricted" is the selected option.
pub fn startup_keys(screen: Screen) -> Option<Keys> {
    match screen {
        Screen::FolderAccess {
            open_restricted_selected: true,
        } => Some(Keys(&["enter"])),
        _ => None,
    }
}

/// The dialog's "No" choice. There is deliberately no approving counterpart.
pub fn deny_keys(screen: Screen) -> Option<Keys> {
    match screen {
        Screen::PermissionDialog(_) => Some(Keys(&["esc"])),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const V: &str = "0.163.0";

    fn fixture(name: &str) -> String {
        let path = format!(
            "{}/tests/fixtures/codex-{V}/{name}.txt",
            env!("CARGO_MANIFEST_DIR")
        );
        std::fs::read_to_string(path).unwrap()
    }

    #[test]
    fn recognises_captured_screens() {
        assert_eq!(
            classify(V, &fixture("folder-access")),
            Screen::FolderAccess {
                open_restricted_selected: true
            }
        );
        assert_eq!(classify(V, &fixture("ready")), Screen::Ready);
        assert_eq!(classify(V, &fixture("turn-done")), Screen::Ready);
        assert_eq!(
            classify(V, &fixture("permission-command")),
            Screen::PermissionDialog(DialogKind::Command)
        );
        assert_eq!(
            classify(V, &fixture("permission-edit")),
            Screen::PermissionDialog(DialogKind::Edits)
        );
        assert_eq!(classify(V, &fixture("after-deny-command")), Screen::Ready);
        assert_eq!(classify(V, &fixture("after-deny-edit")), Screen::Ready);
    }

    #[test]
    fn keys_only_for_recognised_screens() {
        let folder = classify(V, &fixture("folder-access"));
        assert_eq!(startup_keys(folder).unwrap().as_slice(), ["enter"]);
        assert_eq!(deny_keys(folder), None);
        let dialog = classify(V, &fixture("permission-command"));
        assert_eq!(deny_keys(dialog).unwrap().as_slice(), ["esc"]);
        assert_eq!(startup_keys(dialog), None);
        for s in [Screen::Ready, Screen::Unknown, Screen::HooksReview] {
            assert_eq!((startup_keys(s), deny_keys(s)), (None, None));
        }
    }

    #[test]
    fn unsupported_version_recognises_nothing() {
        assert_eq!(
            classify("0.164.0", &fixture("permission-command")),
            Screen::Unknown
        );
        assert_eq!(classify("", &fixture("folder-access")), Screen::Unknown);
    }

    #[test]
    fn folder_access_with_quit_selected_gets_no_key() {
        let text = fixture("folder-access")
            .replace("› 1. Open restricted", "  1. Open restricted")
            .replace("  2. Quit", "› 2. Quit");
        let s = classify(V, &text);
        assert_eq!(
            s,
            Screen::FolderAccess {
                open_restricted_selected: false
            }
        );
        assert_eq!(startup_keys(s), None);
    }

    #[test]
    fn altered_dialogs_are_unknown() {
        let base = fixture("permission-command");
        // Dialog text echoed by the agent, with the composer below it.
        let echoed = format!("{base}\n› Ask Codex to do anything\n  ? for shortcuts\n");
        assert_eq!(classify(V, &echoed), Screen::Ready);
        let reworded = base.replace(DIALOG_NO, "3. No, deny (n)");
        assert_eq!(classify(V, &reworded), Screen::Unknown);
        let no_title = base.replace(COMMAND_TITLE, "Something else?");
        assert_eq!(classify(V, &no_title), Screen::Unknown);
        let trailing = format!("{base}\n  extra line\n");
        assert_eq!(classify(V, &trailing), Screen::Unknown);
        assert_eq!(classify(V, ""), Screen::Unknown);
    }

    #[test]
    fn hooks_review_is_flagged() {
        let text = "  Hooks need review\n  5 hooks are new or changed.\n› 1. Review hooks\n  enter confirm · esc skip\n";
        assert_eq!(classify(V, text), Screen::HooksReview);
    }
}
