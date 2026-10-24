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
    /// The hub's own HTTPS address, for phones that reach it directly. Not needed with `[relay]`.
    pub public_url: Option<Url>,
    pub listen: SocketAddr,
    pub database: PathBuf,
    /// OAuth for step 7's MCP endpoint; the hub doesn't need it until then.
    pub auth: Option<AuthConfig>,
    /// Where a session starts when the caller names no machine. Defaults to the local machine, else `vps`.
    pub default_machine: Option<String>,
    #[serde(default)]
    pub machines: Vec<MachineConfig>,
    #[serde(default)]
    pub projects: Vec<ProjectConfig>,
    /// Runs the WhatsApp bridge inside the hub.
    pub whatsapp: Option<WhatsAppConfig>,
    /// Which front doors may approve; every authenticated caller may deny.
    #[serde(default)]
    pub approve: ApproveRights,
    /// Push notifications to paired phones. Without it they go through `[relay]`, if set.
    pub push: Option<PushConfig>,
    /// Reach phones through a relay instead of a public address of the hub's own.
    pub relay: Option<HubRelayConfig>,
}

/// The hub's side of a relay: where it is, and the hub's own relay keys.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HubRelayConfig {
    /// The relay's public address, e.g. `https://adarshnb.com/relay`.
    pub url: Url,
    /// The hub's relay keys; created with mode 0600 on first start. Keep it outside any repo.
    pub key_file: PathBuf,
}

impl HubRelayConfig {
    fn validate(&self) -> Result<(), ConfigError> {
        check_url(&self.url, "relay.url", &["https"], &["http"])?;
        if self.url.query().is_some() || self.url.fragment().is_some() {
            return Err(invalid("relay.url must not have a query or fragment"));
        }
        if !self.key_file.is_absolute() {
            return Err(invalid("relay.key_file must be an absolute path"));
        }
        if let Some(repo) = enclosing_git_tree(&self.key_file) {
            return Err(invalid(format!(
                "relay.key_file is inside the git working tree {}; keep it outside any repo",
                repo.display()
            )));
        }
        Ok(())
    }

    /// The address phones post to, without a trailing slash.
    pub fn phone_url(&self) -> String {
        self.url.as_str().trim_end_matches('/').to_owned()
    }

    /// The WebSocket the hub dials: the same address with `ws`/`wss` and `/hub`.
    pub fn hub_url(&self) -> String {
        let base = self.phone_url();
        let ws = match base.strip_prefix("https://") {
            Some(rest) => format!("wss://{rest}"),
            None => format!("ws://{}", base.trim_start_matches("http://")),
        };
        format!("{ws}/hub")
    }
}

/// `relay.toml`: the relay that sits between phones and hubs, and sends their pushes.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RelayConfig {
    pub listen: SocketAddr,
    /// Pushes for every hub, with the app publisher's key. Without it pushes are refused.
    pub push: Option<PushConfig>,
}

impl RelayConfig {
    pub fn load(path: &Path) -> Result<Self, ConfigError> {
        Self::parse(&read(path)?)
    }

    pub fn parse(text: &str) -> Result<Self, ConfigError> {
        let cfg: Self = toml::from_str(text)?;
        if let Some(p) = &cfg.push {
            p.validate()?;
        }
        Ok(cfg)
    }
}

/// How push notifications leave the hub.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PushSenderKind {
    /// Apple's push service, with a token-based key from your developer account.
    Apns,
    /// `xcrun simctl push` to a simulator on this Mac, for testing without Apple.
    Simctl,
    /// Logs what would be sent.
    Log,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PushConfig {
    pub sender: PushSenderKind,
    /// The app's bundle id, the APNs topic.
    pub bundle_id: String,
    pub apns: Option<ApnsConfig>,
    /// The simulator for `simctl`: a UDID or `booted`.
    #[serde(default = "booted")]
    pub simulator: String,
}

fn booted() -> String {
    "booted".into()
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ApnsConfig {
    /// The `AuthKey_<key id>.p8` file from the developer portal; kept outside any repo.
    pub key_file: PathBuf,
    pub key_id: String,
    pub team_id: String,
}

impl PushConfig {
    fn validate(&self) -> Result<(), ConfigError> {
        let id_ok = !self.bundle_id.is_empty()
            && self.bundle_id.contains('.')
            && self
                .bundle_id
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '-');
        if !id_ok {
            return Err(invalid(format!(
                "push.bundle_id {:?} is not a bundle id",
                self.bundle_id
            )));
        }
        match (&self.sender, &self.apns) {
            (PushSenderKind::Apns, None) => {
                return Err(invalid(
                    "push.sender = \"apns\" needs a [push.apns] section",
                ));
            }
            (_, Some(a)) => a.validate()?,
            _ => {}
        }
        Ok(())
    }
}

impl ApnsConfig {
    fn validate(&self) -> Result<(), ConfigError> {
        let ten = |s: &str| {
            s.len() == 10
                && s.chars()
                    .all(|c| c.is_ascii_uppercase() || c.is_ascii_digit())
        };
        if !ten(&self.key_id) {
            return Err(invalid("push.apns.key_id must be the 10-character key id"));
        }
        if !ten(&self.team_id) {
            return Err(invalid(
                "push.apns.team_id must be the 10-character team id",
            ));
        }
        if !self.key_file.is_absolute() {
            return Err(invalid("push.apns.key_file must be an absolute path"));
        }
        if let Some(repo) = enclosing_git_tree(&self.key_file) {
            return Err(invalid(format!(
                "push.apns.key_file is inside the git working tree {}; keep the APNs key outside any repo",
                repo.display()
            )));
        }
        Ok(())
    }
}

/// The right to approve a permission request, per front door. Each defaults to allowed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ApproveRights {
    #[serde(default = "allowed")]
    pub whatsapp: bool,
    /// The static bearer token for the xAI API.
    #[serde(default = "allowed")]
    pub xai: bool,
    /// The OAuth client, i.e. ChatGPT.
    #[serde(default = "allowed")]
    pub chatgpt: bool,
    /// Paired phones; an app approval also needs the device's signature.
    #[serde(default = "allowed")]
    pub app: bool,
}

fn allowed() -> bool {
    true
}

impl Default for ApproveRights {
    fn default() -> Self {
        Self {
            whatsapp: true,
            xai: true,
            chatgpt: true,
            app: true,
        }
    }
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
    /// SHA-256 hex of the runner token, for a runner on another machine that dials in.
    pub token_hash: Option<String>,
    /// The hub's own machine: the hub drives Herdr here itself, with no runner process or token.
    pub local: Option<LocalRunnerConfig>,
}

/// The runner built into the hub, for the machine the hub runs on.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LocalRunnerConfig {
    /// Per-session hook settings and secrets live here, outside every worktree.
    pub state_dir: PathBuf,
    /// Claude Code's config, written to pre-trust repo roots. Defaults to `~/.claude.json`.
    pub claude_config: Option<PathBuf>,
    /// How Claude sessions ask: `auto` (the default) lets Claude's own safety check run routine
    /// actions, `acceptEdits` sends every shell command and tool call to you.
    #[serde(default)]
    pub claude_permission_mode: crate::agents::claude::PermissionMode,
    /// Overrides the Herdr server the `herdr` CLI would find from the environment.
    pub herdr_session: Option<String>,
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
    /// Overrides the Herdr server the `herdr` CLI would find from the environment.
    pub herdr_session: Option<String>,
    /// Per-session hook settings and secrets live here, outside every worktree.
    pub state_dir: PathBuf,
    /// Claude Code's config, written to pre-trust repo roots. Defaults to `~/.claude.json`.
    pub claude_config: Option<PathBuf>,
    /// How Claude sessions ask: `auto` (the default) lets Claude's own safety check run routine
    /// actions, `acceptEdits` sends every shell command and tool call to you.
    #[serde(default)]
    pub claude_permission_mode: crate::agents::claude::PermissionMode,
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
        let mut cfg: Self = toml::from_str(text)?;
        if let Some(wa) = &mut cfg.whatsapp {
            wa.normalize()?;
        }
        cfg.validate()?;
        Ok(cfg)
    }

    pub fn project(&self, id: &str) -> Option<&ProjectConfig> {
        self.projects.iter().find(|p| p.id == id)
    }

    pub fn machine(&self, id: &str) -> Option<&MachineConfig> {
        self.machines.iter().find(|m| m.id == id)
    }

    /// The machine the hub runs on and drives itself, if any.
    pub fn local_machine(&self) -> Option<(&str, &LocalRunnerConfig)> {
        self.machines
            .iter()
            .find_map(|m| Some((m.id.as_str(), m.local.as_ref()?)))
    }

    fn validate(&self) -> Result<(), ConfigError> {
        match &self.public_url {
            Some(url) => {
                check_url(url, "public_url", &["https"], &["http"])?;
                if url.query().is_some() || url.fragment().is_some() {
                    return Err(invalid("public_url must not have a query or fragment"));
                }
            }
            None if self.relay.is_none() => {
                return Err(invalid(
                    "set [relay] for phones to reach the hub, or public_url for direct HTTPS",
                ));
            }
            None => {}
        }
        if !self.database.is_absolute() {
            return Err(invalid("database must be an absolute path"));
        }
        if let Some(auth) = &self.auth {
            auth.validate()?;
        }
        if let Some(wa) = &self.whatsapp {
            wa.validate()?;
        }
        if let Some(push) = &self.push {
            push.validate()?;
        }
        if let Some(relay) = &self.relay {
            relay.validate()?;
        }

        let mut machine_ids = HashSet::new();
        for m in &self.machines {
            check_id(&m.id, "machine id")?;
            if !machine_ids.insert(m.id.as_str()) {
                return Err(invalid(format!("duplicate machine id {:?}", m.id)));
            }
            match (&m.token_hash, &m.local) {
                (Some(hash), None) => {
                    check_sha256_hex(hash, &format!("machine {:?} token_hash", m.id))?
                }
                (None, Some(local)) => check_runner_paths(
                    &local.state_dir,
                    local.claude_config.as_deref(),
                    local.herdr_session.as_deref(),
                )?,
                _ => {
                    return Err(invalid(format!(
                        "machine {:?} needs either token_hash (a runner on another machine) or [machines.local] (this one)",
                        m.id
                    )));
                }
            }
        }
        if self.machines.iter().filter(|m| m.local.is_some()).count() > 1 {
            return Err(invalid("only one machine can be local"));
        }
        if let Some(d) = &self.default_machine
            && !machine_ids.contains(d.as_str())
        {
            return Err(invalid(format!(
                "default_machine {d:?} is not a configured machine"
            )));
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
        if let Some(repo) = enclosing_git_tree(&self.token_file) {
            return Err(invalid(format!(
                "token_file is inside the git working tree {}; keep it outside any repo",
                repo.display()
            )));
        }
        check_runner_paths(
            &self.state_dir,
            self.claude_config.as_deref(),
            self.herdr_session.as_deref(),
        )
    }
}

/// A runner's own settings, whether it runs alone or inside the hub.
fn check_runner_paths(
    state_dir: &Path,
    claude_config: Option<&Path>,
    herdr_session: Option<&str>,
) -> Result<(), ConfigError> {
    if !state_dir.is_absolute() {
        return Err(invalid("state_dir must be an absolute path"));
    }
    // The hook secrets kept under the state folder stay out of every repo.
    if let Some(repo) = enclosing_git_tree(state_dir) {
        return Err(invalid(format!(
            "state_dir is inside the git working tree {}; keep it outside any repo",
            repo.display()
        )));
    }
    if claude_config.is_some_and(|p| !p.is_absolute()) {
        return Err(invalid("claude_config must be an absolute path"));
    }
    match herdr_session {
        Some(s) => check_herdr_session(s),
        None => Ok(()),
    }
}

/// The hub's `[whatsapp]` section: the bridge, a second front door next to MCP.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WhatsAppConfig {
    /// Phone numbers allowed to use the bridge, in international format.
    pub allowed_senders: Vec<String>,
    /// The linked-device credential. Kept outside any git working tree, mode 0600.
    pub store_path: PathBuf,
    /// Link your own account and talk to the bridge in your "Message yourself" chat.
    #[serde(default)]
    pub self_chat: bool,
}

impl WhatsAppConfig {
    fn normalize(&mut self) -> Result<(), ConfigError> {
        self.allowed_senders = self
            .allowed_senders
            .iter()
            .map(|n| phone_digits(n))
            .collect::<Result<_, _>>()
            .map_err(|e| invalid(format!("whatsapp.allowed_senders: {e}")))?;
        Ok(())
    }

    fn validate(&self) -> Result<(), ConfigError> {
        if self.allowed_senders.is_empty() {
            return Err(invalid("allowed_senders must name at least one number"));
        }
        if self.self_chat && self.allowed_senders.len() != 1 {
            return Err(invalid(
                "self_chat needs exactly one allowed sender: your own number, the account you link",
            ));
        }
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
    fn the_hubs_own_machine_needs_no_token() {
        let local =
            "id = \"mac\"\n[machines.local]\nstate_dir = \"/Users/me/.local/state/switchboard\"";
        let text = hub_toml("").replace(&format!("id = \"mac\"\ntoken_hash = \"{HASH}\""), local);
        let cfg = HubConfig::parse(&text).unwrap();
        let (id, l) = cfg.local_machine().unwrap();
        assert_eq!(
            (id, l.state_dir.as_path()),
            ("mac", Path::new("/Users/me/.local/state/switchboard"))
        );
        assert!(cfg.machine("vps").unwrap().local.is_none());
        assert!(
            HubConfig::parse(&hub_toml(""))
                .unwrap()
                .local_machine()
                .is_none()
        );

        let both = text.replace(
            "id = \"mac\"\n",
            &format!("id = \"mac\"\ntoken_hash = \"{HASH}\"\n"),
        );
        assert!(err(&both).contains("either token_hash"));
        let neither = hub_toml("").replace(
            &format!("id = \"mac\"\ntoken_hash = \"{HASH}\""),
            "id = \"mac\"",
        );
        assert!(err(&neither).contains("either token_hash"));
        let two = text.replace(
            &format!("id = \"vps\"\ntoken_hash = \"{HASH}\""),
            "id = \"vps\"\n[machines.local]\nstate_dir = \"/srv/state\"",
        );
        assert!(err(&two).contains("only one machine can be local"));
        let relative = text.replace("/Users/me/.local/state/switchboard", "state");
        assert!(err(&relative).contains("state_dir must be an absolute path"));

        let repo = tempfile::tempdir().unwrap();
        std::fs::create_dir(repo.path().join(".git")).unwrap();
        let in_repo = text.replace(
            "/Users/me/.local/state/switchboard",
            &repo.path().join("state").display().to_string(),
        );
        assert!(err(&in_repo).contains("state_dir is inside the git working tree"));
    }

    #[test]
    fn claude_runs_in_auto_mode_unless_the_machine_says_otherwise() {
        use crate::agents::claude::PermissionMode;
        let local =
            "id = \"mac\"\n[machines.local]\nstate_dir = \"/Users/me/.local/state/switchboard\"";
        let text = hub_toml("").replace(&format!("id = \"mac\"\ntoken_hash = \"{HASH}\""), local);
        let mode = |text: &str| {
            HubConfig::parse(text).map(|c| c.local_machine().unwrap().1.claude_permission_mode)
        };
        assert_eq!(mode(&text).unwrap(), PermissionMode::Auto);
        let set = |m: &str| {
            text.replace(
                "[machines.local]\n",
                &format!("[machines.local]\nclaude_permission_mode = \"{m}\"\n"),
            )
        };
        assert_eq!(
            mode(&set("acceptEdits")).unwrap(),
            PermissionMode::AcceptEdits
        );
        assert_eq!(mode(&set("auto")).unwrap(), PermissionMode::Auto);
        for skips_the_hook in ["bypassPermissions", "dontAsk", "default"] {
            assert!(mode(&set(skips_the_hook)).is_err(), "{skips_the_hook}");
        }
    }

    #[test]
    fn valid_hub_config() {
        let cfg = HubConfig::parse(&hub_toml("")).unwrap();
        let p = cfg.project("switchboard").unwrap();
        assert_eq!(p.default_agent, Agent::Claude);
        assert_eq!(p.repos.len(), 2);
        assert!(cfg.machine("mac").is_some());
        assert_eq!(cfg.auth.unwrap().access_token_ttl_secs, 3600);
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

    /// `hub.toml` with a `[whatsapp]` section built from `body`.
    fn wa(body: &str) -> Result<HubConfig, ConfigError> {
        HubConfig::parse(&hub_toml(&format!("\n[whatsapp]\n{body}")))
    }

    #[test]
    fn whatsapp_section() {
        let dir = tempfile::tempdir().unwrap();
        let store = dir.path().join("state/whatsapp.db");
        let ok = format!(
            "allowed_senders = [\"+44 7700 900123\"]\nstore_path = \"{}\"\n",
            store.display()
        );
        let cfg = wa(&ok).unwrap().whatsapp.unwrap();
        assert_eq!(cfg.allowed_senders, ["447700900123"]);
        assert!(!cfg.self_chat, "self-chat mode is off unless asked for");
        assert!(HubConfig::parse(&hub_toml("")).unwrap().whatsapp.is_none());

        let none = ok.replace(r#"["+44 7700 900123"]"#, "[]");
        assert!(wa(&none).unwrap_err().to_string().contains("at least one"));
        let placeholder = ok.replace("+44 7700 900123", "+<your own number, e.g. +919876543210>");
        let err = wa(&placeholder).unwrap_err().to_string();
        assert!(err.contains("whatsapp.allowed_senders"), "{err}");
        // The Herdr session is the runner's business now, and the old watch list is gone.
        for stale in ["herdr_session = \"switchboard\"", "watch = [\"reviewer\"]"] {
            let err = wa(&format!("{ok}{stale}\n")).unwrap_err().to_string();
            assert!(err.contains("unknown field"), "{err}");
        }
        assert!(
            wa(&format!("{ok}self_chat = true\n"))
                .unwrap()
                .whatsapp
                .unwrap()
                .self_chat
        );
        let two = ok.replace(
            r#"["+44 7700 900123"]"#,
            r#"["+44 7700 900123", "+1 555 000 1111"]"#,
        );
        let err = wa(&format!("{two}self_chat = true\n"))
            .unwrap_err()
            .to_string();
        assert!(err.contains("exactly one allowed sender"), "{err}");
        wa(&two).unwrap();
        let relative = ok.replace(&store.display().to_string(), "whatsapp.db");
        assert!(wa(&relative).is_err());
    }

    #[test]
    fn whatsapp_store_inside_a_repo_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir(dir.path().join(".git")).unwrap();
        let body = format!(
            "allowed_senders = [\"447700900123\"]\nstore_path = \"{}\"\n",
            dir.path().join("sub/whatsapp.db").display()
        );
        let err = wa(&body).unwrap_err().to_string();
        assert!(err.contains("git working tree"), "{err}");
    }

    #[test]
    fn approve_rights_default_to_allowed_and_each_can_be_turned_off() {
        let all = HubConfig::parse(&hub_toml("")).unwrap().approve;
        assert_eq!(all, ApproveRights::default());
        assert!(all.whatsapp && all.xai && all.chatgpt && all.app);
        let some = HubConfig::parse(&hub_toml("\n[approve]\nxai = false\nwhatsapp = false\n"))
            .unwrap()
            .approve;
        assert_eq!(
            (some.whatsapp, some.xai, some.chatgpt, some.app),
            (false, false, true, true)
        );
        assert!(err(&hub_toml("\n[approve]\nmcp = false\n")).contains("unknown field"));
    }

    #[test]
    fn push_section() {
        let dir = tempfile::tempdir().unwrap();
        let key = dir.path().join("AuthKey_ABCDE12345.p8");
        let apns = format!(
            "\n[push]\nsender = \"apns\"\nbundle_id = \"dev.example.switchboard\"\n[push.apns]\nkey_file = \"{}\"\nkey_id = \"ABCDE12345\"\nteam_id = \"TEAM123456\"\n",
            key.display()
        );
        let cfg = HubConfig::parse(&hub_toml(&apns)).unwrap().push.unwrap();
        assert_eq!(
            (cfg.sender, cfg.simulator.as_str()),
            (PushSenderKind::Apns, "booted")
        );
        assert!(HubConfig::parse(&hub_toml("")).unwrap().push.is_none());
        let sim = "\n[push]\nsender = \"simctl\"\nbundle_id = \"dev.example.switchboard\"\n";
        assert_eq!(
            HubConfig::parse(&hub_toml(sim))
                .unwrap()
                .push
                .unwrap()
                .sender,
            PushSenderKind::Simctl
        );
        let missing = sim.replace("simctl", "apns");
        assert!(err(&hub_toml(&missing)).contains("[push.apns]"));
        assert!(err(&hub_toml(&apns.replace("ABCDE12345\"\n", "short\"\n"))).contains("key_id"));
        assert!(
            err(&hub_toml(&sim.replace("dev.example.switchboard", "nodots"))).contains("bundle id")
        );
        std::fs::create_dir(dir.path().join(".git")).unwrap();
        assert!(err(&hub_toml(&apns)).contains("outside any repo"));
    }

    #[test]
    fn relay_sections() {
        let dir = tempfile::tempdir().unwrap();
        let key = dir.path().join("relay-key.json");
        let section = format!(
            "\n[relay]\nurl = \"https://adarshnb.com/relay/\"\nkey_file = \"{}\"\n",
            key.display()
        );
        let r = HubConfig::parse(&hub_toml(&section))
            .unwrap()
            .relay
            .unwrap();
        assert_eq!(r.phone_url(), "https://adarshnb.com/relay");
        assert_eq!(r.hub_url(), "wss://adarshnb.com/relay/hub");
        let local = section.replace("https://adarshnb.com/relay/", "http://127.0.0.1:9090");
        let r = HubConfig::parse(&hub_toml(&local)).unwrap().relay.unwrap();
        assert_eq!(r.hub_url(), "ws://127.0.0.1:9090/hub");
        let no_url = |t: &str| t.replace("public_url = \"https://sb.example.com\"\n", "");
        assert!(
            HubConfig::parse(&no_url(&hub_toml(&section))).is_ok(),
            "a relay needs no domain"
        );
        assert!(err(&no_url(&hub_toml(""))).contains("[relay]"));
        let plain = section.replace("https://adarshnb.com", "http://adarshnb.com");
        assert!(err(&hub_toml(&plain)).contains("relay.url"));
        std::fs::create_dir(dir.path().join(".git")).unwrap();
        assert!(err(&hub_toml(&section)).contains("outside any repo"));

        let relay = RelayConfig::parse("listen = \"127.0.0.1:9090\"\n[push]\nsender = \"log\"\nbundle_id = \"com.digitaltracer.switchboard\"\n").unwrap();
        assert_eq!(relay.push.unwrap().sender, PushSenderKind::Log);
        assert!(RelayConfig::parse("listen = \"127.0.0.1:9090\"\nprojects = []\n").is_err());
    }

    #[test]
    fn default_machine_and_optional_auth() {
        let text = hub_toml("").replace("listen =", "default_machine = \"mac\"\nlisten =");
        assert_eq!(
            HubConfig::parse(&text).unwrap().default_machine.as_deref(),
            Some("mac")
        );
        let unknown = hub_toml("").replace("listen =", "default_machine = \"laptop\"\nlisten =");
        assert!(err(&unknown).contains("default_machine"));
        let no_auth = hub_toml("").replace(
            "[auth]\npassphrase_hash = \"$argon2id$v=19$m=19456,t=2,p=1$c2FsdA$aGFzaA\"\n",
            "",
        );
        assert!(HubConfig::parse(&no_auth).unwrap().auth.is_none());
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
        let found = ok.replace("herdr_session = \"switchboard\"\n", "");
        assert_eq!(RunnerConfig::parse(&found).unwrap().herdr_session, None);

        let repo = tempfile::tempdir().unwrap();
        std::fs::create_dir(repo.path().join(".git")).unwrap();
        let in_repo = repo.path().join("state").display().to_string();
        let state = ok.replace("/Users/me/.local/state/switchboard", &in_repo);
        assert!(err_runner(&state).contains("state_dir is inside the git working tree"));
        let token = ok.replace("/Users/me/.config/switchboard/runner.token", &in_repo);
        assert!(err_runner(&token).contains("token_file is inside the git working tree"));
    }

    fn err_runner(text: &str) -> String {
        RunnerConfig::parse(text).unwrap_err().to_string()
    }
}
