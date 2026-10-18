//! Strictly validated TOML config: `hub.toml` on the hub, `runner.toml` on each runner.

use std::collections::{BTreeMap, HashSet};
use std::net::SocketAddr;
use std::path::{Path, PathBuf};

use serde::Deserialize;
use url::Url;

use crate::cli::Agent;

#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    #[error("reading {path}: {source}")]
    Io {
        path: PathBuf,
        source: std::io::Error,
    },
    #[error("parsing: {0}")]
    Parse(#[from] toml::de::Error),
    #[error("{0}")]
    Invalid(String),
}

fn invalid(msg: impl Into<String>) -> ConfigError {
    ConfigError::Invalid(msg.into())
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HubConfig {
    pub public_url: Url,
    pub listen: SocketAddr,
    pub database: PathBuf,
    pub auth: AuthConfig,
    #[serde(default)]
    pub machines: Vec<MachineConfig>,
    #[serde(default)]
    pub projects: Vec<ProjectConfig>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AuthConfig {
    /// Argon2 PHC string for the consent passphrase.
    pub passphrase_hash: String,
    #[serde(default = "default_access_ttl")]
    pub access_token_ttl_secs: u64,
    #[serde(default = "default_refresh_ttl")]
    pub refresh_token_ttl_secs: u64,
    /// SHA-256 hex of the static bearer token for the xAI API.
    pub xai_token_hash: Option<String>,
}

fn default_access_ttl() -> u64 {
    3600
}

fn default_refresh_ttl() -> u64 {
    30 * 24 * 3600
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MachineConfig {
    pub id: String,
    /// SHA-256 hex of the runner token.
    pub token_hash: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProjectConfig {
    pub id: String,
    #[serde(default)]
    pub aliases: Vec<String>,
    pub description: String,
    /// Repository path per machine id.
    pub repos: BTreeMap<String, PathBuf>,
    #[serde(default = "default_agent")]
    pub default_agent: Agent,
    #[serde(default)]
    pub deny_patterns: Vec<String>,
}

fn default_agent() -> Agent {
    Agent::Claude
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RunnerConfig {
    pub machine_id: String,
    pub hub_url: Url,
    pub token_file: PathBuf,
    pub herdr_session: String,
    /// Per-session hook settings and secrets live here, outside every worktree.
    pub state_dir: PathBuf,
    /// Claude Code's config, written to pre-trust repo roots. Defaults to `~/.claude.json`.
    pub claude_config: Option<PathBuf>,
}

fn read(path: &Path) -> Result<String, ConfigError> {
    std::fs::read_to_string(path).map_err(|source| ConfigError::Io {
        path: path.to_owned(),
        source,
    })
}

impl HubConfig {
    pub fn load(path: &Path) -> Result<Self, ConfigError> {
        Self::parse(&read(path)?)
    }

    pub fn parse(text: &str) -> Result<Self, ConfigError> {
        let cfg: Self = toml::from_str(text)?;
        cfg.validate()?;
        Ok(cfg)
    }

    pub fn project(&self, id: &str) -> Option<&ProjectConfig> {
        self.projects.iter().find(|p| p.id == id)
    }

    pub fn machine(&self, id: &str) -> Option<&MachineConfig> {
        self.machines.iter().find(|m| m.id == id)
    }

    fn validate(&self) -> Result<(), ConfigError> {
        check_url(&self.public_url, "public_url", &["https"], &["http"])?;
        if self.public_url.query().is_some() || self.public_url.fragment().is_some() {
            return Err(invalid("public_url must not have a query or fragment"));
        }
        if !self.database.is_absolute() {
            return Err(invalid("database must be an absolute path"));
        }
        self.auth.validate()?;

        let mut machine_ids = HashSet::new();
        for m in &self.machines {
            check_id(&m.id, "machine id")?;
            if !machine_ids.insert(m.id.as_str()) {
                return Err(invalid(format!("duplicate machine id {:?}", m.id)));
            }
            check_sha256_hex(&m.token_hash, &format!("machine {:?} token_hash", m.id))?;
        }

        let mut names = HashSet::new();
        for p in &self.projects {
            check_id(&p.id, "project id")?;
            for name in std::iter::once(&p.id).chain(&p.aliases) {
                check_id(name, "project alias")?;
                if !names.insert(name.as_str()) {
                    return Err(invalid(format!(
                        "project id or alias {name:?} is used twice"
                    )));
                }
            }
            if p.description.trim().is_empty() {
                return Err(invalid(format!("project {:?} needs a description", p.id)));
            }
            if p.repos.is_empty() {
                return Err(invalid(format!("project {:?} has no repos", p.id)));
            }
            for (machine, path) in &p.repos {
                if !machine_ids.contains(machine.as_str()) {
                    return Err(invalid(format!(
                        "project {:?} names unknown machine {machine:?}",
                        p.id
                    )));
                }
                if !path.is_absolute() {
                    return Err(invalid(format!(
                        "project {:?} repo path on {machine:?} must be absolute",
                        p.id
                    )));
                }
            }
            if p.deny_patterns.iter().any(|d| d.trim().is_empty()) {
                return Err(invalid(format!(
                    "project {:?} has an empty deny pattern",
                    p.id
                )));
            }
        }
        Ok(())
    }
}

impl AuthConfig {
    fn validate(&self) -> Result<(), ConfigError> {
        if !self.passphrase_hash.starts_with("$argon2") {
            return Err(invalid("auth.passphrase_hash must be an argon2 PHC string"));
        }
        if self.access_token_ttl_secs == 0 {
            return Err(invalid("auth.access_token_ttl_secs must be positive"));
        }
        if self.refresh_token_ttl_secs <= self.access_token_ttl_secs {
            return Err(invalid(
                "auth.refresh_token_ttl_secs must exceed access_token_ttl_secs",
            ));
        }
        if let Some(h) = &self.xai_token_hash {
            check_sha256_hex(h, "auth.xai_token_hash")?;
        }
        Ok(())
    }
}

impl RunnerConfig {
    pub fn load(path: &Path) -> Result<Self, ConfigError> {
        Self::parse(&read(path)?)
    }

    pub fn parse(text: &str) -> Result<Self, ConfigError> {
        let cfg: Self = toml::from_str(text)?;
        cfg.validate()?;
        Ok(cfg)
    }

    fn validate(&self) -> Result<(), ConfigError> {
        check_id(&self.machine_id, "machine_id")?;
        check_url(&self.hub_url, "hub_url", &["wss"], &["ws"])?;
        if !self.token_file.is_absolute() {
            return Err(invalid("token_file must be an absolute path"));
        }
        if !self.state_dir.is_absolute() {
            return Err(invalid("state_dir must be an absolute path"));
        }
        if self
            .claude_config
            .as_ref()
            .is_some_and(|p| !p.is_absolute())
        {
            return Err(invalid("claude_config must be an absolute path"));
        }
        check_herdr_session(&self.herdr_session)
    }
}

/// `switchboard whatsapp`: runs on the Mac against the local Herdr session.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WhatsAppConfig {
    /// Phone numbers allowed to use the bridge, in international format.
    pub allowed_senders: Vec<String>,
    pub herdr_session: String,
    /// The linked-device credential. Kept outside any git working tree, mode 0600.
    pub store_path: PathBuf,
}

impl WhatsAppConfig {
    pub fn load(path: &Path) -> Result<Self, ConfigError> {
        Self::parse(&read(path)?)
    }

    pub fn parse(text: &str) -> Result<Self, ConfigError> {
        let mut cfg: Self = toml::from_str(text)?;
        cfg.allowed_senders = cfg
            .allowed_senders
            .iter()
            .map(|n| phone_digits(n))
            .collect::<Result<_, _>>()?;
        cfg.validate()?;
        Ok(cfg)
    }

    fn validate(&self) -> Result<(), ConfigError> {
        if self.allowed_senders.is_empty() {
            return Err(invalid("allowed_senders must name at least one number"));
        }
        check_herdr_session(&self.herdr_session)?;
        if !self.store_path.is_absolute() {
            return Err(invalid("store_path must be an absolute path"));
        }
        if let Some(repo) = enclosing_git_tree(&self.store_path) {
            return Err(invalid(format!(
                "store_path is inside the git working tree {}; keep the WhatsApp credential outside any repo",
                repo.display()
            )));
        }
        Ok(())
    }
}

/// Digits of an international phone number such as `+44 7700 900123`.
pub fn phone_digits(number: &str) -> Result<String, ConfigError> {
    let digits: String = number
        .chars()
        .filter(|c| !matches!(c, '+' | ' ' | '-' | '(' | ')'))
        .collect();
    if (7..=15).contains(&digits.len()) && digits.bytes().all(|b| b.is_ascii_digit()) {
        Ok(digits)
    } else {
        Err(invalid(format!(
            "{number:?} is not an international phone number"
        )))
    }
}

/// The nearest ancestor of `path` (or its nearest existing ancestor) holding a `.git` entry.
fn enclosing_git_tree(path: &Path) -> Option<PathBuf> {
    let start = path.ancestors().find(|p| p.exists())?;
    let start = start.canonicalize().unwrap_or_else(|_| start.to_owned());
    start
        .ancestors()
        .find(|p| p.join(".git").exists())
        .map(Path::to_owned)
}

fn check_herdr_session(s: &str) -> Result<(), ConfigError> {
    if s.is_empty()
        || s.len() > 64
        || !s
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
    {
        return Err(invalid("herdr_session must be 1-64 of [A-Za-z0-9_-]"));
    }
    Ok(())
}

/// Ids and aliases: `[a-z][a-z0-9-]{0,31}`.
fn check_id(id: &str, what: &str) -> Result<(), ConfigError> {
    let mut chars = id.chars();
    let ok = id.len() <= 32
        && chars.next().is_some_and(|c| c.is_ascii_lowercase())
        && chars.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-');
    if ok {
        Ok(())
    } else {
        Err(invalid(format!(
            "{what} {id:?} must match [a-z][a-z0-9-]{{0,31}}"
        )))
    }
}

fn check_sha256_hex(s: &str, what: &str) -> Result<(), ConfigError> {
    if s.len() == 64
        && s.bytes()
            .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
    {
        Ok(())
    } else {
        Err(invalid(format!(
            "{what} must be 64 lowercase hex characters"
        )))
    }
}

/// `secure` schemes are always allowed; `plain` ones only for loopback hosts.
fn check_url(url: &Url, what: &str, secure: &[&str], plain: &[&str]) -> Result<(), ConfigError> {
    let loopback = matches!(url.host_str(), Some("localhost" | "127.0.0.1" | "[::1]"));
    let scheme = url.scheme();
    if url.host_str().is_none() {
        return Err(invalid(format!("{what} needs a host")));
    }
    if secure.contains(&scheme) || (plain.contains(&scheme) && loopback) {
        Ok(())
    } else {
        Err(invalid(format!(
            "{what} must use {} ({} only for localhost)",
            secure.join("/"),
            plain.join("/")
        )))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const HASH: &str = "0000000000000000000000000000000000000000000000000000000000000000";

    fn hub_toml(extra: &str) -> String {
        format!(
            r#"
public_url = "https://sb.example.com"
listen = "127.0.0.1:8080"
database = "/var/lib/switchboard/sb.db"

[auth]
passphrase_hash = "$argon2id$v=19$m=19456,t=2,p=1$c2FsdA$aGFzaA"

[[machines]]
id = "vps"
token_hash = "{HASH}"

[[machines]]
id = "mac"
token_hash = "{HASH}"

[[projects]]
id = "switchboard"
aliases = ["sb"]
description = "This repo"
deny_patterns = ["git push"]
[projects.repos]
vps = "/home/agent/src/switchboard"
mac = "/Users/me/switchboard"
{extra}"#
        )
    }

    fn err(text: &str) -> String {
        HubConfig::parse(text).unwrap_err().to_string()
    }

    #[test]
    fn valid_hub_config() {
        let cfg = HubConfig::parse(&hub_toml("")).unwrap();
        let p = cfg.project("switchboard").unwrap();
        assert_eq!(p.default_agent, Agent::Claude);
        assert_eq!(p.repos.len(), 2);
        assert!(cfg.machine("mac").is_some());
        assert_eq!(cfg.auth.access_token_ttl_secs, 3600);
    }

    #[test]
    fn rejects_unknown_field() {
        let text = hub_toml("").replace("listen =", "colour = \"red\"\nlisten =");
        assert!(err(&text).contains("unknown field"));
    }

    #[test]
    fn rejects_unknown_machine_in_repos() {
        let text = hub_toml("").replace("mac = \"/Users", "laptop = \"/Users");
        assert!(err(&text).contains("unknown machine"));
    }

    #[test]
    fn rejects_relative_repo_path() {
        let text = hub_toml("").replace("\"/Users/me/switchboard\"", "\"switchboard\"");
        assert!(err(&text).contains("must be absolute"));
    }

    #[test]
    fn rejects_alias_collision() {
        let extra = r#"
[[projects]]
id = "sb"
description = "clash"
[projects.repos]
vps = "/srv/sb"
"#;
        assert!(err(&hub_toml(extra)).contains("used twice"));
    }

    #[test]
    fn rejects_bad_ids_and_hashes() {
        assert!(err(&hub_toml("").replace("id = \"vps\"", "id = \"VPS\"")).contains("machine id"));
        assert!(err(&hub_toml("").replacen(HASH, "abc", 1)).contains("64 lowercase hex"));
    }

    #[test]
    fn rejects_plain_http_public_url() {
        let text = hub_toml("").replace("https://sb.example.com", "http://sb.example.com");
        assert!(err(&text).contains("public_url"));
        let local = hub_toml("").replace("https://sb.example.com", "http://localhost:8080");
        HubConfig::parse(&local).unwrap();
    }

    #[test]
    fn rejects_bad_auth() {
        assert!(err(&hub_toml("").replace("$argon2id", "plain")).contains("argon2"));
        let text = hub_toml("").replace(
            "[auth]\n",
            "[auth]\naccess_token_ttl_secs = 100\nrefresh_token_ttl_secs = 50\n",
        );
        assert!(err(&text).contains("refresh_token_ttl_secs"));
    }

    #[test]
    fn whatsapp_config() {
        let dir = tempfile::tempdir().unwrap();
        let store = dir.path().join("state/whatsapp.db");
        let ok = format!(
            r#"
allowed_senders = ["+44 7700 900123"]
herdr_session = "switchboard"
store_path = "{}"
"#,
            store.display()
        );
        let cfg = WhatsAppConfig::parse(&ok).unwrap();
        assert_eq!(cfg.allowed_senders, ["447700900123"]);

        let none = ok.replace(r#"["+44 7700 900123"]"#, "[]");
        assert!(
            WhatsAppConfig::parse(&none)
                .unwrap_err()
                .to_string()
                .contains("at least one")
        );
        let bad_number = ok.replace("+44 7700 900123", "call me");
        assert!(WhatsAppConfig::parse(&bad_number).is_err());
        let old_watch_list = format!("{ok}watch = [\"reviewer\"]\n");
        let err = WhatsAppConfig::parse(&old_watch_list)
            .unwrap_err()
            .to_string();
        assert!(err.contains("unknown field `watch`"), "{err}");
        let relative = ok.replace(&store.display().to_string(), "whatsapp.db");
        assert!(WhatsAppConfig::parse(&relative).is_err());
        let extra = format!("{ok}\ncolour = \"red\"\n");
        assert!(
            WhatsAppConfig::parse(&extra)
                .unwrap_err()
                .to_string()
                .contains("unknown field")
        );
    }

    #[test]
    fn whatsapp_store_inside_a_repo_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir(dir.path().join(".git")).unwrap();
        let text = format!(
            "allowed_senders = [\"447700900123\"]\nherdr_session = \"s\"\nstore_path = \"{}\"\n",
            dir.path().join("sub/whatsapp.db").display()
        );
        let err = WhatsAppConfig::parse(&text).unwrap_err().to_string();
        assert!(err.contains("git working tree"), "{err}");
    }

    #[test]
    fn runner_config() {
        let ok = r#"
machine_id = "mac"
hub_url = "wss://sb.example.com/runner"
token_file = "/Users/me/.config/switchboard/runner.token"
herdr_session = "switchboard"
state_dir = "/Users/me/.local/state/switchboard"
"#;
        let cfg = RunnerConfig::parse(ok).unwrap();
        assert_eq!(cfg.machine_id, "mac");
        let insecure = ok.replace("wss://sb.example.com", "ws://sb.example.com");
        assert!(RunnerConfig::parse(&insecure).is_err());
        let local = ok.replace("wss://sb.example.com", "ws://127.0.0.1:8080");
        RunnerConfig::parse(&local).unwrap();
        let rel = ok.replace(
            "\"/Users/me/.config/switchboard/runner.token\"",
            "\"runner.token\"",
        );
        assert!(RunnerConfig::parse(&rel).is_err());
        let bad_session = ok.replace("\"switchboard\"", "\"../x\"");
        assert!(RunnerConfig::parse(&bad_session).is_err());
    }
}
