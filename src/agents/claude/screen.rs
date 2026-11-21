//! Recogniser for Claude Code's question, plan and permission dialogs by their layout, built from
//! the 2.1.312 captures in `tests/fixtures/dialogs/claude-2.1.312/` and the 2.1.325 grouped
//! questions in `tests/fixtures/dialogs/core/claude-2.1.325/`, which stay as its tests.

use crate::agents::dialog::{self, Dialog, DialogKind, DialogOption, DialogStep, Recognised};

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
/// How a group's tab row ends: `←  ☐ Size  ☒ Drink  ✔ Submit  →`.
const GROUP_TAB_END: &str = "✔ Submit";
/// Lines a question's text may wrap to, between its tab row and row 1.
const QUESTION_LINES: usize = 12;
const REVIEW_TITLE: &str = "Review your answers";
const REVIEW_ASK: &str = "Ready to submit your answers?";
const REVIEW_ROWS: &[&str] = &["Submit answers", "Cancel"];

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
        review(&lines)
    };
    found.map_or_else(|| dialog::generic(text, MARKER), Recognised::Dialog)
}

/// The question tabs of a group's tab row, without its arrows and Submit tab.
fn group_tabs(tab: &str) -> Option<&str> {
    let tabs = tab.strip_prefix('←')?.strip_suffix('→')?.trim();
    let tabs = tabs.strip_suffix(GROUP_TAB_END)?.trim();
    tabs.starts_with(['☐', '☒']).then_some(tabs)
}

/// A group's tabs, ending with its Submit tab. The open tab shows only in colour, which a text
/// read doesn't carry, so it is worked out: the Submit tab on the review; otherwise the first
/// unanswered tab, or the only answered one when the open question already has a pick.
fn steps(tabs: &str, review: bool, picked: bool) -> Vec<DialogStep> {
    let mut steps = Vec::new();
    for tab in tabs.split("  ").map(str::trim).filter(|t| !t.is_empty()) {
        let (answered, label) = match (tab.strip_prefix("☒ "), tab.strip_prefix("☐ ")) {
            (Some(l), _) => (true, l),
            (_, Some(l)) => (false, l),
            _ => return Vec::new(),
        };
        steps.push(DialogStep {
            label: label.to_owned(),
            answered,
            current: false,
        });
    }
    if steps.len() < 2 {
        return Vec::new();
    }
    let open: Vec<usize> = (0..steps.len())
        .filter(|&i| steps[i].answered == picked)
        .collect();
    if !review
        && (!picked || open.len() == 1)
        && let Some(&i) = open.first()
    {
        steps[i].current = true;
    }
    steps.push(DialogStep {
        label: "Submit".into(),
        answered: false,
        current: review,
    });
    steps
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
    // The tab row tops the dialog, under a rule; a long question wraps below it behind a bar.
    let floor = r.first.saturating_sub(QUESTION_LINES + 1);
    let tab_at = (floor..r.first.checked_sub(1)?)
        .rev()
        .take_while(|&i| !is_rule(lines[i]))
        .find(|&i| lines[i].starts_with(QUESTION_TAB) || group_tabs(lines[i]).is_some())?;
    let tab = lines[tab_at];
    let ask = lines[tab_at + 1..r.first]
        .iter()
        .map(|l| l.trim_start_matches('│').trim())
        .collect::<Vec<_>>()
        .join(" ");
    let multi_select = r.rows.iter().any(|(_, l)| l.starts_with(MULTI_SELECT_BOX));
    // One question of a group is answered like a single one; Enter moves to the next tab.
    let group = group_tabs(tab);
    let single = tab.starts_with(QUESTION_TAB) && !tab.contains("Submit");
    let answerable = !multi_select && (single || group.is_some());
    let options = r
        .rows
        .iter()
        .map(|&(number, label)| {
            let excluded =
                !answerable || label.starts_with("Type something") || label == "Chat about this";
            DialogOption {
                number,
                label: label.to_owned(),
                is_deny: false,
                excluded,
                // A group submits nothing until its review.
                provisional: !excluded && group.is_some(),
            }
        })
        .collect();
    // An earlier pick on this question ends its row with a tick.
    let picked = r.rows.iter().any(|(_, l)| l.ends_with(" ✔"));
    let steps = group.map_or_else(Vec::new, |tabs| steps(tabs, false, picked));
    Some(Dialog {
        kind: DialogKind::Question,
        prompt: ask,
        options,
        selected: r.selected,
        region: region(lines, tab_at),
        actionable: answerable,
        steps,
    })
}

/// A group's last tab, which lists the answers picked. It has no footer: its Cancel row ends
/// the screen. Cancel declines every question.
fn review(lines: &[&str]) -> Option<Dialog> {
    dialog::row(lines.last()?, MARKER)?;
    let r = dialog::rows(lines, lines.len(), MARKER)?;
    let labels: Vec<&str> = r.rows.iter().map(|r| r.1).collect();
    let ask_at = r.first.checked_sub(1)?;
    if labels != REVIEW_ROWS || lines[ask_at] != REVIEW_ASK {
        return None;
    }
    let floor = ask_at.saturating_sub(dialog::SPAN);
    let title_at = (floor..ask_at).rev().find(|&i| lines[i] == REVIEW_TITLE)?;
    let tab_at = title_at.checked_sub(1)?;
    let tabs = group_tabs(lines[tab_at])?;
    let options = r
        .rows
        .iter()
        .map(|&(number, label)| DialogOption {
            number,
            label: label.to_owned(),
            is_deny: label == REVIEW_ROWS[1],
            excluded: false,
            provisional: false,
        })
        .collect();
    Some(Dialog {
        kind: DialogKind::Question,
        prompt: lines[title_at..=ask_at].join("\n"),
        options,
        selected: r.selected,
        region: region(lines, tab_at),
        actionable: true,
        steps: steps(tabs, true, false),
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
            excluded: false,
            provisional: false,
        })
        .collect();
    Some(Dialog {
        kind: DialogKind::Permission,
        prompt,
        options,
        selected: r.selected,
        region: region(lines, start),
        actionable: true,
        steps: Vec::new(),
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
            provisional: false,
        })
        .collect();
    Some(Dialog {
        kind: DialogKind::Plan,
        prompt,
        options,
        selected: r.selected,
        region: region(lines, start),
        actionable: true,
        steps: Vec::new(),
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
        assert!(d.options.iter().all(|o| !o.provisional), "it submits");
        assert_eq!(d.selected, 0);
        assert_eq!(dialog(&fixture("question-single-down")).selected, 1);
        assert!(d.region.starts_with("☐ Colour"));
    }

    fn grouped(name: &str) -> String {
        let path = format!(
            "{}/tests/fixtures/dialogs/core/claude-2.1.325/{name}.txt",
            env!("CARGO_MANIFEST_DIR")
        );
        std::fs::read_to_string(path).unwrap()
    }

    #[test]
    fn a_question_that_wraps_over_several_lines_is_answerable() {
        let d = dialog(&grouped("question-single-long"));
        assert_eq!(d.kind, DialogKind::Question);
        assert!(d.actionable);
        assert!(
            d.prompt
                .starts_with("The fix for grouped questions is written")
        );
        assert!(d.prompt.contains("The project rules forbid me from typing"));
        assert!(d.prompt.ends_with("so I can run the phone check?"));
        assert!(!d.prompt.contains('│'));
        assert_eq!(
            offered(&d),
            [
                "You restart it for me",
                "I will restart it myself",
                "Skip the phone check"
            ]
        );
        assert_eq!(d.selected, 0);
        assert_eq!(dialog(&grouped("question-single-long-down")).selected, 1);
        assert!(d.region.starts_with("☐ Hub restart"));
        // Without its tab row it is only a list.
        let no_tab = grouped("question-single-long").replace("☐ Hub restart", "Hub restart");
        assert!(!matches!(recognise(&no_tab), Recognised::Dialog(d) if d.actionable));
    }

    #[test]
    fn multi_select_is_read_only() {
        for text in [
            fixture("question-multiselect"),
            grouped("question-mixed-multiselect"),
        ] {
            let d = dialog(&text);
            assert_eq!(d.kind, DialogKind::Question);
            assert!(!d.actionable);
            assert!(offered(&d).is_empty());
        }
    }

    #[test]
    fn each_question_of_a_group_is_answered_on_its_own() {
        let d = dialog(&fixture("question-multi"));
        assert!(d.actionable);
        assert_eq!(d.prompt, "Pick a size");
        assert_eq!(offered(&d), ["Small", "Large"]);
        let cases = [
            ("question-multi-1", "Pick a size", 0),
            ("question-multi-1-down", "Pick a size", 1),
            ("question-multi-2", "Pick a drink", 0),
            ("question-multi-3", "Pick a colour", 0),
        ];
        for (name, prompt, selected) in cases {
            let d = dialog(&grouped(name));
            assert_eq!(d.kind, DialogKind::Question, "{name}");
            assert!(d.actionable, "{name}");
            assert_eq!(
                (d.prompt.as_str(), d.selected),
                (prompt, selected),
                "{name}"
            );
            assert!(d.options.iter().any(|o| o.excluded), "{name}");
            assert!(d.options.iter().all(|o| !o.is_deny), "{name}");
            assert_eq!(d.steps.len(), 4, "{name}");
            assert_eq!(d.steps.iter().filter(|s| s.current).count(), 1, "{name}");
            // Only the review submits, so a question's own rows need no signature.
            assert!(
                d.options.iter().all(|o| o.provisional != o.excluded),
                "{name}"
            );
        }
        assert_eq!(
            offered(&dialog(&grouped("question-multi-3"))),
            ["Red", "Green", "Blue"]
        );
        let tabs = |name: &str| -> Vec<(String, bool, bool)> {
            dialog(&grouped(name))
                .steps
                .into_iter()
                .map(|s| (s.label, s.answered, s.current))
                .collect()
        };
        let tab = |l: &str, answered, current| (l.to_owned(), answered, current);
        assert_eq!(
            tabs("question-multi-2"),
            [
                tab("Size", true, false),
                tab("Drink", false, true),
                tab("Colour", false, false),
                tab("Submit", false, false),
            ]
        );
        // All three are answered and one is open again: which one can't be told from text.
        assert!(tabs("question-multi-3-revisit").iter().all(|t| !t.2));
        assert_eq!(
            tabs("question-multi-review"),
            [
                tab("Size", true, false),
                tab("Drink", true, false),
                tab("Colour", true, false),
                tab("Submit", false, true),
            ]
        );
        assert!(dialog(&fixture("question-single")).steps.is_empty());
        assert!(dialog(&fixture("question-multiselect")).steps.is_empty());
        // An answered tab shown again marks the earlier pick.
        let again = dialog(&grouped("question-multi-3-revisit"));
        assert_eq!(offered(&again), ["Red", "Green", "Blue ✔"]);
        assert_eq!(
            recognise(&grouped("question-multi-after")),
            Recognised::Unknown
        );
    }

    #[test]
    fn a_group_ends_with_a_review_to_submit_or_cancel() {
        let d = dialog(&grouped("question-multi-review"));
        assert_eq!(d.kind, DialogKind::Question);
        assert!(d.actionable);
        assert_eq!(offered(&d), ["Submit answers", "Cancel"]);
        assert!(d.options.iter().all(|o| !o.provisional), "it submits");
        assert_eq!(
            d.options.iter().map(|o| o.is_deny).collect::<Vec<_>>(),
            [false, true]
        );
        assert!(
            d.prompt
                .starts_with("Review your answers\n● Pick a size\n→ Large")
        );
        assert!(d.prompt.ends_with("Ready to submit your answers?"));
        assert!(d.region.starts_with("←  ☒ Size"));
        assert_eq!(d.selected, 0);
        assert_eq!(dialog(&grouped("question-multi-review-down")).selected, 1);
        let unanswered = dialog(&grouped("question-mixed-review-unanswered"));
        assert!(
            unanswered
                .prompt
                .contains("You have not answered all questions")
        );
    }

    #[test]
    fn an_altered_review_is_not_answerable() {
        let review = grouped("question-multi-review");
        let cases = [
            format!("{review}\n❯ \n  ⏸ manual mode on\n"),
            format!("{review}\n  some reply\n"),
            review.replace("1. Submit answers", "1. Submit"),
            review.replace("Ready to submit your answers?", "Ready?"),
            review.replace("Review your answers", "Your answers"),
            review.replace("✔ Submit  →", "→"),
            review.replace("←  ☒ Size", "☒ Size"),
            review.replace("❯ 1. Submit answers", "  1. Submit answers"),
        ];
        for (i, text) in cases.iter().enumerate() {
            assert!(
                !matches!(recognise(text), Recognised::Dialog(d) if d.actionable),
                "case {i} was actionable"
            );
        }
    }

    #[test]
    fn permissions_offer_every_row_including_standing_grants() {
        let d = dialog(&fixture("permission-bash"));
        assert_eq!(d.kind, DialogKind::Permission);
        assert_eq!(offered(&d).len(), d.options.len());
        assert_eq!(offered(&d).first(), Some(&"Yes"));
        assert_eq!(offered(&d).last(), Some(&"No"));
        assert!(d.options.last().unwrap().is_deny);
        assert!(d.prompt.starts_with("Bash command\n"));
        assert!(d.prompt.contains("touch made-by-bash.txt"));
        assert!(d.prompt.ends_with("Do you want to proceed?"));
        let e = dialog(&fixture("permission-edit"));
        assert_eq!(offered(&e).len(), 3);
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
