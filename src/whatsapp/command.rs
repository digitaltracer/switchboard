//! Chat commands. Anything that isn't one is free text, routed by the project names it
//! mentions or to the current session.

pub const DEFAULT_READ_LINES: u32 = 30;
pub const MAX_READ_LINES: u32 = 200;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Command {
    Status,
    /// Live sessions and other named agents, numbered for `use <n>` and `stop <n>`.
    Sessions,
    /// Reuse a session; with text, send it there too.
    Use {
        target: Reuse,
        text: Option<String>,
    },
    New {
        project: String,
        prompt: String,
    },
    Read(u32),
    Stop(String),
    Approve(i64),
    Deny(i64),
    Help,
    Mute(Target),
    Unmute(Target),
    /// Opt in an agent Switchboard didn't create.
    Watch(String),
    /// A bare number: the answer to a numbered question, or free text if none is open.
    Pick(usize),
    Text(String),
}

pub use crate::hub::route::Reuse;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Target {
    All,
    Agent(String),
}

fn target(word: &str) -> Target {
    if word.eq_ignore_ascii_case("all") {
        Target::All
    } else {
        Target::Agent(word.to_owned())
    }
}

fn number(word: &str) -> Option<i64> {
    word.trim_start_matches('#').parse().ok().filter(|n| *n > 0)
}

/// `text` after its first `n` words, trimmed.
fn after_words(text: &str, n: usize) -> &str {
    let mut rest = text.trim_start();
    for _ in 0..n {
        rest = rest
            .find(char::is_whitespace)
            .map_or("", |i| rest[i..].trim_start());
    }
    rest.trim_end()
}

fn parse_use(text: &str) -> Option<Command> {
    let (target, text) = crate::hub::route::parse_use(text)?;
    Some(Command::Use { target, text })
}

pub fn parse(text: &str) -> Command {
    let trimmed = text.trim();
    if let Some(cmd) = parse_use(trimmed) {
        return cmd;
    }
    let mut words = trimmed.split_whitespace();
    let first = words.next().unwrap_or("").to_ascii_lowercase();
    let rest: Vec<&str> = words.collect();
    match (first.as_str(), rest.as_slice()) {
        ("status", []) => Command::Status,
        ("sessions" | "list", []) => Command::Sessions,
        ("help" | "?", []) => Command::Help,
        ("mute", [t]) => Command::Mute(target(t)),
        ("unmute", [t]) => Command::Unmute(target(t)),
        ("watch", [agent]) => Command::Watch((*agent).to_owned()),
        ("stop", [x]) => Command::Stop((*x).to_owned()),
        ("approve", [n]) if number(n).is_some() => Command::Approve(number(n).unwrap()),
        ("deny", [n]) if number(n).is_some() => Command::Deny(number(n).unwrap()),
        ("new", [project, _, ..]) => Command::New {
            project: (*project).to_owned(),
            prompt: after_words(trimmed, 2).to_owned(),
        },
        ("read", []) => Command::Read(DEFAULT_READ_LINES),
        // "read the logs" is a prompt; only a bare count is the command.
        ("read", [n]) if n.parse::<u32>().is_ok() => {
            Command::Read(n.parse::<u32>().unwrap().clamp(1, MAX_READ_LINES))
        }
        (n, []) if n.parse::<usize>().is_ok_and(|n| n > 0) => Command::Pick(n.parse().unwrap()),
        _ => Command::Text(trimmed.to_owned()),
    }
}

pub const HELP: &str = "Name a project to start a session there, e.g. \"in switchboard, fix the flaky hub test\". Anything that names no project goes to the current session.

new <project> <prompt> - start a session
use this: <text> - send to the current session
use <x>: <text> - send to a session or agent (or just use <x> to switch)
sessions - what's running, numbered
status - watched sessions and agents, with pending approvals
read [n] - last n lines of the current session (default 30, max 200)
approve <n> / deny <n> - answer a permission request
stop <x> - stop a session; its worktree and branch are kept
mute <x> / mute all - stop notifications (mute all also covers sessions started later)
unmute <x> / unmute all - resume them
watch <agent> - get notifications from an agent Switchboard didn't start
help - this message

When I ask with a numbered list, reply with the number. Agents Switchboard didn't start have no approvals here; answer their dialogs in the Herdr tab.";

#[cfg(test)]
mod tests {
    use super::*;

    fn named(x: &str, text: Option<&str>) -> Command {
        Command::Use {
            target: Reuse::Named(x.into()),
            text: text.map(Into::into),
        }
    }

    #[test]
    fn commands() {
        assert_eq!(parse("status"), Command::Status);
        assert_eq!(parse("  STATUS \n"), Command::Status);
        assert_eq!(parse("sessions"), Command::Sessions);
        assert_eq!(parse("list"), Command::Sessions);
        assert_eq!(parse("help"), Command::Help);
        assert_eq!(parse("read"), Command::Read(DEFAULT_READ_LINES));
        assert_eq!(parse("read 50"), Command::Read(50));
        assert_eq!(parse("read 99999"), Command::Read(MAX_READ_LINES));
        assert_eq!(parse("read 0"), Command::Read(1));
        assert_eq!(parse("mute all"), Command::Mute(Target::All));
        assert_eq!(parse("Mute ALL"), Command::Mute(Target::All));
        assert_eq!(
            parse("unmute sb-3f9a"),
            Command::Unmute(Target::Agent("sb-3f9a".into()))
        );
        assert_eq!(
            parse("watch switchboard-impl"),
            Command::Watch("switchboard-impl".into())
        );
        assert_eq!(parse("approve 7"), Command::Approve(7));
        assert_eq!(parse("deny #12"), Command::Deny(12));
        assert_eq!(parse("stop 2"), Command::Stop("2".into()));
        assert_eq!(parse("3"), Command::Pick(3));
        assert_eq!(
            parse("new switchboard  fix the flaky   hub test "),
            Command::New {
                project: "switchboard".into(),
                prompt: "fix the flaky   hub test".into()
            }
        );
    }

    #[test]
    fn the_reuse_forms() {
        let this = |text: Option<&str>| Command::Use {
            target: Reuse::This,
            text: text.map(Into::into),
        };
        assert_eq!(parse("use this"), this(None));
        assert_eq!(parse("Use this session"), this(None));
        assert_eq!(
            parse("use this session: also add a test"),
            this(Some("also add a test"))
        );
        assert_eq!(parse("use this:  carry on "), this(Some("carry on")));
        assert_eq!(
            parse("Use the auth session: also add a test"),
            named("auth", Some("also add a test"))
        );
        assert_eq!(
            parse("use auth: also: add a test"),
            named("auth", Some("also: add a test"))
        );
        assert_eq!(
            parse("use switchboard-impl"),
            named("switchboard-impl", None)
        );
        assert_eq!(parse("use 2"), named("2", None));
        assert_eq!(parse("use auth:"), named("auth", None));
    }

    #[test]
    fn everything_else_is_free_text() {
        for text in [
            "In switchboard, fix the flaky hub test",
            "read the logs and fix it",
            "use the other approach",
            "Use the following approach: smaller steps",
            "useful idea",
            "status of the build?",
            "fix the tests",
            "watch the build carefully",
            "mute the noisy logger",
            "mute",
            "approve it",
            "deny 0",
            "new switchboard",
            "stop",
            "0",
        ] {
            assert_eq!(parse(text), Command::Text(text.into()), "{text}");
        }
        assert_eq!(parse("  fix it  "), Command::Text("fix it".into()));
    }
}
