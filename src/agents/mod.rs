//! Launch profiles: CLI arguments, hook configuration and startup handling per agent.

pub mod claude;
pub mod codex;

use std::path::Path;
use std::time::Duration;

use crate::cli::Agent;

/// The `timeout` written into each CLI's hook configuration.
pub const HOOK_TIMEOUT_SECS: u64 = 300;
/// The helper answers before the CLI's hook timeout, so the CLI never sees a timeout.
pub const HELPER_DEADLINE: Duration = Duration::from_secs(290);
/// Undecided approvals are denied before the helper's own deadline.
pub const APPROVAL_DEADLINE: Duration = Duration::from_secs(280);

/// The `switchboard hook` command line a CLI runs, naming the session's secret file.
pub fn hook_command(exe: &Path, agent: Agent, secret_file: &Path) -> String {
    format!(
        "{} hook --agent {} --secret-file {}",
        shell_quote(&exe.to_string_lossy()),
        agent.as_str(),
        shell_quote(&secret_file.to_string_lossy())
    )
}

/// POSIX single-quote quoting.
pub fn shell_quote(s: &str) -> String {
    if !s.is_empty()
        && s.bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"/._-+=:@".contains(&b))
    {
        return s.to_owned();
    }
    format!("'{}'", s.replace('\'', r"'\''"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deadlines_nest_inside_the_cli_timeout() {
        assert!(APPROVAL_DEADLINE < HELPER_DEADLINE);
        assert!(HELPER_DEADLINE < Duration::from_secs(HOOK_TIMEOUT_SECS));
    }

    #[test]
    fn quoting() {
        assert_eq!(shell_quote("/usr/bin/switchboard"), "/usr/bin/switchboard");
        assert_eq!(shell_quote("/a b/it's"), r"'/a b/it'\''s'");
        assert_eq!(shell_quote(""), "''");
        let cmd = hook_command(
            Path::new("/opt/sb/switchboard"),
            Agent::Claude,
            Path::new("/var/sb/s 1/secret"),
        );
        assert_eq!(
            cmd,
            "/opt/sb/switchboard hook --agent claude --secret-file '/var/sb/s 1/secret'"
        );
    }
}
