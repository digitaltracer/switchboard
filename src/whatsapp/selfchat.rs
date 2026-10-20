//! Self-chat mode: the bridge is linked to your own account and listens only in your
//! "Message yourself" chat. Every other chat is ignored, and the bridge's own messages,
//! which come back as messages from you, are recognised by id and by a marker.

use std::collections::VecDeque;
use std::sync::Mutex;

use super::link::LinkEnd;
use super::transport::Inbound;

/// Starts every message the bridge sends in self-chat mode.
pub const MARKER: &str = "🤖 ";
/// How many sent message ids are remembered for echo detection.
const SENT_IDS: usize = 1024;

pub struct SelfChat {
    own_pn: String,
    /// The user part of your privacy id (`…@lid`), learned when the link connects.
    own_lid: Mutex<Option<String>>,
    sent: Mutex<VecDeque<String>>,
}

/// The user part of a JID without any `:device` or `.agent` suffix, and its server.
fn split_jid(jid: &str) -> Option<(&str, &str)> {
    let (user, server) = jid.split_once('@')?;
    Some((user.split([':', '.']).next()?, server))
}

impl SelfChat {
    /// `own_pn` is your number in digits, the single allowed sender.
    pub fn new(own_pn: impl Into<String>) -> Self {
        Self {
            own_pn: own_pn.into(),
            own_lid: Mutex::default(),
            sent: Mutex::default(),
        }
    }

    pub fn own_pn(&self) -> &str {
        &self.own_pn
    }

    pub fn set_own_lid(&self, lid_jid: &str) {
        *self.own_lid.lock().unwrap() = split_jid(lid_jid).map(|(user, _)| user.to_owned());
    }

    /// Your "Message yourself" chat, by phone JID or by privacy id.
    pub fn is_self_chat(&self, chat: &str) -> bool {
        match split_jid(chat) {
            Some((user, "s.whatsapp.net" | "c.us")) => user == self.own_pn,
            Some((user, "lid")) => self.own_lid.lock().unwrap().as_deref() == Some(user),
            _ => false,
        }
    }

    pub fn record_sent(&self, id: String) {
        let mut sent = self.sent.lock().unwrap();
        if sent.len() == SENT_IDS {
            sent.pop_front();
        }
        sent.push_back(id);
    }

    pub fn was_sent(&self, id: &str) -> bool {
        self.sent.lock().unwrap().iter().any(|s| s == id)
    }

    /// A message you typed in your own chat, and not one of the bridge's own coming back.
    pub fn accepts(&self, m: &Inbound) -> bool {
        !m.is_group
            && m.from_me
            && self.is_self_chat(&m.chat)
            && !self.was_sent(&m.id)
            && !m.text.trim_start().starts_with(MARKER.trim_end())
    }

    pub fn mark(text: &str) -> String {
        format!("{MARKER}{text}")
    }
}

/// Which account may be linked. In self-chat mode only your own; otherwise never an allowed
/// sender's, because that would be a personal account.
pub fn check_account(
    own_pn: Option<&str>,
    allowed: &[String],
    self_chat: bool,
) -> Result<(), LinkEnd> {
    if self_chat {
        return match (own_pn, allowed) {
            (Some(own), [only]) if own == only => Ok(()),
            _ => Err(LinkEnd::NotOwnAccount),
        };
    }
    if own_pn.is_some_and(|n| allowed.iter().any(|a| a == n)) {
        return Err(LinkEnd::WrongAccount);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    const ME: &str = "447700900123";

    fn msg(chat: &str, text: &str) -> Inbound {
        Inbound {
            id: "m1".into(),
            chat: chat.into(),
            sender: format!("{ME}:2@s.whatsapp.net"),
            sender_alt: None,
            is_group: false,
            from_me: true,
            text: text.into(),
        }
    }

    #[test]
    fn only_your_own_chat_by_phone_or_privacy_id() {
        let sc = SelfChat::new(ME);
        assert!(sc.accepts(&msg(&format!("{ME}@s.whatsapp.net"), "status")));
        assert!(
            !sc.accepts(&msg("123456789012345@lid", "status")),
            "lid unknown yet"
        );
        sc.set_own_lid("123456789012345:7@lid");
        assert!(sc.accepts(&msg("123456789012345@lid", "status")));
        // Your own message in someone else's chat, a group, or a message from someone else.
        assert!(!sc.accepts(&msg("15550001111@s.whatsapp.net", "status")));
        assert!(!sc.accepts(&msg("999999999999999@lid", "status")));
        let group = Inbound {
            is_group: true,
            ..msg("120363000000000000@g.us", "status")
        };
        assert!(!sc.accepts(&group));
        let theirs = Inbound {
            from_me: false,
            ..msg(&format!("{ME}@s.whatsapp.net"), "status")
        };
        assert!(!sc.accepts(&theirs));
    }

    #[test]
    fn the_bridges_own_messages_are_ignored_by_id_and_by_marker() {
        let sc = SelfChat::new(ME);
        let chat = format!("{ME}@s.whatsapp.net");
        sc.record_sent("m1".into());
        assert!(!sc.accepts(&msg(&chat, "sb-1: idle")), "by id");
        let marked = Inbound {
            id: "m2".into(),
            ..msg(&chat, &SelfChat::mark("sb-1: idle"))
        };
        assert!(!sc.accepts(&marked), "by marker");
        let typed = Inbound {
            id: "m3".into(),
            ..msg(&chat, "status")
        };
        assert!(sc.accepts(&typed));
    }

    #[test]
    fn sent_ids_are_bounded() {
        let sc = SelfChat::new(ME);
        for i in 0..SENT_IDS + 1 {
            sc.record_sent(format!("id-{i}"));
        }
        assert!(!sc.was_sent("id-0") && sc.was_sent(&format!("id-{SENT_IDS}")));
    }

    #[test]
    fn account_checks() {
        let one = vec![ME.to_string()];
        let two = vec![ME.to_string(), "15550001111".to_string()];
        assert_eq!(check_account(Some(ME), &one, true), Ok(()));
        assert_eq!(
            check_account(Some("15550001111"), &one, true),
            Err(LinkEnd::NotOwnAccount)
        );
        assert_eq!(check_account(None, &one, true), Err(LinkEnd::NotOwnAccount));
        assert_eq!(
            check_account(Some(ME), &two, true),
            Err(LinkEnd::NotOwnAccount)
        );
        // Unchanged without self-chat: an allowed sender's account is refused.
        assert_eq!(
            check_account(Some(ME), &one, false),
            Err(LinkEnd::WrongAccount)
        );
        assert_eq!(check_account(Some("15550002222"), &one, false), Ok(()));
    }
}
