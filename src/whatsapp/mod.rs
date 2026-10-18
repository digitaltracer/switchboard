//! `switchboard whatsapp`: follow and prompt Herdr agents from WhatsApp. Notify, read and
//! prompt only; nothing here can send keys to an agent or answer a dialog.

pub mod agents;
pub mod command;
pub mod limits;
pub mod link;
pub mod relay;
pub mod text;
pub mod transport;
pub mod watch;
pub mod watchlist;

use std::path::{Path, PathBuf};

use anyhow::{Context, bail};
use tokio::sync::{mpsc, watch as signal};

use crate::config::{WhatsAppConfig, phone_digits};
use crate::herdr::{Client, session_socket};
use agents::HerdrAgents;
use link::{LinkEnd, LinkOptions, WaTransport};
use relay::Relay;
use watchlist::WatchStore;

pub async fn run(config: PathBuf, pair_phone: Option<String>) -> anyhow::Result<()> {
    let cfg = WhatsAppConfig::load(&config)?;
    let pair_phone = pair_phone.map(|p| phone_digits(&p)).transpose()?;
    if pair_phone
        .as_ref()
        .is_some_and(|p| cfg.allowed_senders.contains(p))
    {
        bail!("--pair-phone is an allowed sender; link the bridge's separate number, not your own");
    }
    // Everything the WhatsApp store creates, WAL and SHM files included, is private.
    // SAFETY: umask only changes this process's file-creation mask.
    unsafe { libc::umask(0o077) };
    prepare_store(&cfg.store_path)?;
    let watch_state = WatchStore::load(&WatchStore::path_for(&cfg.store_path))?;

    let home = PathBuf::from(std::env::var_os("HOME").context("HOME is not set")?);
    let socket = session_socket(&home.join(".config/herdr"), &cfg.herdr_session);
    let version = Client::new(&socket)
        .check_protocol()
        .await
        .with_context(|| format!("Herdr session {:?}", cfg.herdr_session))?;
    tracing::info!(herdr = %version, mute_all = watch_state.state.mute_all, "starting WhatsApp bridge");

    let (status_tx, status_rx) = mpsc::channel(256);
    tokio::spawn(watch::watch(Client::new(&socket), status_tx));

    let transport = WaTransport::default();
    let (inbound_tx, inbound_rx) = mpsc::channel(64);
    let (stop_tx, stop_rx) = signal::channel(false);
    let opts = LinkOptions {
        store: cfg.store_path.clone(),
        pair_phone,
        allowed_senders: cfg.allowed_senders.clone(),
    };
    let link_task = tokio::spawn(link::run(opts, transport.clone(), inbound_tx, stop_rx));

    let mut relay = Relay::new(
        HerdrAgents::new(Client::new(&socket)),
        cfg.allowed_senders,
        watch_state,
    );
    let store = cfg.store_path.display().to_string();
    tokio::select! {
        end = link_task => match end? {
            LinkEnd::Revoked(reason) => bail!(
                "WhatsApp unlinked this device ({reason}). Delete {store} and run the bridge again to link the separate number."
            ),
            LinkEnd::Banned(why) => bail!("WhatsApp temporarily banned the bridge number: {why}. Wait before reconnecting."),
            LinkEnd::WrongAccount => bail!(
                "The linked WhatsApp account is one of your allowed senders, so it is a personal account. \
                 Remove this linked device on that phone, delete {store}, and link the separate number."
            ),
            LinkEnd::Shutdown => Ok(()),
        },
        _ = relay::run(&mut relay, &transport, inbound_rx, status_rx) => Ok(()),
        _ = tokio::signal::ctrl_c() => {
            let _ = stop_tx.send(true);
            tokio::time::sleep(std::time::Duration::from_secs(2)).await;
            Ok(())
        }
    }
}

/// Creates the store's directory as 0700 and tightens an existing store to 0600.
fn prepare_store(store: &Path) -> anyhow::Result<()> {
    use std::os::unix::fs::{DirBuilderExt, PermissionsExt};
    let dir = store
        .parent()
        .context("store_path has no parent directory")?;
    if !dir.exists() {
        std::fs::DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(dir)?;
    }
    let mode = std::fs::metadata(dir)?.permissions().mode() & 0o777;
    if mode & 0o077 != 0 {
        tracing::warn!(dir = %dir.display(), mode = format!("{mode:o}"), "store directory is readable by others");
    }
    if store.exists() {
        std::fs::set_permissions(store, std::fs::Permissions::from_mode(0o600))?;
    }
    link::secure_store_files(store);
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    /// Acceptance check 5: the bridge has no way to send keystrokes to an agent.
    #[test]
    fn no_key_sending_anywhere_in_the_bridge() {
        let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("src/whatsapp");
        let needles = [
            concat!("send", "_keys"),
            concat!("send", "-keys"),
            concat!("Send", "Keys"),
        ];
        for entry in std::fs::read_dir(&dir).unwrap() {
            let path = entry.unwrap().path();
            let text = std::fs::read_to_string(&path).unwrap();
            for n in needles {
                assert!(!text.contains(n), "{} mentions {n}", path.display());
            }
        }
    }

    #[test]
    fn store_files_are_private() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let store = dir.path().join("wa/whatsapp.db");
        super::prepare_store(&store).unwrap();
        for name in ["whatsapp.db", "whatsapp.db-wal"] {
            let p = store.with_file_name(name);
            std::fs::write(&p, b"x").unwrap();
            std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o644)).unwrap();
        }
        super::prepare_store(&store).unwrap();
        let mode = |p: &Path| std::fs::metadata(p).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode(store.parent().unwrap()), 0o700);
        assert_eq!(mode(&store), 0o600);
        assert_eq!(mode(&store.with_file_name("whatsapp.db-wal")), 0o600);
    }
}
