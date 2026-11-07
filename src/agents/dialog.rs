//! Shared option-row parser for agent dialogs drawn on screen. The per-CLI layers in
//! `claude/screen.rs` and `codex/screen.rs` add the anchors that make a dialog answerable; a
//! list this parser finds without them is shown read-only.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DialogKind {
    Question,
    Plan,
    Permission,
    /// A numbered list no pinned layer recognised.
    Other,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DialogOption {
    /// The number the row shows.
    pub number: u32,
    pub label: String,
    pub is_deny: bool,
    /// Never chosen by keys: free text, a chat, or a standing permission.
    pub excluded: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Dialog {
    pub kind: DialogKind,
    pub prompt: String,
    pub options: Vec<DialogOption>,
    /// Index into `options` of the row with the selection marker.
    pub selected: usize,
    /// The raw dialog lines, title to footer. Runner-private: it may hold secrets.
    pub region: String,
    /// A pinned layer recognised it, so keys may answer it.
    pub actionable: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Recognised {
    Dialog(Dialog),
    Unknown,
}

/// How far above the footer a dialog's first row may sit.
pub const SPAN: usize = 40;
/// Lines a row's description or wrap may take before the next row.
const MAX_GAP: usize = 4;

/// Trimmed, non-empty lines of a screen read.
pub fn lines(text: &str) -> Vec<&str> {
    text.lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .collect()
}

/// `N. label`, optionally after `marker`: the number, the label and whether it's selected.
pub fn row(line: &str, marker: char) -> Option<(u32, &str, bool)> {
    let (rest, selected) = match line.strip_prefix(marker) {
        Some(r) => (r.trim_start(), true),
        None => (line, false),
    };
    let dot = rest.find(". ")?;
    let n: u32 = rest[..dot].parse().ok()?;
    let label = rest[dot + 2..].trim();
    (n > 0 && dot <= 2 && !label.is_empty()).then_some((n, label, selected))
}

/// Option rows found scanning up from just above the footer (`lines[footer]`) to row 1.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Rows<'a> {
    /// Index in `lines` of row 1.
    pub first: usize,
    pub rows: Vec<(u32, &'a str)>,
    pub selected: usize,
}

/// The numbered rows directly above `footer`, ending at row 1 and numbered without gaps, with
/// exactly one selected. Anything else between rows (descriptions, wraps, rules) is skipped.
pub fn rows<'a>(lines: &[&'a str], footer: usize, marker: char) -> Option<Rows<'a>> {
    let floor = footer.saturating_sub(SPAN);
    let mut found: Vec<(u32, &str, bool)> = Vec::new();
    let mut gap = 0;
    let mut first = None;
    for i in (floor..footer).rev() {
        match row(lines[i], marker) {
            Some((n, label, sel)) => {
                let expected = found.last().map(|r| r.0 - 1);
                if expected.is_some_and(|e| e != n) {
                    return None;
                }
                found.push((n, label, sel));
                gap = 0;
                if n == 1 {
                    first = Some(i);
                    break;
                }
            }
            None => {
                gap += 1;
                if gap > MAX_GAP {
                    return None;
                }
            }
        }
    }
    let first = first?;
    found.reverse();
    let marked: Vec<usize> = (0..found.len()).filter(|&i| found[i].2).collect();
    let [selected] = marked[..] else {
        return None;
    };
    Some(Rows {
        first,
        rows: found.into_iter().map(|(n, l, _)| (n, l)).collect(),
        selected,
    })
}

/// A read-only list under a footer that mentions Esc, for screens no layer pins.
pub fn generic(text: &str, marker: char) -> Recognised {
    let lines = lines(text);
    let Some(footer) = lines.len().checked_sub(1) else {
        return Recognised::Unknown;
    };
    let foot = lines[footer].to_ascii_lowercase();
    if !foot.contains("esc") {
        return Recognised::Unknown;
    }
    let Some(r) = rows(&lines, footer, marker) else {
        return Recognised::Unknown;
    };
    let prompt = r
        .first
        .checked_sub(1)
        .map(|i| lines[i].to_owned())
        .unwrap_or_default();
    Recognised::Dialog(Dialog {
        kind: DialogKind::Other,
        prompt,
        options: r
            .rows
            .iter()
            .map(|(n, l)| DialogOption {
                number: *n,
                label: (*l).to_owned(),
                is_deny: false,
                excluded: true,
            })
            .collect(),
        selected: r.selected,
        region: lines[r.first.saturating_sub(1)..].join("\n"),
        actionable: false,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rows_need_one_marker_and_no_gaps() {
        let l = ["Pick", "❯ 1. Red", "desc", "2. Blue", "Enter · Esc"];
        let r = rows(&l, 4, '❯').unwrap();
        assert_eq!((r.first, r.selected), (1, 0));
        assert_eq!(r.rows, vec![(1, "Red"), (2, "Blue")]);
        assert!(rows(&["❯ 1. A", "3. B", "f"], 2, '❯').is_none());
        assert!(rows(&["1. A", "2. B", "f"], 2, '❯').is_none());
        assert!(rows(&["❯ 1. A", "❯ 2. B", "f"], 2, '❯').is_none());
        assert!(rows(&["❯ 2. A", "3. B", "f"], 2, '❯').is_none());
    }

    #[test]
    fn too_much_text_between_rows_is_not_a_list() {
        let l = ["❯ 1. A", "x", "x", "x", "x", "x", "2. B", "f"];
        assert!(rows(&l, 7, '❯').is_none());
    }

    #[test]
    fn generic_lists_are_read_only() {
        let Recognised::Dialog(d) =
            generic("Proceed?\n> 1. Yes\n  2. No\nenter ok, esc back\n", '>')
        else {
            panic!("not recognised");
        };
        assert_eq!(d.kind, DialogKind::Other);
        assert!(!d.actionable && d.options.iter().all(|o| o.excluded));
        assert_eq!(d.prompt, "Proceed?");
        assert_eq!(
            generic("> 1. Yes\n  2. No\nsome reply\n", '>'),
            Recognised::Unknown
        );
    }
}
