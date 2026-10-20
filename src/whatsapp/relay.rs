//! Chat logic: allowlist, routing, commands, approvals and notifications, on top of the hub's
//! session service. Knows nothing about WhatsApp beyond the `Transport` trait.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::Duration;

use serde_json::Value;
use tokio::sync::{broadcast, mpsc};
use tokio::time::Instant;

use super::command::{self, Command, Reuse, Target as Mute};
use super::limits::{Debouncer, Notify, Outbox, Outgoing};
use super::route::{self, Live, Target};
use super::selfchat::SelfChat;
use super::text::{fit_one, overfetch, split, tail};
use super::transport::{Inbound, Transport, jid_phone};
use super::watchlist::{SWITCHBOARD_PREFIX, WatchState};
use crate::herdr::check_agent_name;
use crate::herdr::types::AgentStatus;
use crate::hub::service::{
    ApprovalView, Caller, DecideResult, HubError, HubEvent, SessionService, StartRequest,
    StartResult,
};
use crate::proto::{AgentPromptOutcome, ErrorCode};
use crate::redact::redact_str;
use crate::store::{Decision, Delivery, Lifecycle};

const EXCERPT_LINES: usize = 20;
/// Enough to get above a TUI's input box and status bar to the reply itself.
const FINISHED_LINES: usize = 20;
const MAX_INPUT_CHARS: usize = 1500;
const TICK: Duration = Duration::from_millis(500);
const FOREIGN_DIALOG: &str =
    "Answer it in its Herdr tab; Switchboard can't answer dialogs for agents it didn't start.";
const SESSION_DIALOG: &str =
    "It isn't a permission request Switchboard can answer, so answer it in the Herdr tab.";

/// A numbered question waiting for its answer.
enum Choice {
    Project {
        options: Vec<String>,
        prompt: String,
        machine: Option<String>,
    },
    Target {
        options: Vec<Target>,
        then: Then,
    },
}

enum Then {
    Use(Option<String>),
    Stop,
}

impl Choice {
    fn len(&self) -> usize {
        match self {
            Choice::Project { options, .. } => options.len(),
            Choice::Target { options, .. } => options.len(),
        }
    }
}

enum Resolved {
    One(Target),
    Many(Vec<Target>),
    Nothing(String),
}

/// A start finished in the background.
pub struct Started {
    to: String,
    result: Result<StartResult, String>,
}

pub struct Relay {
    svc: Arc<SessionService>,
    caller: Caller,
    allowed: Vec<String>,
    debouncer: Debouncer,
    /// Set in self-chat mode: only your own "Message yourself" chat is read.
    self_chat: Option<Arc<SelfChat>>,
    dropped: u64,
    /// Where text that names no project goes.
    current: Option<Target>,
    pending: Option<Choice>,
    /// The last `sessions` listing, for `use <n>` and `stop <n>`.
    listing: Vec<Target>,
    /// The machine each agent was last reported on.
    machines: HashMap<String, String>,
    started_tx: mpsc::Sender<Started>,
    started_rx: Option<mpsc::Receiver<Started>>,
}

fn status_word(s: &str) -> &str {
    match s {
        "blocked" => "blocked (waiting at a dialog)",
        other => other,
    }
}

fn numbered<'a>(items: impl Iterator<Item = &'a str>) -> String {
    items
        .enumerate()
        .map(|(i, s)| format!("{}. {s}", i + 1))
        .collect::<Vec<_>>()
        .join("\n")
}

fn parse_status(s: &str) -> AgentStatus {
    serde_json::from_value(Value::String(s.to_owned())).unwrap_or(AgentStatus::Unknown)
}

/// A tool's input as the approval message shows it: a shell command as is, anything else as JSON.
fn describe_input(v: &Value) -> String {
    let shown = match v.get("command").and_then(Value::as_str) {
        Some(c) => c.to_owned(),
        None => serde_json::to_string_pretty(v).unwrap_or_default(),
    };
    let shown = redact_str(&shown);
    match shown.char_indices().nth(MAX_INPUT_CHARS) {
        Some((i, _)) => format!("{}…", &shown[..i]),
        None => shown,
    }
}

/// System reasons such as `expired`; a person's own decision needs no explanation.
fn reason_note(r: &DecideResult) -> String {
    match r.reason.as_deref() {
        Some(why) if !why.starts_with("approved by") && !why.starts_with("denied by") => {
            format!(" ({})", why.replace('_', " "))
        }
        _ => String::new(),
    }
}

fn describe_decision(n: i64, allow: bool, r: &DecideResult) -> String {
    let what = match r.outcome {
        Decision::Allow => "approved",
        Decision::Deny => "denied",
    };
    if r.replayed {
        return format!("Approval {n} was already {what}{}.", reason_note(r));
    }
    match (r.outcome, r.delivery) {
        (Decision::Allow, Some(Delivery::Unknown)) => format!(
            "Approved {n}, but the connection dropped while sending it, so the tool may or may not have run."
        ),
        (Decision::Allow, _) => format!("Approved {n}."),
        (Decision::Deny, _) if r.reason.as_deref() == Some("expired") => {
            format!("Approval {n} had expired, so it was denied.")
        }
        (Decision::Deny, _) if allow => format!(
            "Approval {n} couldn't reach the session in time, so it was denied{}.",
            reason_note(r)
        ),
        (Decision::Deny, _) => format!("Denied {n}."),
    }
}

impl Relay {
    pub fn new(svc: Arc<SessionService>, allowed: Vec<String>) -> Self {
        let (started_tx, started_rx) = mpsc::channel(16);
        Self {
            svc,
            caller: Caller::whatsapp(),
            allowed,
            debouncer: Debouncer::default(),
            self_chat: None,
            dropped: 0,
            current: None,
            pending: None,
            listing: Vec::new(),
            machines: HashMap::new(),
            started_tx,
            started_rx: Some(started_rx),
        }
    }

    /// Linked to your own account: replies and notifications go to your own chat.
    pub fn self_chat(svc: Arc<SessionService>, state: Arc<SelfChat>) -> Self {
        let own = state.own_pn().to_owned();
        Self {
            self_chat: Some(state),
            ..Self::new(svc, vec![own])
        }
    }

    /// Messages ignored in self-chat mode; only this count is kept, never their content.
    pub fn dropped(&self) -> u64 {
        self.dropped
    }

    pub fn current(&self) -> Option<&Target> {
        self.current.as_ref()
    }

    /// The allowed sender's number, or `None` to drop the message unanswered.
    pub fn authorize(&self, m: &Inbound) -> Option<String> {
        if let Some(sc) = &self.self_chat {
            return sc.accepts(m).then(|| sc.own_pn().to_owned());
        }
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
            if self.self_chat.is_some() {
                // Other chats are none of the bridge's business: not even their ids are logged.
                self.dropped += 1;
                tracing::debug!(
                    dropped = self.dropped,
                    "ignored a message outside your own chat"
                );
            } else {
                // Only the sender id is logged, never the content.
                tracing::info!(sender = %m.sender, group = m.is_group, from_me = m.from_me, "dropped message");
            }
            return vec![];
        };
        // WhatsApp's message id makes a start or prompt safe to retry.
        let rid = format!("wa-{}", m.id);
        let texts = match command::parse(&m.text) {
            Command::Help => vec![command::HELP.to_owned()],
            Command::Status => vec![self.status().await],
            Command::Sessions => vec![self.sessions().await],
            Command::Pick(n) if self.pending.is_some() => vec![self.pick(n, &to, &rid).await],
            Command::Pick(_) => vec![self.text(m.text.trim(), &to, &rid).await],
            Command::Text(t) => vec![self.text(&t, &to, &rid).await],
            Command::New { project, prompt } => {
                vec![self.new_session(&project, &prompt, &to, &rid)]
            }
            Command::Use { target, text } => vec![self.reuse(target, text, &rid).await],
            Command::Stop(x) => vec![self.stop(&x).await],
            Command::Read(n) => self.read(n).await,
            Command::Approve(n) => vec![self.decide(n, true).await],
            Command::Deny(n) => vec![self.decide(n, false).await],
            Command::Mute(t) => vec![self.mute(t, true).await],
            Command::Unmute(t) => vec![self.mute(t, false).await],
            Command::Watch(agent) => vec![self.opt_in(&agent).await],
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

    // Routing.

    /// Free text: a named project starts a session there; otherwise it goes to the current one.
    async fn text(&mut self, text: &str, to: &str, rid: &str) -> String {
        self.pending = None;
        let projects = self.svc.projects();
        match route::projects_named(text, &projects).as_slice() {
            [one] => match self.machine_named(text) {
                Ok(machine) => self.start(one, text, machine, to, rid),
                Err(e) => e,
            },
            [] if self.current.is_some() => {
                let target = self.current.clone().expect("checked");
                self.send(&target, text, rid).await
            }
            [] => self.ask_project(
                projects.into_iter().map(|p| p.id).collect(),
                text,
                "Which project should this start in? It names none I know, and no session is current.",
            ),
            several => self.ask_project(
                several.to_vec(),
                text,
                "That names more than one project. Which one should this start in?",
            ),
        }
    }

    fn new_session(&mut self, project: &str, prompt: &str, to: &str, rid: &str) -> String {
        self.pending = None;
        let projects = self.svc.projects();
        match route::project_by_name(project, &projects) {
            Some(p) => match self.machine_named(prompt) {
                Ok(machine) => self.start(&p.id.clone(), prompt, machine, to, rid),
                Err(e) => e,
            },
            None => self.ask_project(
                projects.into_iter().map(|p| p.id).collect(),
                prompt,
                &format!("{project} isn't a project I know. Which one?"),
            ),
        }
    }

    /// A configured machine named in the text, or `None` for the hub's default.
    fn machine_named(&self, text: &str) -> Result<Option<String>, String> {
        let ids: Vec<String> = self
            .svc
            .config()
            .machines
            .iter()
            .map(|m| m.id.clone())
            .collect();
        match route::machines_named(text, &ids).as_slice() {
            [] => Ok(None),
            [one] => Ok(Some(one.clone())),
            several => Err(format!(
                "That names more than one machine ({}), so I didn't start anything. Name one.",
                several.join(", ")
            )),
        }
    }

    fn ask_project(&mut self, options: Vec<String>, prompt: &str, question: &str) -> String {
        if options.is_empty() {
            return "There are no projects in hub.toml yet.".into();
        }
        let machine = self.machine_named(prompt).ok().flatten();
        let list = numbered(options.iter().map(String::as_str));
        self.pending = Some(Choice::Project {
            options,
            prompt: prompt.to_owned(),
            machine,
        });
        format!("{question}\n{list}\nReply with the number.")
    }

    fn ask_target(&mut self, options: Vec<Target>, then: Then) -> String {
        let list = numbered(options.iter().map(Target::label));
        self.pending = Some(Choice::Target { options, then });
        format!("That matches more than one. Which?\n{list}\nReply with the number.")
    }

    async fn pick(&mut self, n: usize, to: &str, rid: &str) -> String {
        let choice = self.pending.take().expect("checked by the caller");
        let len = choice.len();
        if n > len {
            self.pending = Some(choice);
            return format!("Pick a number from 1 to {len}.");
        }
        match choice {
            Choice::Project {
                options,
                prompt,
                machine,
            } => self.start(&options[n - 1], &prompt, machine, to, rid),
            Choice::Target {
                options,
                then: Then::Use(text),
            } => self.adopt(options[n - 1].clone(), text, rid).await,
            Choice::Target {
                options,
                then: Then::Stop,
            } => self.stop_target(&options[n - 1]).await,
        }
    }

    /// Starts a session in the background; its result arrives as a `Started`.
    fn start(
        &mut self,
        project: &str,
        prompt: &str,
        machine: Option<String>,
        to: &str,
        rid: &str,
    ) -> String {
        let req = StartRequest {
            project_id: project.to_owned(),
            prompt: prompt.to_owned(),
            machine_id: machine.clone(),
            agent: None,
            notify: true,
            request_id: Some(rid.to_owned()),
        };
        let (svc, caller, tx, to) = (
            self.svc.clone(),
            self.caller.clone(),
            self.started_tx.clone(),
            to.to_owned(),
        );
        tokio::spawn(async move {
            let result = svc.start(&caller, req).await.map_err(|e| e.to_string());
            let _ = tx.send(Started { to, result }).await;
        });
        let on = machine.map(|m| format!(" on {m}")).unwrap_or_default();
        format!("Starting a session in {project}{on}…")
    }

    pub fn started(&mut self, s: Started) -> Outgoing {
        let text = match s.result {
            Ok(r) => {
                let label = route::label(&r.project_id, &r.session_id);
                self.current = Some(Target::Session {
                    id: r.session_id.clone(),
                    label: label.clone(),
                    machine: r.machine_id.clone(),
                    agent: r.agent_name.clone(),
                });
                format!(
                    "Started {label} in {}, branch {}. Messages that name no project now go to it.",
                    r.project_id,
                    r.branch.unwrap_or_default()
                )
            }
            Err(e) => format!("Couldn't start that session: {e}"),
        };
        Outgoing {
            to: s.to,
            text,
            about: None,
        }
    }

    async fn reuse(&mut self, target: Reuse, text: Option<String>, rid: &str) -> String {
        self.pending = None;
        let target = match target {
            Reuse::This => match &self.current {
                Some(t) => t.clone(),
                None => {
                    return "No session is current yet. Send sessions to see what's running, then use <n>.".into();
                }
            },
            Reuse::Named(x) => match self.resolve(&x).await {
                Resolved::One(t) => t,
                Resolved::Many(options) => return self.ask_target(options, Then::Use(text)),
                Resolved::Nothing(reply) => return reply,
            },
        };
        self.adopt(target, text, rid).await
    }

    async fn adopt(&mut self, target: Target, text: Option<String>, rid: &str) -> String {
        self.current = Some(target.clone());
        match text {
            Some(text) => self.send(&target, &text, rid).await,
            None => format!(
                "Messages that name no project now go to {}.",
                target.label()
            ),
        }
    }

    /// `x` as a number from the last listing, or by name against what is running.
    async fn resolve(&mut self, x: &str) -> Resolved {
        if let Ok(n) = x.trim_start_matches('#').parse::<usize>() {
            return match n.checked_sub(1).and_then(|i| self.listing.get(i)) {
                Some(t) => Resolved::One(t.clone()),
                None => Resolved::Nothing(format!(
                    "There's no {n} in the last sessions list. Send sessions to see them."
                )),
            };
        }
        let live = match self.live().await {
            Ok(l) => l,
            Err(e) => return Resolved::Nothing(e),
        };
        let mut hits = route::match_target(x, &live);
        match hits.len() {
            0 => Resolved::Nothing(format!(
                "Nothing running matches {x}. Send sessions to see what is."
            )),
            1 => Resolved::One(hits.remove(0)),
            _ => Resolved::Many(hits),
        }
    }

    /// Sessions that are starting or running, then named agents Switchboard didn't create.
    async fn live(&self) -> Result<Vec<Live>, String> {
        let sessions = self
            .svc
            .list()
            .await
            .map_err(|e| format!("Couldn't list sessions: {e}"))?;
        let projects = self.svc.projects();
        let mut out = Vec::new();
        let mut session_agents = HashSet::new();
        for s in sessions
            .into_iter()
            .filter(|s| matches!(s.lifecycle, Lifecycle::Starting | Lifecycle::Running))
        {
            let label = route::label(&s.project_id, &s.id);
            let mut names = vec![
                route::norm(&s.id),
                route::norm(&label),
                route::norm(&s.project_id),
            ];
            names.extend(s.branch.iter().chain(&s.agent_name).map(|n| route::norm(n)));
            if let Some(p) = projects.iter().find(|p| p.id == s.project_id) {
                names.extend(p.aliases.iter().map(|a| route::norm(a)));
            }
            session_agents.extend(s.agent_name.clone());
            out.push(Live {
                target: Target::Session {
                    id: s.id.clone(),
                    label,
                    machine: s.machine_id.clone(),
                    agent: s.agent_name.clone(),
                },
                names,
                status: s.agent_status.clone(),
                detail: format!("{} · {}", s.project_id, s.branch.unwrap_or_default()),
                notes: s
                    .pending_approvals
                    .iter()
                    .map(|a| {
                        let n = a.number.unwrap_or_default();
                        format!(
                            "  approval {n} waiting: {} (approve {n} / deny {n})",
                            a.tool_name
                        )
                    })
                    .collect(),
            });
        }
        for m in self.svc.machines().into_iter().filter(|m| m.connected) {
            match self.svc.agents(&m.id).await {
                Ok(agents) => out.extend(
                    agents
                        .into_iter()
                        .filter(|a| a.session_id.is_none() && !session_agents.contains(&a.name))
                        .map(|a| Live {
                            target: Target::Agent {
                                machine: m.id.clone(),
                                name: a.name.clone(),
                            },
                            names: vec![route::norm(&a.name)],
                            status: a.agent_status,
                            detail: "not started by Switchboard".into(),
                            notes: vec![],
                        }),
                ),
                Err(e) => tracing::warn!(machine = %m.id, error = %e, "listing agents"),
            }
        }
        Ok(out)
    }

    // Sending, reading and stopping.

    async fn send(&mut self, target: &Target, text: &str, rid: &str) -> String {
        match target {
            Target::Session { id, label, .. } => {
                match self.svc.prompt(&self.caller, id, text, Some(rid)).await {
                    Ok(_) => format!("Sent to {label}."),
                    Err(HubError::Runner(e)) if e.code == ErrorCode::AgentBlocked => {
                        self.blocked_session(id, label).await
                    }
                    Err(e @ (HubError::NotRunning { .. } | HubError::UnknownSession(_))) => {
                        self.current = None;
                        format!(
                            "{label} isn't running any more ({e}). Name a project to start a new session."
                        )
                    }
                    Err(e) => format!("Couldn't send to {label}: {e}"),
                }
            }
            Target::Agent { machine, name } => {
                match self
                    .svc
                    .prompt_agent(&self.caller, machine, name, text)
                    .await
                {
                    Ok(AgentPromptOutcome::Sent) => format!("Sent to {name}."),
                    Ok(AgentPromptOutcome::Blocked { screen }) => fit_one(&format!(
                        "{name} is waiting at a dialog, so I didn't send that. {FOREIGN_DIALOG}\n\n{}",
                        tail(&redact_str(&screen), EXCERPT_LINES)
                    )),
                    Ok(AgentPromptOutcome::NotFound) => {
                        self.current = None;
                        format!("{name} isn't running any more. Send sessions to pick another.")
                    }
                    Err(e) => format!("Couldn't send to {name}: {e}"),
                }
            }
        }
    }

    /// Why a session refused a prompt: its pending approval, or a dialog only the tab can answer.
    async fn blocked_session(&self, id: &str, label: &str) -> String {
        let Ok(view) = self.svc.get(id, None, Duration::ZERO).await else {
            return format!("{label} is waiting at a dialog, so I didn't send that.");
        };
        if let Some(a) = view.pending_approvals.first() {
            let n = a.number.unwrap_or_default();
            return format!(
                "{label} is waiting for approval {n} ({}), so I didn't send that. Reply approve {n} or deny {n} first.",
                a.tool_name
            );
        }
        let screen = match &view.agent_name {
            Some(agent) => self
                .svc
                .read_agent(&view.machine_id, agent, 0, true)
                .await
                .map(|r| r.text)
                .unwrap_or_default(),
            None => String::new(),
        };
        fit_one(&format!(
            "{label} is waiting at a dialog, so I didn't send that. {SESSION_DIALOG}\n\n{}",
            tail(&redact_str(&screen), EXCERPT_LINES)
        ))
    }

    async fn read(&self, n: u32) -> Vec<String> {
        let Some(target) = &self.current else {
            return vec!["No session is current. Send sessions, then use <n>.".into()];
        };
        let read = match target {
            Target::Session { id, .. } => self.svc.read(id, overfetch(n)).await,
            Target::Agent { machine, name } => {
                self.svc
                    .read_agent(machine, name, overfetch(n), false)
                    .await
            }
        };
        match read {
            Ok(r) => {
                // Masked before splitting, so no secret is cut in two and missed.
                let text = tail(&redact_str(&r.text), n as usize);
                if text.is_empty() {
                    vec![format!("{} has no output yet.", target.label())]
                } else {
                    split(&text)
                }
            }
            Err(e) => vec![format!("Couldn't read {}: {e}", target.label())],
        }
    }

    async fn stop(&mut self, x: &str) -> String {
        self.pending = None;
        match self.resolve(x).await {
            Resolved::One(t) => self.stop_target(&t).await,
            Resolved::Many(options) => self.ask_target(options, Then::Stop),
            Resolved::Nothing(reply) => reply,
        }
    }

    async fn stop_target(&mut self, target: &Target) -> String {
        match target {
            Target::Agent { name, .. } => format!(
                "Switchboard didn't start {name}, so it won't stop it. Close it in its Herdr tab."
            ),
            Target::Session { id, label, .. } => match self.svc.stop(&self.caller, id, false).await
            {
                Ok(r) => {
                    if self.current.as_ref() == Some(target) {
                        self.current = None;
                    }
                    format!(
                        "Stopped {label}. Its worktree and branch {} are kept.",
                        r.branch.unwrap_or_default()
                    )
                }
                Err(e) => format!("Couldn't stop {label}: {e}"),
            },
        }
    }

    // Approvals.

    async fn decide(&self, n: i64, allow: bool) -> String {
        match self.svc.decide_number(&self.caller, n, allow).await {
            Ok(r) => describe_decision(n, allow, &r),
            Err(HubError::UnknownApproval(_)) => format!("There's no approval {n}."),
            Err(e) => format!("Couldn't decide approval {n}: {e}"),
        }
    }

    /// A new permission request, unless its session is muted.
    pub async fn approval(&mut self, session_id: &str, a: &ApprovalView) -> Vec<Outgoing> {
        let (Ok(s), Ok(state)) = (
            self.svc.get(session_id, None, Duration::ZERO).await,
            self.svc.notify_state().await,
        ) else {
            return vec![];
        };
        if s.agent_name.as_deref().is_none_or(|n| !state.notifies(n)) {
            return vec![];
        }
        let label = route::label(&s.project_id, &s.id);
        let n = a.number.unwrap_or_default();
        let minutes = ((a.deadline - self.svc.now()).max(0) + 59_999) / 60_000;
        let text = fit_one(&format!(
            "{label} asks to use {}, approval {n}:\n\n{}\n\nReply approve {n} or deny {n}. Unanswered, it's denied in about {minutes} min.",
            redact_str(&a.tool_name),
            describe_input(&a.tool_input)
        ));
        self.to_everyone(text, None)
    }

    // Notifications and mutes, kept in the hub's shared store.

    fn to_everyone(&self, text: String, about: Option<String>) -> Vec<Outgoing> {
        self.allowed
            .iter()
            .map(|to| Outgoing {
                to: to.clone(),
                text: text.clone(),
                about: about.clone(),
            })
            .collect()
    }

    /// Every named agent is tracked, so watching or unmuting takes effect at once.
    pub fn observe(&mut self, machine: &str, agent: &str, status: AgentStatus, now: Instant) {
        self.machines.insert(agent.to_owned(), machine.to_owned());
        self.debouncer.observe(agent, status, now);
    }

    /// Notifications whose status has held long enough, for agents that notify right now.
    pub async fn due(&mut self, now: Instant) -> Vec<Outgoing> {
        let due = self.debouncer.due(now);
        if due.is_empty() {
            return vec![];
        }
        let state = match self.svc.notify_state().await {
            Ok(s) => s,
            Err(e) => {
                tracing::warn!(error = %e, "reading mute state");
                return vec![];
            }
        };
        let mut out = Vec::new();
        for (agent, notify) in due.into_iter().filter(|(a, _)| state.notifies(a)) {
            let session = self.svc.session_by_agent(&agent).await.ok().flatten();
            let Some(machine) = session
                .as_ref()
                .map(|s| s.machine_id.clone())
                .or_else(|| self.machines.get(&agent).cloned())
            else {
                continue;
            };
            let label = session
                .as_ref()
                .map_or(agent.clone(), |s| route::label(&s.project_id, &s.id));
            let text = match notify {
                Notify::Blocked => {
                    // Its approval message already says what it's waiting for.
                    if session
                        .as_ref()
                        .is_some_and(|s| !s.pending_approvals.is_empty())
                    {
                        continue;
                    }
                    let screen = self
                        .svc
                        .read_agent(&machine, &agent, 0, true)
                        .await
                        .map(|r| r.text)
                        .unwrap_or_default();
                    let hint = if session.is_some() {
                        SESSION_DIALOG
                    } else {
                        FOREIGN_DIALOG
                    };
                    format!(
                        "{label} is waiting at a dialog:\n\n{}\n\n{hint}",
                        tail(&redact_str(&screen), EXCERPT_LINES)
                    )
                }
                Notify::Finished => {
                    let output = self
                        .svc
                        .read_agent(&machine, &agent, overfetch(FINISHED_LINES as u32), false)
                        .await
                        .map(|r| r.text)
                        .unwrap_or_default();
                    format!(
                        "{label} finished:\n\n{}",
                        tail(&redact_str(&output), FINISHED_LINES)
                    )
                }
            };
            out.extend(self.to_everyone(fit_one(&text), Some(agent)));
        }
        out
    }

    fn mute_all_note(state: &WatchState) -> Option<String> {
        state
            .mute_all
            .then(|| "All notifications are muted. Send unmute all to resume.".to_owned())
    }

    fn current_mark(&self, t: &Target) -> &'static str {
        if self.current.as_ref() == Some(t) {
            " ← current"
        } else {
            ""
        }
    }

    /// Watched sessions and agents with their pending approvals, plus opted-in ones not running.
    async fn status(&self) -> String {
        let (state, live) = match (self.svc.notify_state().await, self.live().await) {
            (Ok(s), Ok(l)) => (s, l),
            (Err(e), _) => return format!("Couldn't read the mute state: {e}"),
            (_, Err(e)) => return e,
        };
        let mut lines: Vec<String> = Self::mute_all_note(&state).into_iter().collect();
        let mut seen = Vec::new();
        for l in &live {
            let Some(name) = l.target.agent_name() else {
                continue;
            };
            if state.is_watched(name) {
                seen.push(name.to_owned());
                lines.push(format!(
                    "{}: {} · {}{}",
                    l.target.label(),
                    status_word(&l.status),
                    state.describe(name),
                    self.current_mark(&l.target)
                ));
                lines.extend(l.notes.iter().cloned());
            }
        }
        for name in state.watched.iter().filter(|n| !seen.contains(n)) {
            lines.push(format!("{name}: not running · {}", state.describe(name)));
        }
        if lines.len() == usize::from(state.mute_all) {
            lines.push(format!(
                "No watched sessions or agents are running. Switchboard's sessions ({SWITCHBOARD_PREFIX}...) \
                 are watched automatically; send watch <agent> to add another."
            ));
        }
        lines.join("\n")
    }

    async fn sessions(&mut self) -> String {
        let live = match self.live().await {
            Ok(l) => l,
            Err(e) => return e,
        };
        self.listing = live.iter().map(|l| l.target.clone()).collect();
        if live.is_empty() {
            return "Nothing is running. Name a project to start a session.".into();
        }
        let lines: Vec<String> = live
            .iter()
            .enumerate()
            .map(|(i, l)| {
                format!(
                    "{}. {} · {} · {}{}",
                    i + 1,
                    l.target.label(),
                    l.detail,
                    status_word(&l.status),
                    self.current_mark(&l.target)
                )
            })
            .collect();
        format!("{}\nSend use <n> to switch to one.", lines.join("\n"))
    }

    fn save_failed(e: HubError) -> String {
        format!("Couldn't save that, so nothing changed: {e}")
    }

    /// The agent name behind `x`: a running session or agent, or `x` itself.
    async fn agent_for(&mut self, x: &str) -> Result<String, String> {
        if let Resolved::One(t) = self.resolve(x).await
            && let Some(name) = t.agent_name()
        {
            return Ok(name.to_owned());
        }
        check_agent_name(x)
            .map(|()| x.to_owned())
            .map_err(|_| format!("{x} isn't a session or agent name. Send sessions to see them."))
    }

    async fn mute(&mut self, target: Mute, muted: bool) -> String {
        let state = match self.svc.notify_state().await {
            Ok(s) => s,
            Err(e) => return Self::save_failed(e),
        };
        match target {
            Mute::All if muted => match self.svc.set_mute_all(&self.caller, true).await {
                Ok(_) => "Muted all notifications, including sessions started later. Send unmute all to resume.".into(),
                Err(e) => Self::save_failed(e),
            },
            Mute::All => {
                let mut result = self.svc.set_mute_all(&self.caller, false).await;
                for name in &state.muted {
                    if result.is_ok() {
                        result = self.svc.set_agent_muted(&self.caller, name, false).await;
                    }
                }
                match result {
                    Ok(_) => "Notifications are back on for every watched session and agent.".into(),
                    Err(e) => Self::save_failed(e),
                }
            }
            Mute::Agent(_) if !muted && state.mute_all => {
                "All notifications are muted. Send unmute all first.".into()
            }
            Mute::Agent(x) => {
                let name = match self.agent_for(&x).await {
                    Ok(n) => n,
                    Err(e) => return e,
                };
                if !muted && !state.muted.contains(&name) {
                    return format!("{name} wasn't muted.");
                }
                let watched = state.is_watched(&name);
                match self.svc.set_agent_muted(&self.caller, &name, muted).await {
                    Ok(_) if muted && watched => format!("Muted {name}."),
                    Ok(_) if muted => {
                        format!("Muted {name}. It isn't watched, so it wasn't notifying anyway.")
                    }
                    Ok(_) if watched => format!("Unmuted {name}."),
                    Ok(_) => format!(
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
        let running = self
            .live()
            .await
            .map(|l| l.iter().any(|x| x.target.agent_name() == Some(name)))
            .unwrap_or(false);
        let mut result = self.svc.set_agent_watched(&self.caller, name, true).await;
        if let Ok(s) = &result
            && s.muted.contains(name)
        {
            result = self.svc.set_agent_muted(&self.caller, name, false).await;
        }
        match result {
            Ok(state) => {
                let mut reply = format!("Watching {name}.");
                if !running {
                    reply.push_str(" It isn't running now; you'll hear from it once it is.");
                }
                if state.mute_all {
                    reply.push_str(
                        " All notifications are muted, though; send unmute all to resume.",
                    );
                }
                reply
            }
            Err(e) => Self::save_failed(e),
        }
    }

    // Sending.

    /// The text as sent: masked, and marked in self-chat mode so its echo is recognised.
    fn outgoing(&self, text: &str) -> String {
        let text = redact_str(text);
        match self.self_chat {
            Some(_) => SelfChat::mark(&text),
            None => text,
        }
    }

    fn sent(&self, id: Option<String>) {
        if let (Some(sc), Some(id)) = (&self.self_chat, id) {
            sc.record_sent(id);
        }
    }
}

/// Runs the relay until the inbound channel closes.
pub async fn run<T: Transport>(
    relay: &mut Relay,
    transport: &T,
    mut inbound: mpsc::Receiver<Inbound>,
    mut events: broadcast::Receiver<HubEvent>,
) {
    let mut started = relay.started_rx.take().expect("a relay runs once");
    let mut outbox = Outbox::new(Instant::now());
    let mut tick = tokio::time::interval(TICK);
    let mut events_open = true;
    loop {
        tokio::select! {
            m = inbound.recv() => match m {
                Some(m) => for o in relay.handle(&m).await { outbox.push(o) },
                None => break,
            },
            ev = events.recv(), if events_open => match ev {
                Ok(HubEvent::AgentStatus { machine_id, agent_name, agent_status }) => {
                    relay.observe(&machine_id, &agent_name, parse_status(&agent_status), Instant::now());
                }
                Ok(HubEvent::ApprovalRequested { session_id, approval }) => {
                    for o in relay.approval(&session_id, &approval).await { outbox.push(o) }
                }
                Err(broadcast::error::RecvError::Lagged(missed)) => {
                    tracing::warn!(missed, "the bridge fell behind the hub's events");
                }
                Err(broadcast::error::RecvError::Closed) => events_open = false,
            },
            Some(s) = started.recv() => outbox.push(relay.started(s)),
            _ = tick.tick() => for o in relay.due(Instant::now()).await { outbox.push(o) },
        }
        while let Some(o) = outbox.pop_ready(Instant::now()) {
            // Every message leaves the machine; mask once more in case a reply quoted a secret.
            match transport.send(&o.to, &relay.outgoing(&o.text)).await {
                Ok(id) => relay.sent(id),
                Err(e) => {
                    tracing::warn!(error = %e, "send failed; will retry");
                    outbox.requeue(o);
                    break;
                }
            }
        }
    }
}

#[cfg(test)]
#[path = "relay_tests.rs"]
mod tests;
