//! Chat commands. Anything that isn't one is a prompt for the selected agent.

pub const DEFAULT_READ_LINES: u32 = 30;
pub const MAX_READ_LINES: u32 = 200;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Command {
    Status,
    List,
    Use(String),
    Read(u32),
    Help,
    Mute(Target),
    Unmute(Target),
    /// Opt in an agent Switchboard didn't create.
    Watch(String),
    Prompt(String),
}

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

pub fn parse(text: &str) -> Command {
    let trimmed = text.trim();
    let mut words = trimmed.split_whitespace();
    let first = words.next().unwrap_or("").to_ascii_lowercase();
    let rest: Vec<&str> = words.collect();
    match (first.as_str(), rest.as_slice()) {
        ("status", []) => Command::Status,
        ("list", []) => Command::List,
        ("help", []) | ("?", []) => Command::Help,
        ("use", [agent]) => Command::Use((*agent).to_owned()),
        ("mute", [t]) => Command::Mute(target(t)),
        ("unmute", [t]) => Command::Unmute(target(t)),
        ("watch", [agent]) => Command::Watch((*agent).to_owned()),
        ("read", []) => Command::Read(DEFAULT_READ_LINES),
        // "read the logs" is a prompt; only a bare count is the command.
        ("read", [n]) if n.parse::<u32>().is_ok() => {
            Command::Read(n.parse::<u32>().unwrap().clamp(1, MAX_READ_LINES))
        }
        _ => Command::Prompt(trimmed.to_owned()),
    }
}

pub const HELP: &str = "Commands:
status - watched agents and whether they're muted
list - every live Herdr agent and its watch state
use <agent> - send later messages to this agent
read [n] - last n lines of the agent's output (default 30, max 200)
mute <agent> / mute all - stop notifications (mute all also covers agents created later)
unmute <agent> / unmute all - resume them
watch <agent> - get notifications from an agent Switchboard didn't start
help - this message

Switchboard's own agents (sb-...) are watched automatically. Anything else you send goes to the selected agent as a prompt, watched or not. This bridge can't approve or deny anything.";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn commands() {
        assert_eq!(parse("status"), Command::Status);
        assert_eq!(parse("  STATUS \n"), Command::Status);
        assert_eq!(parse("list"), Command::List);
        assert_eq!(parse("help"), Command::Help);
        assert_eq!(parse("use reviewer"), Command::Use("reviewer".into()));
        assert_eq!(parse("read"), Command::Read(DEFAULT_READ_LINES));
        assert_eq!(parse("read 50"), Command::Read(50));
        assert_eq!(parse("read 99999"), Command::Read(MAX_READ_LINES));
        assert_eq!(parse("read 0"), Command::Read(1));
        assert_eq!(parse("mute all"), Command::Mute(Target::All));
        assert_eq!(parse("Mute ALL"), Command::Mute(Target::All));
        assert_eq!(
            parse("mute sb-3f9a"),
            Command::Mute(Target::Agent("sb-3f9a".into()))
        );
        assert_eq!(parse("unmute all"), Command::Unmute(Target::All));
        assert_eq!(
            parse("unmute sb-3f9a"),
            Command::Unmute(Target::Agent("sb-3f9a".into()))
        );
        assert_eq!(
            parse("watch switchboard-impl"),
            Command::Watch("switchboard-impl".into())
        );
    }

    #[test]
    fn everything_else_is_a_prompt() {
        for text in [
            "read the logs and fix it",
            "use the other approach",
            "status of the build?",
            "list files",
            "fix the tests",
            "watch the build carefully",
            "mute the noisy logger",
            "mute",
        ] {
            assert_eq!(parse(text), Command::Prompt(text.into()), "{text}");
        }
        assert_eq!(parse("  fix it  "), Command::Prompt("fix it".into()));
    }
}
