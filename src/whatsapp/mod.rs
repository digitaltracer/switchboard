//! The WhatsApp bridge, a front door to the hub next to MCP. It runs inside the hub process and
//! calls the session service: starts, prompts, reads, approvals and the shared mute state.
//! Nothing here can send keys to an agent; approvals go through the structured permission hooks.

pub mod command;
pub mod limits;
pub mod link;
pub mod relay;
/// The hub's routing, shared by every front door.
pub use crate::hub::route;
pub mod selfchat;
pub mod text;
pub mod transport;
pub mod watchlist;

use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, bail};
use tokio::sync::{mpsc, watch as signal};

use crate::config::{WhatsAppConfig, phone_digits};
use crate::hub::service::{Caller, SessionService};
use link::{LinkEnd, LinkOptions, WaTransport};
use relay::Relay;
use selfchat::SelfChat;

/// Runs the bridge until the hub shuts down or the link ends for good.
pub async fn run(
    svc: Arc<SessionService>,
    cfg: WhatsAppConfig,
    pair_phone: Option<String>,
) -> anyhow::Result<()> {
    let pair_phone = pair_phone.map(|p| phone_digits(&p)).transpose()?;
    match (&pair_phone, cfg.self_chat) {
        (Some(p), false) if cfg.allowed_senders.contains(p) => bail!(
            "--whatsapp-pair-phone is an allowed sender; link the bridge's separate number, not your own"
        ),
        (Some(p), true) if !cfg.allowed_senders.contains(p) => bail!(
            "self_chat is on, so --whatsapp-pair-phone must be your own number, the allowed sender"
        ),
        _ => {}
    }
    // Everything the WhatsApp store creates, WAL and SHM files included, is private.
    // SAFETY: umask only changes this process's file-creation mask.
    unsafe { libc::umask(0o077) };
    prepare_store(&cfg.store_path)?;
    import_legacy_watch(&svc, &cfg.store_path).await?;
    tracing::info!("starting WhatsApp bridge");

    let transport = WaTransport::default();
    let (inbound_tx, inbound_rx) = mpsc::channel(64);
    let (stop_tx, stop_rx) = signal::channel(false);
    let self_chat = cfg.self_chat.then(|| {
        tracing::warn!(
            "self-chat mode: this linked device receives all your chats; only your own chat is read"
        );
        Arc::new(SelfChat::new(cfg.allowed_senders[0].clone()))
    });
    let opts = LinkOptions {
        store: cfg.store_path.clone(),
        pair_phone,
        allowed_senders: cfg.allowed_senders.clone(),
        self_chat: self_chat.clone(),
    };
    let link_task = tokio::spawn(link::run(opts, transport.clone(), inbound_tx, stop_rx));

    let events = svc.subscribe();
    let mut shutdown = svc.shutdown_signal();
    let stopping = async move {
        let _ = shutdown.wait_for(|stop| *stop).await;
    };
    let mut relay = match self_chat {
        Some(sc) => Relay::self_chat(svc, sc),
        None => Relay::new(svc, cfg.allowed_senders),
    };
    let store = cfg.store_path.display().to_string();
    tokio::select! {
        end = link_task => match end? {
            LinkEnd::Revoked(reason) => bail!(
                "WhatsApp unlinked this device ({reason}). Delete {store} and start the hub again to link it."
            ),
            LinkEnd::Banned(why) => bail!("WhatsApp temporarily banned the bridge number: {why}. Wait before reconnecting."),
            LinkEnd::WrongAccount => bail!(
                "The linked WhatsApp account is one of your allowed senders, so it is a personal account. \
                 Remove this linked device on that phone, delete {store}, and link the separate number."
            ),
            LinkEnd::NotOwnAccount => bail!(
                "self_chat is on, but the linked WhatsApp account isn't your allowed sender's number. \
                 Remove this linked device on that phone, delete {store}, and link your own account."
            ),
            LinkEnd::Shutdown => Ok(()),
        },
        _ = relay::run(&mut relay, &transport, inbound_rx, events) => Ok(()),
        _ = stopping => {
            let _ = stop_tx.send(true);
            tokio::time::sleep(Duration::from_secs(2)).await;
            Ok(())
        }
    }
}

/// Moves the old `whatsapp-watch.json` choices into the hub's store; the hub takes them once.
async fn import_legacy_watch(svc: &SessionService, store_path: &Path) -> anyhow::Result<()> {
    let path = watchlist::legacy_path(store_path);
    let Some(state) = watchlist::load_legacy(&path)? else {
        return Ok(());
    };
    let imported = svc
        .import_watch_state(&Caller::new("whatsapp"), &state)
        .await
        .with_context(|| format!("importing {}", path.display()))?;
    if imported {
        tracing::info!(file = %path.display(), "imported the bridge's old mute and watch choices");
    }
    Ok(())
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

    /// The bridge has no way to send keystrokes to an agent, and neither has the runner code
    /// that reads and prompts agents for it.
    #[test]
    fn no_key_sending_anywhere_in_the_bridge() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR"));
        let needles = [
            concat!("send", "_keys"),
            concat!("send", "-keys"),
            concat!("Send", "Keys"),
        ];
        let bridge = std::fs::read_dir(root.join("src/whatsapp"))
            .unwrap()
            .map(|e| e.unwrap().path());
        let runner = ["agents.rs", "agents_tests.rs"].map(|f| root.join("src/runner").join(f));
        for path in bridge.chain(runner) {
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
