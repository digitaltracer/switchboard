//! The chat connection behind a small trait, so the relay can be tested without a network.

use std::future::Future;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};

/// An incoming text message. Senders are JID strings such as `447700900123@s.whatsapp.net`
/// or a privacy `…@lid`; `sender_alt` carries the phone-number JID when WhatsApp provides it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Inbound {
    pub sender: String,
    pub sender_alt: Option<String>,
    pub is_group: bool,
    pub from_me: bool,
    pub text: String,
}

pub trait Transport: Send + Sync + 'static {
    /// Sends a text to the phone number `to` (digits only).
    fn send(&self, to: &str, text: &str) -> impl Future<Output = Result<(), String>> + Send;
}

/// Digits of a phone-number JID, ignoring any `:device` suffix; `None` for LIDs and groups.
pub fn jid_phone(jid: &str) -> Option<String> {
    let (user, server) = jid.split_once('@')?;
    if server != "s.whatsapp.net" && server != "c.us" {
        return None;
    }
    let user = user.split([':', '.']).next()?;
    (!user.is_empty() && user.bytes().all(|b| b.is_ascii_digit())).then(|| user.to_owned())
}

/// Records sends instead of delivering them.
#[derive(Default)]
pub struct FakeTransport {
    sent: Mutex<Vec<(String, String)>>,
    down: AtomicBool,
}

impl FakeTransport {
    pub fn sent(&self) -> Vec<(String, String)> {
        self.sent.lock().unwrap().clone()
    }

    pub fn take(&self) -> Vec<(String, String)> {
        std::mem::take(&mut *self.sent.lock().unwrap())
    }

    /// While down, every send fails as if the link had dropped.
    pub fn set_down(&self, down: bool) {
        self.down.store(down, Ordering::SeqCst);
    }
}

impl Transport for FakeTransport {
    async fn send(&self, to: &str, text: &str) -> Result<(), String> {
        if self.down.load(Ordering::SeqCst) {
            return Err("not connected".into());
        }
        self.sent
            .lock()
            .unwrap()
            .push((to.to_owned(), text.to_owned()));
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn phone_jids() {
        assert_eq!(
            jid_phone("447700900123@s.whatsapp.net").as_deref(),
            Some("447700900123")
        );
        assert_eq!(
            jid_phone("447700900123:12@s.whatsapp.net").as_deref(),
            Some("447700900123")
        );
        assert_eq!(
            jid_phone("447700900123@c.us").as_deref(),
            Some("447700900123")
        );
        assert_eq!(jid_phone("123456789012345@lid"), None);
        assert_eq!(jid_phone("120363000000000000@g.us"), None);
        assert_eq!(jid_phone("abc@s.whatsapp.net"), None);
        assert_eq!(jid_phone("no-at-sign"), None);
    }
}
