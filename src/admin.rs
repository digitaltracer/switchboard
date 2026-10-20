use std::collections::HashMap;
use std::io::Write;
use std::path::{Path, PathBuf};

use anyhow::{Context, bail};

use std::time::Duration;

use crate::auth::device::{CODE_TTL, Devices};
use crate::cli::AdminCommand;
use crate::config::{HubConfig, RunnerConfig};
use crate::hub::service::Clock;
use crate::proto::auth::hash_token;
use crate::runner::random_id;
use crate::store::Store;

pub fn run(command: AdminCommand) -> anyhow::Result<()> {
    match command {
        AdminCommand::CheckConfig { hub, runner } => {
            if hub.is_none() && runner.is_none() {
                bail!("pass --hub and/or --runner");
            }
            if let Some(path) = hub {
                HubConfig::load(&path).with_context(|| format!("{}", path.display()))?;
                println!("{}: ok", path.display());
            }
            if let Some(path) = runner {
                RunnerConfig::load(&path).with_context(|| format!("{}", path.display()))?;
                println!("{}: ok", path.display());
            }
            Ok(())
        }
        AdminCommand::MintRunnerToken { machine, out } => {
            let hash = mint_runner_token(&out)?;
            println!(
                "Wrote a runner token to {} (mode 0600). Add this to hub.toml:\n\n[[machines]]\nid = \"{machine}\"\ntoken_hash = \"{hash}\"",
                out.display()
            );
            Ok(())
        }
        AdminCommand::ScanProjects { machine, roots } => {
            let repos = find_repos(&roots)?;
            print!("{}", projects_toml(&machine, &repos)?);
            Ok(())
        }
        AdminCommand::PairDevice { config } => with_devices(&config, |d, cfg| async move {
            let stdin = std::io::stdin();
            pair_device(
                &d,
                cfg.public_url.as_str(),
                &mut stdin.lock(),
                &mut std::io::stdout(),
                PAIR_POLL,
            )
            .await
        }),
        AdminCommand::ListDevices { config } => with_devices(&config, |d, _| async move {
            let list = d.list().await?;
            if list.is_empty() {
                println!("No devices.");
            }
            for dev in list {
                println!(
                    "{}  {:<8}  {}  fingerprint {}",
                    dev.id,
                    dev.state.as_str(),
                    dev.name,
                    dev.fingerprint
                );
            }
            Ok(())
        }),
        AdminCommand::RevokeDevice { config, device } => with_devices(&config, |d, _| async move {
            if d.revoke(&device, "admin").await? {
                println!("Revoked {device}. Its next request is refused.");
                Ok(())
            } else {
                bail!("no paired or pending device {device}")
            }
        }),
        AdminCommand::Openapi => {
            print!("{}", crate::hub::app::openapi::spec_json());
            Ok(())
        }
        other => bail!("admin {other:?} is not implemented yet"),
    }
}

const PAIR_POLL: Duration = Duration::from_millis(500);

/// Runs `f` on the hub's device store; the hub itself may be running on the same database.
fn with_devices<F, Fut>(config: &Path, f: F) -> anyhow::Result<()>
where
    F: FnOnce(Devices, HubConfig) -> Fut,
    Fut: std::future::Future<Output = anyhow::Result<()>>,
{
    let cfg = HubConfig::load(config).with_context(|| format!("{}", config.display()))?;
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?
        .block_on(async move {
            let store = Store::open(&cfg.database)
                .await
                .with_context(|| format!("opening {}", cfg.database.display()))?;
            f(Devices::new(store, Clock::System), cfg).await
        })
}

/// The link a phone scans: the hub's URL, the code and the hub id, which the phone checks.
pub fn pairing_link(public_url: &str, code: &str, hub_id: &str) -> String {
    let mut url = url::Url::parse("switchboard://pair").expect("valid");
    url.query_pairs_mut()
        .append_pair("url", public_url)
        .append_pair("code", code)
        .append_pair("hub", hub_id);
    url.into()
}

/// Shows a code, waits for a phone to claim it, and pairs only if you confirm its fingerprint.
pub async fn pair_device(
    d: &Devices,
    public_url: &str,
    input: &mut impl std::io::BufRead,
    out: &mut impl Write,
    poll: Duration,
) -> anyhow::Result<()> {
    let code = d.issue_code().await?;
    let link = pairing_link(public_url, &code.code, &code.hub_id);
    writeln!(
        out,
        "\nScan this with the Switchboard app (valid for {} minutes, once):\n\n{}\n\nOr paste this link in the app:\n  {link}\n\nOr enter by hand:\n  hub  {public_url}\n  code {}\n",
        CODE_TTL.as_secs() / 60,
        render_qr(&link),
        code.code
    )?;
    let device = loop {
        if let Some(dev) = d.claim_of(&code.code_hash).await? {
            break dev;
        }
        if Clock::System.now_ms() >= code.expires_at {
            bail!("the code expired before a phone used it; run pair-device again");
        }
        tokio::time::sleep(poll).await;
    };
    writeln!(
        out,
        "A phone named {:?} used the code. It shows this fingerprint:\n\n    {}\n\nDoes the phone show exactly the same? Type yes to pair it: ",
        device.name, device.fingerprint
    )?;
    out.flush()?;
    let mut answer = String::new();
    input.read_line(&mut answer)?;
    let accept = answer.trim().eq_ignore_ascii_case("yes");
    d.confirm(&device.id, accept).await?;
    if accept {
        writeln!(
            out,
            "Paired {} ({}). Devices already paired are told.",
            device.name, device.id
        )?;
        Ok(())
    } else {
        writeln!(
            out,
            "Not paired. The code is used up; run pair-device again."
        )?;
        bail!("pairing rejected")
    }
}

fn render_qr(text: &str) -> String {
    use qrcode::render::unicode::Dense1x2;
    match qrcode::QrCode::new(text.as_bytes()) {
        // Inverted so it scans on a dark terminal background.
        Ok(qr) => qr
            .render::<Dense1x2>()
            .dark_color(Dense1x2::Light)
            .light_color(Dense1x2::Dark)
            .quiet_zone(true)
            .build(),
        Err(e) => format!("(could not render the QR code: {e})"),
    }
}

/// Writes a fresh token to a new 0600 file and returns its hash. The token itself is never printed.
fn mint_runner_token(out: &Path) -> anyhow::Result<String> {
    use std::os::unix::fs::OpenOptionsExt;
    let token = format!("{}{}", random_id(), random_id());
    let mut f = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(out)
        .with_context(|| {
            format!(
                "creating {} (an existing token file is never overwritten)",
                out.display()
            )
        })?;
    f.write_all(token.as_bytes())?;
    f.sync_all()?;
    Ok(hash_token(&token))
}

/// Folders never searched for repos.
const SKIP: &[&str] = &["node_modules", "target", "vendor"];
const MAX_DEPTH: usize = 4;

fn is_repo(dir: &Path) -> bool {
    let git = dir.join(".git");
    git.is_dir() || std::fs::read_to_string(&git).is_ok_and(|s| s.starts_with("gitdir:"))
}

/// Git repos under each root, nested ones included, as (root, repo path), sorted.
pub fn find_repos(roots: &[PathBuf]) -> anyhow::Result<Vec<(PathBuf, PathBuf)>> {
    let mut out = Vec::new();
    for root in roots {
        let root = root
            .canonicalize()
            .with_context(|| format!("{}", root.display()))?;
        let mut stack = vec![(root.clone(), 0)];
        while let Some((dir, depth)) = stack.pop() {
            if depth > 0 && is_repo(&dir) {
                out.push((root.clone(), dir.clone()));
            }
            if depth == MAX_DEPTH {
                continue;
            }
            let Ok(entries) = std::fs::read_dir(&dir) else {
                continue;
            };
            for e in entries.flatten() {
                let name = e.file_name().to_string_lossy().into_owned();
                if name.starts_with('.') || SKIP.contains(&name.as_str()) {
                    continue;
                }
                if e.file_type().is_ok_and(|t| t.is_dir()) {
                    stack.push((e.path(), depth + 1));
                }
            }
        }
    }
    out.sort();
    Ok(out)
}

/// A folder name as a project id: `[a-z][a-z0-9-]{0,31}`.
fn project_id(name: &str) -> String {
    let mut id = String::new();
    for c in name.chars() {
        let c = if c.is_ascii_alphanumeric() {
            c.to_ascii_lowercase()
        } else {
            '-'
        };
        if !(c == '-' && (id.is_empty() || id.ends_with('-'))) {
            id.push(c);
        }
    }
    let id = id.trim_end_matches('-');
    let id = if id.starts_with(|c: char| c.is_ascii_lowercase()) {
        id.to_owned()
    } else {
        format!("p-{id}")
    };
    id.chars()
        .take(32)
        .collect::<String>()
        .trim_end_matches('-')
        .to_owned()
}

/// Each repo's id is its folder name. Names found more than once are qualified by the path
/// from the root's parent, so `personal/foo` and `work/foo` become `personal-foo` and `work-foo`.
pub fn assign_ids(repos: &[(PathBuf, PathBuf)]) -> anyhow::Result<Vec<String>> {
    let bare: Vec<String> = repos
        .iter()
        .map(|(_, p)| project_id(&p.file_name().unwrap_or_default().to_string_lossy()))
        .collect();
    let mut count: HashMap<&str, usize> = HashMap::new();
    for id in &bare {
        *count.entry(id.as_str()).or_default() += 1;
    }
    let ids: Vec<String> = repos
        .iter()
        .zip(&bare)
        .map(|((root, path), id)| {
            if count[id.as_str()] == 1 {
                return id.clone();
            }
            let base = root.parent().unwrap_or(root);
            project_id(&path.strip_prefix(base).unwrap_or(path).to_string_lossy())
        })
        .collect();
    let mut seen = HashMap::new();
    for (id, (_, path)) in ids.iter().zip(repos) {
        if let Some(other) = seen.insert(id.as_str(), path) {
            bail!(
                "{} and {} both become project {id:?}; add them to hub.toml by hand",
                other.display(),
                path.display()
            );
        }
    }
    Ok(ids)
}

pub fn projects_toml(machine: &str, repos: &[(PathBuf, PathBuf)]) -> anyhow::Result<String> {
    let home = std::env::var_os("HOME").map(PathBuf::from);
    let mut out = String::new();
    for (id, (_, path)) in assign_ids(repos)?.iter().zip(repos) {
        let shown = match home.as_deref().and_then(|h| path.strip_prefix(h).ok()) {
            Some(rel) => format!("~/{}", rel.display()),
            None => path.display().to_string(),
        };
        out.push_str(&format!(
            "[[projects]]\nid = \"{id}\"\ndescription = \"Git repo at {shown}\"\nrepos = {{ {machine} = {:?} }}\n\n",
            path.display().to_string()
        ));
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn folder_names_become_valid_ids() {
        assert_eq!(project_id("switchboard"), "switchboard");
        assert_eq!(project_id("DPv2-apps-helm"), "dpv2-apps-helm");
        assert_eq!(project_id("lib_utils.rs"), "lib-utils-rs");
        assert_eq!(project_id("2fa--tool"), "p-2fa-tool");
        assert_eq!(project_id(&"x".repeat(40)).len(), 32);
    }

    #[test]
    fn colliding_names_are_qualified_by_tree() {
        let r =
            |root: &str, rel: &str| (PathBuf::from(root), PathBuf::from(format!("{root}/{rel}")));
        let repos = vec![
            r("/h/personal", "foo"),
            r("/h/personal", "switchboard"),
            r("/h/work", "foo"),
            r("/h/work", "inbound-se/hubspot-form-demo"),
        ];
        assert_eq!(
            assign_ids(&repos).unwrap(),
            [
                "personal-foo",
                "switchboard",
                "work-foo",
                "hubspot-form-demo"
            ]
        );
        let cfg = HubConfig::parse(&format!(
            "public_url = \"http://localhost:1\"\nlisten = \"127.0.0.1:1\"\ndatabase = \"/d\"\n\
             [[machines]]\nid = \"mac\"\ntoken_hash = \"{}\"\n{}",
            "0".repeat(64),
            projects_toml("mac", &repos).unwrap()
        ))
        .unwrap();
        assert_eq!(cfg.projects.len(), 4);
        assert_eq!(
            cfg.project("work-foo").unwrap().repos["mac"],
            Path::new("/h/work/foo")
        );
    }

    #[test]
    fn finds_nested_repos_and_skips_hidden_and_marker_files() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("work");
        for d in [
            "a/.git",
            "a/nested/.git",
            "b/src",
            ".cache/c/.git",
            "node_modules/d/.git",
        ] {
            std::fs::create_dir_all(root.join(d)).unwrap();
        }
        std::fs::create_dir_all(root.join("wt")).unwrap();
        std::fs::write(root.join("wt/.git"), "gitdir: /x/.git/worktrees/wt\n").unwrap();
        std::fs::create_dir_all(root.join("empty-marker")).unwrap();
        std::fs::write(root.join("empty-marker/.git"), "").unwrap();
        let found: Vec<String> = find_repos(std::slice::from_ref(&root))
            .unwrap()
            .into_iter()
            .map(|(_, p)| {
                p.strip_prefix(root.canonicalize().unwrap())
                    .unwrap()
                    .display()
                    .to_string()
            })
            .collect();
        assert_eq!(found, ["a", "a/nested", "wt"]);
    }

    #[derive(Clone, Default)]
    struct Shared(std::sync::Arc<std::sync::Mutex<Vec<u8>>>);

    impl Write for Shared {
        fn write(&mut self, b: &[u8]) -> std::io::Result<usize> {
            self.0.lock().unwrap().extend_from_slice(b);
            Ok(b.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    #[tokio::test]
    async fn pairing_waits_for_the_claim_and_pairs_only_on_yes() {
        use crate::auth::device::ClaimRequest;
        use crate::auth::device::tests::Phone;
        use crate::store::DeviceState;
        for (answer, want) in [
            ("yes\n", DeviceState::Active),
            ("no\n", DeviceState::Rejected),
        ] {
            let d = Devices::new(Store::open_in_memory().await.unwrap(), Clock::System);
            let phone = Phone::new();
            let (a, b) = (phone.auth.public(), phone.approval.public());
            let out = Shared::default();
            let mut input = std::io::Cursor::new(answer.as_bytes());
            let phone_side = async {
                let code = loop {
                    tokio::time::sleep(Duration::from_millis(5)).await;
                    let text = String::from_utf8(out.0.lock().unwrap().clone()).unwrap();
                    if let Some(rest) = text.split("  code ").nth(1) {
                        break rest.lines().next().unwrap().to_owned();
                    }
                };
                let claim = ClaimRequest {
                    code: &code,
                    name: "Test phone",
                    auth_key: &a,
                    approval_key: &b,
                    client_build: None,
                };
                d.claim(claim).await.unwrap()
            };
            let mut w = out.clone();
            let (result, claimed) = tokio::join!(
                pair_device(
                    &d,
                    "https://hub.example",
                    &mut input,
                    &mut w,
                    Duration::from_millis(5)
                ),
                phone_side
            );
            assert_eq!(result.is_ok(), want == DeviceState::Active);
            let text = String::from_utf8(out.0.lock().unwrap().clone()).unwrap();
            assert!(
                text.contains(&claimed.fingerprint),
                "the admin sees the phone's fingerprint"
            );
            assert_eq!(
                d.device(&claimed.device_id).await.unwrap().unwrap().state,
                want
            );
        }
    }

    #[test]
    fn the_pairing_link_carries_the_hub_url_code_and_hub_id() {
        let link = pairing_link("https://hub.example.com", "c0de", "h1");
        let url = url::Url::parse(&link).unwrap();
        let q: HashMap<_, _> = url.query_pairs().into_owned().collect();
        assert_eq!(
            (url.scheme(), url.host_str()),
            ("switchboard", Some("pair"))
        );
        assert_eq!(q["url"], "https://hub.example.com");
        assert_eq!((q["code"].as_str(), q["hub"].as_str()), ("c0de", "h1"));
    }

    #[test]
    fn a_minted_token_is_private_never_overwritten_and_matches_its_hash() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let out = dir.path().join("runner.token");
        let hash = mint_runner_token(&out).unwrap();
        let token = std::fs::read_to_string(&out).unwrap();
        assert_eq!(token.len(), 64);
        assert_eq!(hash, hash_token(&token));
        assert_eq!(
            std::fs::metadata(&out).unwrap().permissions().mode() & 0o777,
            0o600
        );
        assert!(mint_runner_token(&out).is_err());
        assert_eq!(std::fs::read_to_string(&out).unwrap(), token);
    }
}
