//! Chat logic: allowlist, commands, prompts and notifications. Knows nothing about WhatsApp
//! or Herdr beyond the `Transport` and `AgentControl` traits.

use std::time::Duration;

use tokio::sync::mpsc;
use tokio::time::Instant;

use super::agents::{AgentControl, AgentSummary, PromptOutcome};
use super::command::{self, Command, Target};
use super::limits::{Debouncer, Notify, Outbox, Outgoing};
use super::text::{fit_one, overfetch, split, tail};
use super::transport::{Inbound, Transport, jid_phone};
use super::watchlist::{SWITCHBOARD_PREFIX, WatchStore};
use crate::herdr::check_agent_name;
use crate::herdr::types::AgentStatus;
use crate::redact::redact_str;

const EXCERPT_LINES: usize = 20;
/// Enough to get above a TUI's input box and status bar to the reply itself.
const FINISHED_LINES: usize = 20;
const TICK: Duration = Duration::from_millis(500);
const BLOCKED_HINT: &str = "Approve or deny it in the Herdr tab or through Claude's remote control. This bridge can't answer dialogs.";

pub struct Relay<A> {
    agents: A,
    allowed: Vec<String>,
    watch: WatchStore,
    selected: Option<String>,
    debouncer: Debouncer,
}

fn status_word(s: AgentStatus) -> &'static str {
    match s {
        AgentStatus::Idle => "idle",
        AgentStatus::Working => "working",
        AgentStatus::Blocked => "blocked (waiting for approval)",
        AgentStatus::Done => "done",
        AgentStatus::Unknown => "unknown",
    }
}

impl<A: AgentControl> Relay<A> {
    pub fn new(agents: A, allowed: Vec<String>, watch: WatchStore) -> Self {
        Self {
            agents,
            allowed,
            watch,
            selected: None,
            debouncer: Debouncer::default(),
        }
    }

    pub fn watch_state(&self) -> &super::watchlist::WatchState {
        &self.watch.state
    }

    pub fn selected(&self) -> Option<&str> {
        self.selected.as_deref()
    }

    /// The allowed sender's number, or `None` to drop the message unanswered.
    pub fn authorize(&self, m: &Inbound) -> Option<String> {
        if m.is_group || m.from_me {
            return None;
        }
        [Some(m.sender.as_str()), m.sender_alt.as_deref()]
            .into_iter()
            .flatten()
            .filter_map(jid_phone)
            .find(|n| self.allowed.contains(n))
    }

    pub async fn handle(&mut self, m: &Inbound) -> Vec<Outgoing> {
        let Some(to) = self.authorize(m) else {
            // Only the sender id is logged, never the content.
            tracing::info!(sender = %m.sender, group = m.is_group, from_me = m.from_me, "dropped message");
            return vec![];
        };
        let texts = match command::parse(&m.text) {
            Command::Help => vec![command::HELP.to_owned()],
            Command::Status => vec![self.status().await],
            Command::List => vec![self.list().await],
            Command::Use(agent) => vec![self.select(&agent).await],
            Command::Read(n) => self.read(n).await,
            Command::Mute(t) => vec![self.mute(t)],
            Command::Unmute(t) => vec![self.unmute(t)],
            Command::Watch(agent) => vec![self.opt_in(&agent).await],
            Command::Prompt(text) => vec![self.prompt(&text).await],
        };
        texts
            .into_iter()
            .map(|text| Outgoing {
                to: to.clone(),
                text,
                about: None,
            })
            .collect()
    }

    async fn live(&self) -> Result<Vec<AgentSummary>, String> {
        self.agents
            .list()
            .await
            .map_err(|e| format!("Couldn't reach Herdr: {e}"))
    }

    fn mute_all_note(&self) -> Option<String> {
        self.watch
            .state
            .mute_all
            .then(|| "All notifications are muted. Send unmute all to resume.".to_owned())
    }

    /// Watched agents: live Switchboard and opted-in agents, plus opted-in ones not running.
    async fn status(&self) -> String {
        let live = match self.live().await {
            Ok(l) => l,
            Err(e) => return e,
        };
        let state = &self.watch.state;
        let mut lines: Vec<String> = self.mute_all_note().into_iter().collect();
        let mut seen = Vec::new();
        for a in &live {
            let Some(name) = a.name.as_deref() else {
                continue;
            };
            if state.is_watched(name) {
                seen.push(name.to_owned());
                lines.push(format!(
                    "{name}: {} · {}",
                    status_word(a.status),
                    state.describe(name)
                ));
            }
        }
        for name in state.watched.iter().filter(|n| !seen.contains(n)) {
            lines.push(format!("{name}: not running · {}", state.describe(name)));
        }
        if lines.len() == usize::from(state.mute_all) {
            lines.push(format!(
                "No watched agents are running. Switchboard's agents ({SWITCHBOARD_PREFIX}...) are watched \
                 automatically; send watch <agent> to add another."
            ));
        }
        lines.join("\n")
    }

    async fn list(&self) -> String {
        let live = match self.live().await {
            Ok(l) => l,
            Err(e) => return e,
        };
        if live.is_empty() {
            return "No agents are running in this Herdr session.".into();
        }
        live.iter()
            .map(|a| {
                let mark = if self.selected.as_deref() == Some(a.label()) {
                    "* "
                } else {
                    ""
                };
                let kind = a
                    .kind
                    .as_deref()
                    .map(|k| format!(" ({k})"))
                    .unwrap_or_default();
                let watch = match a.name.as_deref() {
                    Some(name) => self.watch.state.describe(name),
                    None => "no name, can't be watched",
                };
                format!(
                    "{mark}{}{kind}: {} · {watch}",
                    a.label(),
                    status_word(a.status)
                )
            })
            .chain(self.mute_all_note())
            .collect::<Vec<_>>()
            .join("\n")
    }

    fn save_failed(e: std::io::Error) -> String {
        format!("Couldn't save that, so nothing changed: {e}")
    }

    fn mute(&mut self, target: Target) -> String {
        match target {
            Target::All => match self.watch.update(|s| s.mute_all = true) {
                Ok(()) => "Muted all notifications, including agents created later. Send unmute all to resume.".into(),
                Err(e) => Self::save_failed(e),
            },
            Target::Agent(name) => {
                if check_agent_name(&name).is_err() {
                    return format!("{name} isn't an agent name. Send list to see them.");
                }
                let watched = self.watch.state.is_watched(&name);
                match self.watch.update(|s| {
                    s.muted.insert(name.clone());
                }) {
                    Ok(()) if watched => format!("Muted {name}."),
                    Ok(()) => format!("Muted {name}. It isn't watched, so it wasn't notifying anyway."),
                    Err(e) => Self::save_failed(e),
                }
            }
        }
    }

    fn unmute(&mut self, target: Target) -> String {
        match target {
            Target::All => match self.watch.update(|s| {
                s.mute_all = false;
                s.muted.clear();
            }) {
                Ok(()) => "Notifications are back on for every watched agent.".into(),
                Err(e) => Self::save_failed(e),
            },
            Target::Agent(_) if self.watch.state.mute_all => {
                "All notifications are muted. Send unmute all first.".into()
            }
            Target::Agent(name) => {
                if !self.watch.state.muted.contains(&name) {
                    return format!("{name} wasn't muted.");
                }
                let watched = self.watch.state.is_watched(&name);
                match self.watch.update(|s| {
                    s.muted.remove(&name);
                }) {
                    Ok(()) if watched => format!("Unmuted {name}."),
                    Ok(()) => format!(
                        "Unmuted {name}. It isn't watched; send watch {name} to get its notifications."
                    ),
                    Err(e) => Self::save_failed(e),
                }
            }
        }
    }

    async fn opt_in(&mut self, name: &str) -> String {
        if check_agent_name(name).is_err() {
            return format!(
                "Only named agents can be watched. Name it first: herdr agent rename {name} <name>."
            );
        }
        if name.starts_with(SWITCHBOARD_PREFIX) {
            return format!("{name} is a Switchboard agent, so it's watched already.");
        }
        let live = self
            .live()
            .await
            .map(|l| l.iter().any(|a| a.name.as_deref() == Some(name)))
            .unwrap_or(false);
        match self.watch.update(|s| {
            s.watched.insert(name.to_owned());
            s.muted.remove(name);
        }) {
            Ok(()) => {
                let mut reply = format!("Watching {name}.");
                if !live {
                    reply.push_str(" It isn't running now; you'll hear from it once it is.");
                }
                if self.watch.state.mute_all {
                    reply.push_str(
                        " All notifications are muted, though; send unmute all to resume.",
                    );
                }
                reply
            }
            Err(e) => Self::save_failed(e),
        }
    }

    async fn select(&mut self, agent: &str) -> String {
        let live = match self.live().await {
            Ok(l) => l,
            Err(e) => return e,
        };
        match live
            .iter()
            .find(|a| a.name.as_deref() == Some(agent) || a.pane_id == agent)
        {
            Some(a) => {
                self.selected = Some(a.label().to_owned());
                format!("Messages now go to {}.", a.label())
            }
            None => format!("No live agent called {agent}. Send list to see them."),
        }
    }

    fn target(&self) -> Result<String, String> {
        self.selected
            .clone()
            .ok_or_else(|| "Pick an agent first: use <agent>. Send list to see them.".into())
    }

    async fn read(&self, n: u32) -> Vec<String> {
        let target = match self.target() {
            Ok(t) => t,
            Err(e) => return vec![e],
        };
        match self.agents.read(&target, overfetch(n)).await {
            Ok(text) => {
                // Masked before splitting, so no secret is cut in two and missed.
                let text = tail(&redact_str(&text), n as usize);
                if text.is_empty() {
                    vec![format!("{target} has no output yet.")]
                } else {
                    split(&text)
                }
            }
            Err(e) => vec![format!("Couldn't read {target}: {e}")],
        }
    }

    async fn prompt(&self, text: &str) -> String {
        let target = match self.target() {
            Ok(t) => t,
            Err(e) => return e,
        };
        match self.agents.prompt(&target, text).await {
            Ok(PromptOutcome::Sent) => format!("Sent to {target}."),
            Ok(PromptOutcome::Blocked { screen }) => fit_one(&format!(
                "{target} is waiting at a dialog, so I didn't send that. {BLOCKED_HINT}\n\n{}",
                tail(&redact_str(&screen), EXCERPT_LINES)
            )),
            Ok(PromptOutcome::NotFound) => {
                format!("{target} isn't running any more. Send list to pick another.")
            }
            Err(e) => format!("Couldn't prompt {target}: {e}"),
        }
    }

    /// Every named agent is tracked, so watching or unmuting takes effect at once.
    pub fn observe(&mut self, agent: &str, status: AgentStatus, now: Instant) {
        self.debouncer.observe(agent, status, now);
    }

    /// Notifications whose status has held long enough, for agents that notify right now,
    /// one message per allowed sender.
    pub async fn due(&mut self, now: Instant) -> Vec<Outgoing> {
        let mut out = Vec::new();
        let due = self.debouncer.due(now);
        for (agent, notify) in due
            .into_iter()
            .filter(|(a, _)| self.watch.state.notifies(a))
        {
            let text = match notify {
                Notify::Blocked => {
                    let screen = self.agents.screen(&agent).await.unwrap_or_default();
                    format!(
                        "{agent} is waiting for approval:\n\n{}\n\n{BLOCKED_HINT}",
                        tail(&redact_str(&screen), EXCERPT_LINES)
                    )
                }
                Notify::Finished => {
                    let output = self
                        .agents
                        .read(&agent, overfetch(FINISHED_LINES as u32))
                        .await
                        .unwrap_or_default();
                    format!(
                        "{agent} finished:\n\n{}",
                        tail(&redact_str(&output), FINISHED_LINES)
                    )
                }
            };
            let text = fit_one(&text);
            for to in &self.allowed {
                out.push(Outgoing {
                    to: to.clone(),
                    text: text.clone(),
                    about: Some(agent.clone()),
                });
            }
        }
        out
    }
}

/// Runs the relay until the inbound channel closes.
pub async fn run<A: AgentControl, T: Transport>(
    relay: &mut Relay<A>,
    transport: &T,
    mut inbound: mpsc::Receiver<Inbound>,
    mut statuses: mpsc::Receiver<(String, AgentStatus)>,
) {
    let mut outbox = Outbox::new(Instant::now());
    let mut tick = tokio::time::interval(TICK);
    let mut statuses_open = true;
    loop {
        tokio::select! {
            m = inbound.recv() => match m {
                Some(m) => for o in relay.handle(&m).await { outbox.push(o) },
                None => break,
            },
            s = statuses.recv(), if statuses_open => match s {
                Some((agent, status)) => relay.observe(&agent, status, Instant::now()),
                None => statuses_open = false,
            },
            _ = tick.tick() => for o in relay.due(Instant::now()).await { outbox.push(o) },
        }
        while let Some(o) = outbox.pop_ready(Instant::now()) {
            // Every message leaves the machine; mask once more in case a reply quoted a secret.
            if let Err(e) = transport.send(&o.to, &redact_str(&o.text)).await {
                tracing::warn!(error = %e, "send failed; will retry");
                outbox.requeue(o);
                break;
            }
        }
    }
}

#[cfg(test)]
#[path = "relay_tests.rs"]
mod tests;
