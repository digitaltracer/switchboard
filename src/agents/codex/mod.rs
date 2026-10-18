//! Codex profile: restricted mode per run, own hooks from `-c`, version-pinned screen adapter.
//! Codex can't make a failed hook fail closed, so the runner's keystroke-deny fallback covers it.

pub mod screen;

use std::path::Path;

use super::HOOK_TIMEOUT_SECS;

/// Versions whose startup and permission screens the adapter was built from.
pub const SUPPORTED_VERSIONS: &[&str] = &["0.163.0"];

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("codex {0:?} is not a supported version ({supported})", supported = SUPPORTED_VERSIONS.join(", "))]
pub struct UnsupportedVersion(pub String);

/// Parses `codex --version` output such as `codex-cli 0.163.0`.
pub fn parse_version(output: &str) -> Option<String> {
    let v = output.trim().strip_prefix("codex-cli ")?.trim();
    (!v.is_empty() && !v.contains(char::is_whitespace)).then(|| v.to_owned())
}

pub fn check_version(version: &str) -> Result<(), UnsupportedVersion> {
    if SUPPORTED_VERSIONS.contains(&version) {
        Ok(())
    } else {
        Err(UnsupportedVersion(version.to_owned()))
    }
}

/// Arguments for `herdr agent start --kind codex -- …`.
/// Marking the repo untrusted disables its own `.codex` config, hooks and exec policies, so the
/// bypass flag only skips review for the hook passed here.
pub fn launch_args(repo_root: &Path, hook_command: &str) -> Vec<String> {
    vec![
        "-s".into(),
        "workspace-write".into(),
        "-a".into(),
        "on-request".into(),
        "-c".into(),
        format!(
            "projects={{{}={{trust_level=\"untrusted\"}}}}",
            toml_str(&repo_root.to_string_lossy())
        ),
        "--dangerously-bypass-hook-trust".into(),
        "-c".into(),
        format!(
            "hooks.PermissionRequest=[{{matcher=\"*\",hooks=[{{type=\"command\",command={},timeout={HOOK_TIMEOUT_SECS}}}]}}]",
            toml_str(hook_command)
        ),
    ]
}

/// A TOML basic string.
pub fn toml_str(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            c if c.is_control() => out.push_str(&format!("\\u{:04X}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn version_pinning() {
        assert_eq!(
            parse_version("codex-cli 0.163.0\n").as_deref(),
            Some("0.163.0")
        );
        assert_eq!(parse_version("codex 0.163.0"), None);
        assert_eq!(parse_version("codex-cli "), None);
        assert!(check_version("0.163.0").is_ok());
        assert_eq!(
            check_version("0.164.0"),
            Err(UnsupportedVersion("0.164.0".into()))
        );
    }

    #[test]
    fn launch_args_untrust_repo_and_pass_own_hook() {
        let args = launch_args(
            Path::new("/srv/my \"repo\""),
            "/bin/sb hook --agent codex --secret-file '/s 1'",
        );
        assert_eq!(args[..4], ["-s", "workspace-write", "-a", "on-request"]);
        assert_eq!(
            args[5],
            r#"projects={"/srv/my \"repo\""={trust_level="untrusted"}}"#
        );
        assert!(args.contains(&"--dangerously-bypass-hook-trust".to_string()));
        assert_eq!(
            args[8],
            r#"hooks.PermissionRequest=[{matcher="*",hooks=[{type="command",command="/bin/sb hook --agent codex --secret-file '/s 1'",timeout=300}]}]"#
        );
        assert!(
            !args
                .iter()
                .any(|a| a.contains("trusted\"") && !a.contains("untrusted"))
        );
    }

    #[test]
    fn launch_args_parse_as_toml() {
        let args = launch_args(Path::new("C:\\odd\\path"), "x \"y\"");
        let doc: toml::Table = toml::from_str(&args[5]).unwrap();
        assert_eq!(
            doc["projects"]["C:\\odd\\path"]["trust_level"].as_str(),
            Some("untrusted")
        );
        let hook: toml::Table =
            toml::from_str(&args[8].replacen("hooks.PermissionRequest", "h", 1)).unwrap();
        assert_eq!(
            hook["h"][0]["hooks"][0]["command"].as_str(),
            Some("x \"y\"")
        );
    }
}
