//! Recogniser for Claude Code's question, plan and permission dialogs by their layout, built from
//! the 2.1.312 captures in `tests/fixtures/dialogs/claude-2.1.312/`, which stay as its tests.

use crate::agents::dialog::{self, Dialog, DialogKind, DialogOption, Recognised};

const MARKER: char = '❯';
const QUESTION_FOOTER_START: &str = "Enter to select · ";
const QUESTION_FOOTER_END: &str = " · Esc to cancel";
const PERMISSION_FOOTERS: &[&str] = &["Esc to cancel", "Esc to cancel · Tab to amend"];
const PLAN_FOOTER_START: &str = "ctrl+g to edit in Vim · ";
const PLAN_PROMPT: &str =
    "Claude has written up a plan and is ready to execute. Would you like to proceed?";
const PLAN_TITLE: &str = "Ready to code?";
const PLAN_ROWS: &[&str] = &[
    "Yes, and use auto mode",
    "Yes, manually approve edits",
    "Tell Claude what to change",
];
/// The single question's tab: one unchecked box and no Submit tab.
const QUESTION_TAB: &str = "☐ ";
const MULTI_SELECT_BOX: &str = "[ ] ";

/// Classifies a Herdr `visible` read. A layout this doesn't know only gets the read-only parser.
pub fn recognise(text: &str) -> Recognised {
    let lines = dialog::lines(text);
    let Some(footer) = lines.len().checked_sub(1) else {
        return Recognised::Unknown;
    };
    let foot = lines[footer];
    let found = if foot.starts_with(QUESTION_FOOTER_START) && foot.ends_with(QUESTION_FOOTER_END) {
        question(&lines, footer)
    } else if PERMISSION_FOOTERS.contains(&foot) {
        permission(&lines, footer)
    } else if foot.starts_with(PLAN_FOOTER_START) {
        plan(&lines, footer)
    } else {
        None
    };
    found.map_or_else(|| dialog::generic(text, MARKER), Recognised::Dialog)
}

fn is_rule(line: &str) -> bool {
    line.chars().count() >= 20 && line.chars().all(|c| "─╌▔━".contains(c))
}

/// The text from the nearest rule above `end` (exclusive) down to `end`, rules dropped.
fn body(lines: &[&str], end: usize) -> (usize, String) {
    let floor = end.saturating_sub(dialog::SPAN);
    let start = (floor..end)
        .rev()
        .find(|&i| lines[i].chars().all(|c| c == '─') && is_rule(lines[i]))
        .map_or(floor, |i| i + 1);
    let text = lines[start..=end]
        .iter()
        .filter(|l| !is_rule(l))
        .copied()
        .collect::<Vec<_>>()
        .join("\n");
    (start, text)
}

fn region(lines: &[&str], start: usize) -> String {
    lines[start..].join("\n")
}

fn question(lines: &[&str], footer: usize) -> Option<Dialog> {
    let r = dialog::rows(lines, footer, MARKER)?;
    let prompt_at = r.first.checked_sub(1)?;
    let tab = lines.get(prompt_at.checked_sub(1)?)?;
    if !tab.contains('☐') && !tab.contains('☒') && !tab.contains('✔') {
        return None;
    }
    let multi_select = r.rows.iter().any(|(_, l)| l.starts_with(MULTI_SELECT_BOX));
    // A Submit tab or arrows mean several questions, or a multi-select.
    let single = tab.starts_with(QUESTION_TAB) && !tab.contains("Submit") && !multi_select;
    let options = r
        .rows
        .iter()
        .map(|&(number, label)| DialogOption {
            number,
            label: label.to_owned(),
            is_deny: false,
            excluded: !single || label.starts_with("Type something") || label == "Chat about this",
        })
        .collect();
    Some(Dialog {
        kind: DialogKind::Question,
        prompt: lines[prompt_at].to_owned(),
        options,
        selected: r.selected,
        region: region(lines, prompt_at - 1),
        actionable: single,
    })
}

fn permission(lines: &[&str], footer: usize) -> Option<Dialog> {
    let r = dialog::rows(lines, footer, MARKER)?;
    let ask = lines[r.first.checked_sub(1)?];
    if !(ask.starts_with("Do you want to ") && ask.ends_with('?')) {
        return None;
    }
    let (first, last) = (r.rows.first()?.1, r.rows.last()?.1);
    if first != "Yes" || last != "No" || r.rows.len() < 2 {
        return None;
    }
    let (start, prompt) = body(lines, r.first - 1);
    let options = r
        .rows
        .iter()
        .map(|&(number, label)| DialogOption {
            number,
            label: label.to_owned(),
            is_deny: label == "No",
            excluded: label != "Yes" && label != "No",
        })
        .collect();
    Some(Dialog {
        kind: DialogKind::Permission,
        prompt,
        options,
        selected: r.selected,
        region: region(lines, start),
        actionable: true,
    })
}

fn plan(lines: &[&str], footer: usize) -> Option<Dialog> {
    let r = dialog::rows(lines, footer, MARKER)?;
    let ask_at = r.first.checked_sub(1)?;
    let labels: Vec<&str> = r.rows.iter().map(|r| r.1).collect();
    if lines[ask_at] != PLAN_PROMPT || labels != PLAN_ROWS {
        return None;
    }
    let floor = ask_at.saturating_sub(dialog::SPAN);
    let start = (floor..ask_at)
        .rev()
        .find(|&i| lines[i] == PLAN_TITLE)
        .unwrap_or(ask_at);
    let prompt = lines[start..=ask_at]
        .iter()
        .filter(|l| !is_rule(l))
        .copied()
        .collect::<Vec<_>>()
        .join("\n");
    let options = r
        .rows
        .iter()
        .map(|&(number, label)| DialogOption {
            number,
            label: label.to_owned(),
            is_deny: false,
            excluded: label == PLAN_ROWS[2],
        })
        .collect();
    Some(Dialog {
        kind: DialogKind::Plan,
        prompt,
        options,
        selected: r.selected,
        region: region(lines, start),
        actionable: true,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const V: &str = "2.1.312";

    fn fixture(name: &str) -> String {
        let path = format!(
            "{}/tests/fixtures/dialogs/claude-{V}/{name}.txt",
            env!("CARGO_MANIFEST_DIR")
        );
        std::fs::read_to_string(path).unwrap()
    }

    fn dialog(text: &str) -> Dialog {
        match recognise(text) {
            Recognised::Dialog(d) => d,
            Recognised::Unknown => panic!("not recognised:\n{text}"),
        }
    }

    fn offered(d: &Dialog) -> Vec<&str> {
        d.options
            .iter()
            .filter(|o| !o.excluded)
            .map(|o| o.label.as_str())
            .collect()
    }

    #[test]
    fn single_question() {
        let d = dialog(&fixture("question-single"));
        assert_eq!(d.kind, DialogKind::Question);
        assert!(d.actionable);
        assert_eq!(d.prompt, "Which colour do you prefer?");
        assert_eq!(offered(&d), ["Red", "Green", "Blue"]);
        assert_eq!(d.options.len(), 5);
        assert_eq!(d.selected, 0);
        assert_eq!(dialog(&fixture("question-single-down")).selected, 1);
        assert!(d.region.starts_with("☐ Colour"));
    }

    #[test]
    fn multi_select_and_multi_question_are_read_only() {
        for name in ["question-multiselect", "question-multi"] {
            let d = dialog(&fixture(name));
            assert_eq!(d.kind, DialogKind::Question, "{name}");
            assert!(!d.actionable, "{name}");
            assert!(offered(&d).is_empty(), "{name}");
        }
    }

    #[test]
    fn permissions_offer_yes_and_no_only() {
        let d = dialog(&fixture("permission-bash"));
        assert_eq!(d.kind, DialogKind::Permission);
        assert_eq!(offered(&d), ["Yes", "No"]);
        assert!(d.options.last().unwrap().is_deny);
        assert!(d.prompt.starts_with("Bash command\n"));
        assert!(d.prompt.contains("touch made-by-bash.txt"));
        assert!(d.prompt.ends_with("Do you want to proceed?"));
        let e = dialog(&fixture("permission-edit"));
        assert_eq!(offered(&e), ["Yes", "No"]);
        assert_eq!(e.options.len(), 3);
        assert!(e.prompt.starts_with("Edit file\nnotes.txt"));
    }

    #[test]
    fn plan_offers_both_approvals_and_no_feedback_row() {
        let d = dialog(&fixture("plan"));
        assert_eq!(d.kind, DialogKind::Plan);
        assert_eq!(
            offered(&d),
            ["Yes, and use auto mode", "Yes, manually approve edits"]
        );
        assert!(d.options.iter().all(|o| !o.is_deny));
        assert!(d.prompt.starts_with("Ready to code?"));
        assert!(d.prompt.contains("1. Append a line to notes.txt."));
        assert_eq!(dialog(&fixture("plan-down")).selected, 1);
    }

    #[test]
    fn mutated_screens_are_unknown_or_read_only() {
        let bash = fixture("permission-bash");
        let cases = [
            bash.replace("4. No", "4. Nope"),
            bash.replace("Do you want to proceed?", "Do you want to proceed"),
            bash.replace(" ❯ 1. Yes", "   1. Yes"),
            bash.replace("   4. No", " ❯ 4. No"),
            format!("{bash}\n❯ \n  ⏸ manual mode on\n"),
            fixture("plan").replace(PLAN_PROMPT, "Proceed?"),
            fixture("question-single").replace("☐ Colour", "Colour"),
        ];
        for (i, text) in cases.iter().enumerate() {
            assert!(
                !matches!(recognise(text), Recognised::Dialog(d) if d.actionable),
                "case {i} was actionable"
            );
        }
    }

    #[test]
    fn a_moved_marker_moves_the_selection() {
        let text = fixture("permission-bash")
            .replace(" ❯ 1. Yes", "   1. Yes")
            .replace("   4. No", " ❯ 4. No");
        let d = dialog(&text);
        assert!(d.actionable);
        assert_eq!(d.selected, 3);
    }

    #[test]
    fn a_layout_it_does_not_know_is_shown_read_only() {
        let text =
            fixture("permission-bash").replace("Tab to amend", "Tab to amend · ctrl+e to explain");
        match recognise(&text) {
            Recognised::Dialog(d) => {
                assert!(!d.actionable);
                assert_eq!(d.options.len(), 4);
            }
            Recognised::Unknown => panic!("hidden:\n{text}"),
        }
    }

    #[test]
    fn echoes_get_no_dialog() {
        for name in ["question-single", "permission-bash", "plan"] {
            let text = fixture(name);
            let echoed = format!("{text}\n❯ \n  ⏸ manual mode on · ← 2 agents\n");
            assert_eq!(recognise(&echoed), Recognised::Unknown, "{name}");
        }
        assert_eq!(recognise(""), Recognised::Unknown);
    }
}
