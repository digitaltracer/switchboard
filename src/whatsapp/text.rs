//! Terminal text to WhatsApp messages: strip ANSI, keep the tail, split to size.

/// Characters per WhatsApp message; well under the protocol limit, readable on a phone.
pub const MAX_MESSAGE_CHARS: usize = 3000;
/// More chunks than this and the oldest text is dropped.
pub const MAX_MESSAGES: usize = 4;

/// Removes CSI/OSC escape sequences and other control characters except newlines and tabs.
pub fn strip_ansi(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '\u{1b}' => match chars.next() {
                Some('[') => {
                    for c in chars.by_ref() {
                        if ('@'..='~').contains(&c) {
                            break;
                        }
                    }
                }
                Some(']') => {
                    while let Some(c) = chars.next() {
                        if c == '\u{7}' || (c == '\u{1b}' && chars.peek() == Some(&'\\')) {
                            chars.next_if_eq(&'\\');
                            break;
                        }
                    }
                }
                _ => {}
            },
            '\n' | '\t' => out.push(c),
            c if c.is_control() => {}
            c => out.push(c),
        }
    }
    out
}

/// Lines drawn only from box-drawing characters, like a TUI's horizontal rules, which a
/// narrow pane may cut short with an ellipsis.
fn is_rule(line: &str) -> bool {
    let body = line.trim().trim_end_matches('…');
    !body.is_empty() && body.chars().all(|c| ('\u{2500}'..='\u{257F}').contains(&c))
}

/// Lines to ask Herdr for when `n` meaningful lines are wanted; TUIs pad with blank rows.
pub fn overfetch(n: u32) -> u32 {
    n.saturating_mul(4).min(800)
}

/// The last `n` lines, ANSI stripped, with box-drawing rules dropped and blank runs collapsed.
pub fn tail(text: &str, n: usize) -> String {
    let clean = strip_ansi(text);
    let mut lines: Vec<&str> = clean
        .lines()
        .map(str::trim_end)
        .filter(|l| !is_rule(l))
        .collect();
    lines.dedup_by(|a, b| a.is_empty() && b.is_empty());
    let end = lines
        .iter()
        .rposition(|l| !l.is_empty())
        .map_or(0, |i| i + 1);
    lines[end.saturating_sub(n)..end].join("\n")
}

/// Splits on line boundaries into at most `MAX_MESSAGES` chunks of `MAX_MESSAGE_CHARS`,
/// keeping the newest text when it doesn't fit.
pub fn split(text: &str) -> Vec<String> {
    let mut chunks: Vec<String> = Vec::new();
    let mut current = String::new();
    for line in text.lines() {
        let mut line: String = line.to_owned();
        // A single over-long line is cut into pieces.
        while line.chars().count() > MAX_MESSAGE_CHARS {
            let cut: String = line.chars().take(MAX_MESSAGE_CHARS).collect();
            line = line.chars().skip(MAX_MESSAGE_CHARS).collect();
            if !current.is_empty() {
                chunks.push(std::mem::take(&mut current));
            }
            chunks.push(cut);
        }
        let extra = if current.is_empty() { 0 } else { 1 };
        if current.chars().count() + extra + line.chars().count() > MAX_MESSAGE_CHARS {
            chunks.push(std::mem::take(&mut current));
        }
        if !current.is_empty() {
            current.push('\n');
        }
        current.push_str(&line);
    }
    if !current.is_empty() || chunks.is_empty() {
        chunks.push(current);
    }
    if chunks.len() > MAX_MESSAGES {
        let dropped = chunks.len() - MAX_MESSAGES;
        chunks.drain(..dropped);
        chunks[0] = format!("(earlier output omitted)\n{}", chunks[0]);
    }
    chunks
}

/// One message: the newest `MAX_MESSAGE_CHARS` of `text`.
pub fn fit_one(text: &str) -> String {
    let n = text.chars().count();
    if n <= MAX_MESSAGE_CHARS {
        return text.to_owned();
    }
    let keep: String = text.chars().skip(n - (MAX_MESSAGE_CHARS - 2)).collect();
    format!("…\n{keep}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fit_one_keeps_the_newest_text() {
        assert_eq!(fit_one("abc"), "abc");
        let long = format!("{}END", "x".repeat(MAX_MESSAGE_CHARS));
        let fitted = fit_one(&long);
        assert_eq!(fitted.chars().count(), MAX_MESSAGE_CHARS);
        assert!(fitted.ends_with("END"));
    }

    #[test]
    fn strips_ansi_and_controls() {
        let raw = "\u{1b}[1;32mok\u{1b}[0m done\u{7}\r\n\u{1b}]0;title\u{7}next\ttab\u{1b}]8;;http://x\u{1b}\\link";
        assert_eq!(strip_ansi(raw), "ok done\nnext\ttablink");
    }

    #[test]
    fn tail_collapses_tui_padding() {
        let text = format!("⏺ PONGWA\n\n✻ done\n{}❯\n  status bar\n", "\n".repeat(25));
        assert_eq!(tail(&text, 5), "\n✻ done\n\n❯\n  status bar");
        assert!(tail(&text, 6).starts_with("⏺ PONGWA"));
        assert_eq!(overfetch(30), 120);
        assert_eq!(overfetch(500), 800);
    }

    #[test]
    fn tail_drops_tui_rules() {
        let text = "⏺ PONG\n────────────\n ╌╌╌╌ \n  ───…\n❯ \n  ⏸ manual mode on\n";
        assert_eq!(tail(text, 10), "⏺ PONG\n❯\n  ⏸ manual mode on");
    }

    #[test]
    fn tail_skips_trailing_blank_lines() {
        let text = "a\nb\nc\n\n   \n";
        assert_eq!(tail(text, 2), "b\nc");
        assert_eq!(tail(text, 10), "a\nb\nc");
        assert_eq!(tail("", 5), "");
    }

    #[test]
    fn split_respects_limits_and_keeps_newest() {
        assert_eq!(split("short"), vec!["short".to_string()]);
        assert_eq!(split(""), vec![String::new()]);

        let line = "x".repeat(1000);
        let text = vec![line.clone(); 5].join("\n");
        let parts = split(&text);
        assert_eq!(
            parts.len(),
            3,
            "two lines plus a newline fit, a third would not"
        );
        assert!(parts.iter().all(|p| p.chars().count() <= MAX_MESSAGE_CHARS));
        assert_eq!(parts.concat().matches('x').count(), 5000);

        let huge: String = (0..100)
            .map(|i| format!("line {i} {}\n", "y".repeat(400)))
            .collect();
        let parts = split(&huge);
        assert_eq!(parts.len(), MAX_MESSAGES);
        assert!(parts[0].starts_with("(earlier output omitted)"));
        assert!(parts.last().unwrap().contains("line 99 "));
        assert!(
            parts
                .iter()
                .all(|p| p.chars().count() <= MAX_MESSAGE_CHARS + 30)
        );

        let one_long_line = "z".repeat(MAX_MESSAGE_CHARS * 2 + 5);
        let parts = split(&one_long_line);
        assert_eq!(
            parts.iter().map(|p| p.len()).sum::<usize>(),
            MAX_MESSAGE_CHARS * 2 + 5
        );
        assert!(parts.iter().all(|p| p.chars().count() <= MAX_MESSAGE_CHARS));
    }
}
