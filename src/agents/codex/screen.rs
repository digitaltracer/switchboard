//! Recogniser for the Codex screens Switchboard may answer with keys, by their layout. Built from
//! the Codex 0.163.0 captures in `tests/fixtures/codex-0.163.0/` and the 0.165.0 ones in
//! `tests/fixtures/dialogs/codex-0.165.0/`, whose layout is the same; they stay as its tests. `classify` only ever
//! yields the "Open restricted" startup key and the permission dialog's deny key; `recognise`
//! lists the options of a permission dialog, or of one question of Plan mode's question dialog
//! (the 0.169.0 captures in `tests/fixtures/dialogs/core/codex-0.169.0/`), for the owner's
//! signed choice.

use crate::agents::dialog::{self, Dialog, DialogOption, DialogStep, Recognised};

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

/// Classifies a Herdr `visible` read; a layout this doesn't know is `Unknown`.
pub fn classify(text: &str) -> Screen {
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

const MARKER: char = '›';
/// `Question 2/3 (2 unanswered)`, above the question's text.
const QUESTION_TITLE: &str = "Question ";
/// The last question of a group says "submit all": its Enter sends every answer. Enter under the
/// second footer only moves to the next question.
const QUESTION_FOOTERS: &[&str] = &[
    "tab to add notes | enter to submit answer | esc to interrupt",
    "tab to add notes | enter to submit answer | ←/→ to navigate questions | esc to interrupt",
    "tab to add notes | enter to submit all | ←/→ to navigate questions | esc to interrupt",
];
/// The row Codex adds for an answer typed as notes.
const QUESTION_OTHER: &str = "None of the above";

/// A permission dialog or one question as options, for an answer by keys; row 2's standing
/// approval included. Anything else is only parsed read-only.
pub fn recognise(text: &str) -> Recognised {
    exact(text)
        .or_else(|| question(text))
        .unwrap_or_else(|| dialog::generic(text, MARKER))
}

/// Which question of how many a title line names.
fn question_title(line: &str) -> Option<(u32, u32)> {
    let rest = line.strip_prefix(QUESTION_TITLE)?;
    let (count, rest) = rest.split_once(' ').unwrap_or((rest, ""));
    let (n, of) = count.split_once('/')?;
    let (n, of): (u32, u32) = (n.parse().ok()?, of.parse().ok()?);
    let counted = rest.is_empty() || (rest.starts_with('(') && rest.ends_with(" unanswered)"));
    (counted && n >= 1 && n <= of).then_some((n, of))
}

/// One question of Plan mode's question dialog. Enter on a row answers it and moves to the
/// next question, or submits them all from the last.
fn question(text: &str) -> Option<Recognised> {
    let lines = dialog::lines(text);
    let footer = lines.len().checked_sub(1)?;
    if !QUESTION_FOOTERS.contains(&lines[footer]) {
        return None;
    }
    dialog::row(lines[footer.checked_sub(1)?], MARKER)?;
    let r = dialog::rows(&lines, footer, MARKER)?;
    let floor = r.first.saturating_sub(DIALOG_SPAN);
    let title_at = (floor..r.first)
        .rev()
        .find(|&i| question_title(lines[i]).is_some())?;
    let (n, of) = question_title(lines[title_at])?;
    let ask = lines.get(title_at + 1..r.first).filter(|a| !a.is_empty())?;
    // In a group the title says how far along it is and how many are still unanswered.
    let prompt = if of > 1 {
        lines[title_at..r.first].join("\n")
    } else {
        ask.join("\n")
    };
    let options = r
        .rows
        .iter()
        .map(|&(number, row)| {
            // A row's description follows its label on the same line.
            let label = row.split("  ").next().unwrap_or(row);
            DialogOption {
                number,
                label: label.to_owned(),
                is_deny: false,
                excluded: label == QUESTION_OTHER,
                provisional: lines[footer] == QUESTION_FOOTERS[1] && label != QUESTION_OTHER,
            }
        })
        .collect();
    Some(Recognised::Dialog(Dialog {
        kind: dialog::DialogKind::Question,
        prompt,
        options,
        selected: r.selected,
        // Without the unanswered count, which moves with the selection on an answered question.
        region: format!("Question {n}/{of}\n{}", lines[title_at + 1..].join("\n")),
        actionable: true,
        // Codex says which question is open, but not which of the others are answered.
        steps: (1..=of)
            .filter(|_| of > 1)
            .map(|i| DialogStep {
                label: i.to_string(),
                answered: false,
                current: i == n,
            })
            .collect(),
    }))
}

fn exact(text: &str) -> Option<Recognised> {
    let lines = dialog::lines(text);
    let footer = lines.len().checked_sub(1)?;
    if lines[footer] != DIALOG_FOOTER || lines.contains(&HOOKS_REVIEW) {
        return None;
    }
    let r = dialog::rows(&lines, footer, MARKER)?;
    let labels: Vec<&str> = r.rows.iter().map(|r| r.1).collect();
    let [yes, _, no] = labels[..] else {
        return None;
    };
    if format!("1. {yes}") != DIALOG_YES || format!("3. {no}") != DIALOG_NO {
        return None;
    }
    let floor = r.first.saturating_sub(DIALOG_SPAN);
    let title_at = (floor..r.first)
        .rev()
        .find(|&i| lines[i] == COMMAND_TITLE || lines[i] == EDITS_TITLE)?;
    let options = r
        .rows
        .iter()
        .map(|&(number, label)| DialogOption {
            number,
            label: label.to_owned(),
            is_deny: number == 3,
            excluded: false,
            provisional: false,
        })
        .collect();
    Some(Recognised::Dialog(Dialog {
        kind: dialog::DialogKind::Permission,
        prompt: lines[title_at..r.first].join("\n"),
        options,
        selected: r.selected,
        region: lines[title_at..].join("\n"),
        actionable: true,
        steps: Vec::new(),
    }))
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
            classify(&fixture("folder-access")),
            Screen::FolderAccess {
                open_restricted_selected: true
            }
        );
        assert_eq!(classify(&fixture("ready")), Screen::Ready);
        assert_eq!(classify(&fixture("turn-done")), Screen::Ready);
        assert_eq!(
            classify(&fixture("permission-command")),
            Screen::PermissionDialog(DialogKind::Command)
        );
        assert_eq!(
            classify(&fixture("permission-edit")),
            Screen::PermissionDialog(DialogKind::Edits)
        );
        assert_eq!(classify(&fixture("after-deny-command")), Screen::Ready);
        assert_eq!(classify(&fixture("after-deny-edit")), Screen::Ready);
    }

    #[test]
    fn keys_only_for_recognised_screens() {
        let folder = classify(&fixture("folder-access"));
        assert_eq!(startup_keys(folder).unwrap().as_slice(), ["enter"]);
        assert_eq!(deny_keys(folder), None);
        let dialog = classify(&fixture("permission-command"));
        assert_eq!(deny_keys(dialog).unwrap().as_slice(), ["esc"]);
        assert_eq!(startup_keys(dialog), None);
        for s in [Screen::Ready, Screen::Unknown, Screen::HooksReview] {
            assert_eq!((startup_keys(s), deny_keys(s)), (None, None));
        }
    }

    #[test]
    fn folder_access_with_quit_selected_gets_no_key() {
        let text = fixture("folder-access")
            .replace("› 1. Open restricted", "  1. Open restricted")
            .replace("  2. Quit", "› 2. Quit");
        let s = classify(&text);
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
        assert_eq!(classify(&echoed), Screen::Ready);
        let reworded = base.replace(DIALOG_NO, "3. No, deny (n)");
        assert_eq!(classify(&reworded), Screen::Unknown);
        let no_title = base.replace(COMMAND_TITLE, "Something else?");
        assert_eq!(classify(&no_title), Screen::Unknown);
        let trailing = format!("{base}\n  extra line\n");
        assert_eq!(classify(&trailing), Screen::Unknown);
        assert_eq!(classify(""), Screen::Unknown);
    }

    fn fixture_165(name: &str) -> String {
        let path = format!(
            "{}/tests/fixtures/dialogs/codex-0.165.0/{name}.txt",
            env!("CARGO_MANIFEST_DIR")
        );
        std::fs::read_to_string(path).unwrap()
    }

    fn dialog_165(text: &str) -> Dialog {
        match recognise(text) {
            Recognised::Dialog(d) => d,
            Recognised::Unknown => panic!("not recognised:\n{text}"),
        }
    }

    #[test]
    fn classifies_0_165_0_screens() {
        assert_eq!(
            classify(&fixture_165("folder-access")),
            Screen::FolderAccess {
                open_restricted_selected: true
            }
        );
        assert_eq!(classify(&fixture_165("ready")), Screen::Ready);
        assert_eq!(classify(&fixture_165("question-queued")), Screen::Ready);
        assert_eq!(
            classify(&fixture_165("permission-command")),
            Screen::PermissionDialog(DialogKind::Command)
        );
        assert_eq!(
            classify(&fixture_165("permission-edit")),
            Screen::PermissionDialog(DialogKind::Edits)
        );
        assert_eq!(classify(&fixture_165("after-deny-command")), Screen::Ready);
        assert_eq!(classify(&fixture_165("after-allow-edit")), Screen::Ready);
    }

    #[test]
    fn recognises_permission_options() {
        let d = dialog_165(&fixture_165("permission-command"));
        assert_eq!(d.kind, dialog::DialogKind::Permission);
        assert!(d.actionable);
        assert_eq!(d.selected, 0);
        let offered: Vec<_> = d.options.iter().filter(|o| !o.excluded).collect();
        assert_eq!(offered.len(), 3);
        assert_eq!(offered[0].label, "Yes, proceed (y)");
        assert!(offered[1].label.starts_with("Yes, and don't ask again"));
        assert!(offered[2].is_deny);
        assert!(d.prompt.starts_with(COMMAND_TITLE));
        assert!(d.prompt.contains("$ touch made-by-codex.txt"));
        assert_eq!(
            dialog_165(&fixture_165("permission-command-no-selected")).selected,
            2
        );
        let e = dialog_165(&fixture_165("permission-edit"));
        assert!(e.prompt.starts_with(EDITS_TITLE));
        assert_eq!(dialog_165(&fixture("permission-edit")).options.len(), 3);
    }

    #[test]
    fn altered_dialogs_get_no_actionable_options() {
        let base = fixture_165("permission-command");
        let cases = [
            base.replace(DIALOG_NO, "3. No, deny (n)"),
            base.replace(COMMAND_TITLE, "Something else?"),
            base.replace("› 1. Yes", "  1. Yes"),
            format!("{base}\n› Ask Codex to do anything\n  ? for shortcuts\n"),
            fixture_165("question-queued"),
            fixture_165("ready"),
        ];
        for (i, text) in cases.iter().enumerate() {
            assert!(
                !matches!(recognise(text), Recognised::Dialog(d) if d.actionable),
                "case {i}"
            );
        }
    }

    fn fixture_169(name: &str) -> String {
        let path = format!(
            "{}/tests/fixtures/dialogs/core/codex-0.169.0/{name}.txt",
            env!("CARGO_MANIFEST_DIR")
        );
        std::fs::read_to_string(path).unwrap()
    }

    fn offered(d: &Dialog) -> Vec<&str> {
        d.options
            .iter()
            .filter(|o| !o.excluded)
            .map(|o| o.label.as_str())
            .collect()
    }

    #[test]
    fn recognises_a_single_question() {
        let d = dialog_165(&fixture_169("question-single"));
        assert_eq!(d.kind, dialog::DialogKind::Question);
        assert!(d.actionable);
        assert_eq!(d.prompt, "Pick a colour");
        assert_eq!(offered(&d), ["Red", "Blue"]);
        assert_eq!(d.options.len(), 3);
        assert!(d.options[2].excluded && d.options.iter().all(|o| !o.is_deny));
        assert!(d.options.iter().all(|o| !o.provisional), "it submits");
        assert!(d.steps.is_empty());
        assert_eq!(d.selected, 0);
        assert_eq!(dialog_165(&fixture_169("question-single-down")).selected, 1);
        assert!(d.region.starts_with("Question 1/1\nPick a colour"));
    }

    #[test]
    fn each_question_of_a_group_is_answered_on_its_own() {
        let cases = [
            (
                "question-multi-1",
                "Question 1/3 (3 unanswered)\nPick a size",
                0,
            ),
            (
                "question-multi-1-down",
                "Question 1/3 (3 unanswered)\nPick a size",
                1,
            ),
            (
                "question-multi-2",
                "Question 2/3 (2 unanswered)\nPick a drink",
                0,
            ),
            (
                "question-multi-2-revisit",
                "Question 2/3 (1 unanswered)\nPick a drink",
                0,
            ),
            (
                "question-multi-3",
                "Question 3/3 (1 unanswered)\nPick a colour",
                0,
            ),
            (
                "question-multi-3-down",
                "Question 3/3 (1 unanswered)\nPick a colour",
                2,
            ),
        ];
        for (name, prompt, selected) in cases {
            let d = dialog_165(&fixture_169(name));
            assert_eq!(d.kind, dialog::DialogKind::Question, "{name}");
            assert!(d.actionable, "{name}");
            assert_eq!(
                (d.prompt.as_str(), d.selected),
                (prompt, selected),
                "{name}"
            );
            let open: Vec<&str> = d
                .steps
                .iter()
                .filter(|s| s.current)
                .map(|s| s.label.as_str())
                .collect();
            assert_eq!((d.steps.len(), open), (3, vec![&name[15..16]]), "{name}");
            // Only the last question's Enter submits; the others need no signature.
            let last = name.starts_with("question-multi-3");
            assert!(
                d.options
                    .iter()
                    .all(|o| o.provisional == (!last && !o.excluded)),
                "{name}"
            );
        }
        assert_eq!(
            offered(&dialog_165(&fixture_169("question-multi-3"))),
            ["Red", "Green", "Blue"]
        );
    }

    #[test]
    fn altered_questions_get_no_actionable_options() {
        let base = fixture_169("question-multi-2");
        let cases = [
            base.replace("| esc to interrupt", "| esc to stop"),
            base.replace("tab to add notes", "type your notes"),
            base.replace("Question 2/3 (2 unanswered)", "Question two"),
            base.replace("Question 2/3", "Question 4/3"),
            base.replace("› 1. Tea", "  1. Tea"),
            base.replace("2. Coffee", "3. Coffee"),
            base.replace(
                "  tab to add notes",
                "  a note being typed\n  tab to add notes",
            ),
            format!("{base}\n› Ask Codex to do anything\n  ? for shortcuts\n"),
            fixture_169("question-single-after"),
            fixture_169("question-multi-after"),
        ];
        for (i, text) in cases.iter().enumerate() {
            assert!(
                !matches!(recognise(text), Recognised::Dialog(d) if d.actionable),
                "case {i}"
            );
        }
    }

    #[test]
    fn hooks_review_is_flagged() {
        let text = "  Hooks need review\n  5 hooks are new or changed.\n› 1. Review hooks\n  enter confirm · esc skip\n";
        assert_eq!(classify(text), Screen::HooksReview);
    }
}
