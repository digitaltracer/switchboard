//! The real WhatsApp side: whatsapp-rust 0.7 as a linked device, behind `Transport`.
//! The client reconnects on its own; this loop rebuilds it if it stops for any other reason.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use tokio::sync::{mpsc, watch};
use whatsapp_rust::pair_code::PairCodeOptions;
use whatsapp_rust::prelude::*;
use whatsapp_rust_sqlite_storage::SqliteStore;

use super::selfchat::{SelfChat, check_account};
use super::transport::{Inbound, Transport, jid_phone};
use crate::proto::conn::Backoff;

type Slot = Arc<Mutex<Option<Arc<Client>>>>;

/// Sends through whichever client is currently connected.
#[derive(Clone, Default)]
pub struct WaTransport {
    client: Slot,
}

impl Transport for WaTransport {
    async fn send(&self, to: &str, text: &str) -> Result<Option<String>, String> {
        let client = self
            .client
            .lock()
            .unwrap()
            .clone()
            .ok_or("WhatsApp is not connected")?;
        client
            .send_message(Jid::pn(to), wa::Message::text(text))
            .await
            .map(|sent| Some(sent.message_id))
            .map_err(|e| e.to_string())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LinkEnd {
    /// The phone unlinked this device, or WhatsApp logged it out.
    Revoked(String),
    Banned(String),
    /// The linked account is one of the allowed senders, i.e. a personal account.
    WrongAccount,
    /// Self-chat mode, but the linked account isn't the configured number.
    NotOwnAccount,
    Shutdown,
}

pub struct LinkOptions {
    pub store: PathBuf,
    /// The bridge's own number, for pair-code linking instead of a QR code.
    pub pair_phone: Option<String>,
    pub allowed_senders: Vec<String>,
    /// Set in self-chat mode; learns your privacy id when the link connects.
    pub self_chat: Option<Arc<SelfChat>>,
}

pub async fn run(
    opts: LinkOptions,
    transport: WaTransport,
    inbound: mpsc::Sender<Inbound>,
    mut shutdown: watch::Receiver<bool>,
) -> LinkEnd {
    let mut backoff = Backoff::new(Duration::from_secs(1), Duration::from_secs(60));
    loop {
        let end: Arc<Mutex<Option<LinkEnd>>> = Arc::default();
        if let Err(e) = connect_once(&opts, &transport, &inbound, &end, &mut shutdown).await {
            tracing::warn!(error = %e, "WhatsApp client stopped");
        }
        *transport.client.lock().unwrap() = None;
        if let Some(end) = end.lock().unwrap().take() {
            return end;
        }
        if *shutdown.borrow() {
            return LinkEnd::Shutdown;
        }
        let delay = backoff.next_delay();
        tracing::info!(?delay, "reconnecting to WhatsApp");
        tokio::time::sleep(delay).await;
    }
}

async fn connect_once(
    opts: &LinkOptions,
    transport: &WaTransport,
    inbound: &mpsc::Sender<Inbound>,
    end: &Arc<Mutex<Option<LinkEnd>>>,
    shutdown: &mut watch::Receiver<bool>,
) -> Result<(), String> {
    let path = opts.store.to_str().ok_or("store_path is not valid UTF-8")?;
    let store = SqliteStore::new(path)
        .await
        .map_err(|e| format!("opening WhatsApp store: {e}"))?;
    secure_store_files(&opts.store);

    let (slot, allowed, end_c, end_l, end_b) = (
        transport.client.clone(),
        opts.allowed_senders.clone(),
        end.clone(),
        end.clone(),
        end.clone(),
    );
    let self_chat = opts.self_chat.clone();
    let whose_phone = if self_chat.is_some() {
        "your own phone"
    } else {
        "the phone that holds the bridge's separate number"
    };
    let inbound = inbound.clone();
    let store_path = opts.store.clone();
    let mut builder = Bot::builder()
        .with_backend(store)
        .skip_history_sync()
        .on_qr_code(move |code, timeout| async move { print_qr(&code, timeout, whose_phone) })
        .on_pair_code(
            move |code, timeout| async move { print_pair_code(&code, timeout, whose_phone) },
        )
        .on_connected(move |client| {
            let (slot, allowed, end, store_path, self_chat) = (
                slot.clone(),
                allowed.clone(),
                end_c.clone(),
                store_path.clone(),
                self_chat.clone(),
            );
            async move {
                let own = client.pn().and_then(|j| jid_phone(&j.to_string()));
                if let Err(refused) = check_account(own.as_deref(), &allowed, self_chat.is_some()) {
                    *end.lock().unwrap() = Some(refused);
                    client.disconnect().await;
                    return;
                }
                if let (Some(sc), Some(lid)) = (&self_chat, client.lid()) {
                    sc.set_own_lid(&lid.to_string());
                }
                secure_store_files(&store_path);
                *slot.lock().unwrap() = Some(client);
                tracing::info!("WhatsApp connected");
            }
        })
        .on_logged_out(move |info| {
            let end = end_l.clone();
            async move {
                *end.lock().unwrap() = Some(LinkEnd::Revoked(format!("{:?}", info.reason)));
            }
        })
        .on_event_for(&[EventKind::TemporaryBan], move |event, client| {
            let end = end_b.clone();
            async move {
                if let Event::TemporaryBan(ban) = &*event {
                    *end.lock().unwrap() = Some(LinkEnd::Banned(format!(
                        "{:?} for {:?}",
                        ban.code, ban.expire
                    )));
                    client.disconnect().await;
                }
            }
        })
        .on_event_for(&[EventKind::Disconnected], |_event, _client| async {
            tracing::info!("WhatsApp disconnected; the client will reconnect");
        })
        .on_message(move |ctx| {
            let inbound = inbound.clone();
            async move {
                let Some(text) = ctx.message.text_content() else {
                    return;
                };
                let source = &ctx.info.source;
                let msg = Inbound {
                    id: ctx.info.id.to_string(),
                    chat: source.chat.to_string(),
                    sender: source.sender.to_string(),
                    sender_alt: source.sender_alt.as_ref().map(ToString::to_string),
                    is_group: source.is_group,
                    from_me: source.is_from_me,
                    text: text.to_owned(),
                };
                let _ = inbound.send(msg).await;
            }
        });
    if let Some(phone) = &opts.pair_phone {
        builder = builder.with_pair_code(PairCodeOptions {
            phone_number: phone.clone(),
            ..Default::default()
        });
    }
    let bot = builder
        .build()
        .await
        .map_err(|e| format!("starting WhatsApp client: {e}"))?;
    let mut handle = bot.spawn();
    tokio::select! {
        _ = &mut handle => Ok(()),
        _ = shutdown.changed() => {
            handle.shutdown().await;
            Ok(())
        }
    }
}

/// Keeps the credential and its WAL/SHM side files private.
pub fn secure_store_files(store: &Path) {
    use std::os::unix::fs::PermissionsExt;
    for suffix in ["", "-wal", "-shm", "-journal"] {
        let path = PathBuf::from(format!("{}{suffix}", store.display()));
        if path.exists() {
            let _ = std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600));
        }
    }
}

fn print_qr(code: &str, timeout: Duration, whose_phone: &str) {
    use qrcode::render::unicode::Dense1x2;
    let rendered = match qrcode::QrCode::new(code.as_bytes()) {
        // Inverted so it scans on a dark terminal background.
        Ok(qr) => qr
            .render::<Dense1x2>()
            .dark_color(Dense1x2::Light)
            .light_color(Dense1x2::Dark)
            .quiet_zone(true)
            .build(),
        Err(e) => format!("(could not render the QR code: {e})"),
    };
    println!(
        "\nScan this with {whose_phone}:\n\
         WhatsApp > Settings > Linked devices > Link a device. Valid for {}s.\n\n{rendered}\n",
        timeout.as_secs()
    );
}

fn print_pair_code(code: &str, timeout: Duration, whose_phone: &str) {
    println!(
        "\nPair code (valid for {}s): {code}\n\
         On {whose_phone}: WhatsApp > Settings > Linked devices >\n\
         Link a device > Link with phone number instead, then enter the code.\n",
        timeout.as_secs()
    );
}
