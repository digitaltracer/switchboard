//! "While you were away": what each agent did since a time, from facts the hub already records
//! (turns started and finished, dialogs, errors, approvals, sessions started) plus each agent's
//! own last line, all masked. An opt-in summary hands those facts to `claude -p` on a runner.

use std::collections::{BTreeMap, HashMap};
use std::time::Duration;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::*;
use crate::proto::CAP_SUMMARY;
use crate::store::AgentEvent;

const SETTINGS_KEY: &str = "away";
const SUMMARY_TIMEOUT: Duration = Duration::from_secs(75);

/// The owner's choices for "while you were away", kept on the hub.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AwaySettings {
    /// Summarise with `claude -p` on the owner's Claude account. Off unless the owner turns it on.
    pub summary: bool,
    pub model: String,
}

impl Default for AwaySettings {
    fn default() -> Self {
        Self {
            summary: false,
            model: "haiku".into(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct AwayFact {
    /// Unix ms.
    pub at: i64,
    /// `started`, `working`, `finished`, `waiting`, `error`, `asked`, `allowed` or `denied`.
    pub kind: String,
    /// Masked.
    pub text: String,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct AwayAgent {
    pub machine_id: String,
    pub agent: String,
    pub session_id: Option<String>,
    /// What it is now: `waiting`, `error`, `working` or `idle`, when the hub has seen it.
    pub state: Option<String>,
    pub facts: Vec<AwayFact>,
    /// The first line of the last thing it said, masked.
    pub last_reply: Option<String>,
}

/// What one status means for the board of facts.
pub fn state_of(agent_status: &str, error: bool) -> &'static str {
    match (error, agent_status) {
        (true, _) => "error",
        (_, "blocked") => "waiting",
        (_, "working") => "working",
        _ => "idle",
    }
}

/// The event a change of state records, if any. A first sighting records nothing, since what
/// came before it is unknown.
pub fn transition(prev: Option<&str>, now: &str) -> Option<&'static str> {
    let prev = prev?;
    if prev == now {
        return None;
    }
    match now {
        "working" => Some("working"),
        "waiting" => Some("waiting"),
        "error" => Some("error"),
        "idle" if prev == "working" => Some("finished"),
        _ => None,
    }
}

/// One agent's entry, made on first use.
fn slot<'a>(
    by: &'a mut BTreeMap<(String, String), AwayAgent>,
    machine: &str,
    agent: &str,
    session: Option<&String>,
) -> &'a mut AwayAgent {
    let e = by
        .entry((machine.to_owned(), agent.to_owned()))
        .or_insert_with(|| AwayAgent {
            machine_id: machine.to_owned(),
            agent: agent.to_owned(),
            session_id: None,
            state: None,
            facts: vec![],
            last_reply: None,
        });
    if e.session_id.is_none() {
        e.session_id = session.cloned();
    }
    e
}

/// A short, masked line for what a tool call wanted: the command for a shell call.
fn asked(tool: &str, input: &str) -> String {
    let v: Value = serde_json::from_str(input).unwrap_or(Value::Null);
    let what = v["command"]
        .as_str()
        .map(str::to_owned)
        .unwrap_or_else(|| tool.to_owned());
    let what: String = redact_str(&what).chars().take(120).collect();
    format!("Asked to run {what}")
}

impl SessionService {
    /// Notes each listed agent's state, recording the changes as events. A session's agent is
    /// left to its own status events, which a listing taken a moment earlier could contradict.
    pub(super) async fn note_states(&self, machine: &str, agents: &[AgentSummary]) {
        for a in agents.iter().filter(|a| a.session_id.is_none()) {
            self.note_state(
                machine,
                &a.name,
                a.session_id.as_deref(),
                &a.agent_status,
                a.error,
            )
            .await;
        }
    }

    /// Notes one agent's state, from a listing or a runner's status event; a change is recorded.
    pub(super) async fn note_state(
        &self,
        machine: &str,
        agent: &str,
        session_id: Option<&str>,
        agent_status: &str,
        error: bool,
    ) {
        let now = state_of(agent_status, error);
        let prev = self
            .agent_states
            .lock()
            .unwrap()
            .insert((machine.to_owned(), agent.to_owned()), now.to_owned());
        let Some(kind) = transition(prev.as_deref(), now) else {
            return;
        };
        let e = AgentEvent {
            machine_id: machine.to_owned(),
            agent: agent.to_owned(),
            session_id: session_id.map(str::to_owned),
            kind: kind.into(),
            at: self.now(),
        };
        if let Err(err) = self.store.record_agent_event(e).await {
            tracing::warn!(error = %err, "recording an agent event");
        }
    }

    pub async fn away_settings(&self) -> Result<AwaySettings, HubError> {
        Ok(self
            .store
            .setting(SETTINGS_KEY)
            .await?
            .and_then(|v| serde_json::from_str(&v).ok())
            .unwrap_or_default())
    }

    pub async fn set_away_settings(
        &self,
        caller: &Caller,
        s: AwaySettings,
    ) -> Result<(), HubError> {
        if !crate::runner::summary::valid_model(&s.model) {
            return Err(HubError::BadRequest("not a model name".into()));
        }
        self.store
            .set_setting(
                SETTINGS_KEY,
                &serde_json::to_string(&s).expect("serializes"),
            )
            .await?;
        self.audit(
            &caller.actor,
            None,
            "away_settings_set",
            json!({ "summary": s.summary, "model": s.model }),
        )
        .await
    }

    /// Each agent that did something since `since` (Unix ms), with what it did, oldest first.
    pub async fn away(&self, since: i64) -> Result<Vec<AwayAgent>, HubError> {
        let mut by: BTreeMap<(String, String), AwayAgent> = BTreeMap::new();
        let fact = |at: i64, kind: &str, text: String| AwayFact {
            at,
            kind: kind.into(),
            text,
        };
        let sessions = self.list().await?;
        // A session's own status is fresher than the last listing.
        let session_state: HashMap<String, &'static str> = sessions
            .iter()
            .map(|s| (s.id.clone(), state_of(&s.agent_status, false)))
            .collect();
        for s in sessions {
            let Some(agent) = s.agent_name.clone().filter(|_| s.created_at >= since) else {
                continue;
            };
            let label = route::label(&s.project_id, &s.id, s.name.as_deref());
            slot(&mut by, &s.machine_id, &agent, Some(&s.id))
                .facts
                .push(fact(
                    s.created_at,
                    "started",
                    redact_str(&format!("Started as {label}")),
                ));
        }
        for ev in self.store.agent_events_since(since).await? {
            let text = match ev.kind.as_str() {
                "working" => "Started working",
                "finished" => "Finished its turn",
                "waiting" => "Waiting at a dialog",
                _ => "Stopped with an error",
            };
            slot(&mut by, &ev.machine_id, &ev.agent, ev.session_id.as_ref())
                .facts
                .push(fact(ev.at, &ev.kind, text.into()));
        }
        // A dialog the hub recorded, unless a status change already said it waited then.
        for (machine, agent, session, at) in self.store.attention_since(since).await? {
            let e = slot(&mut by, &machine, &agent, session.as_ref());
            if !e
                .facts
                .iter()
                .any(|f| f.kind == "waiting" && (f.at - at).abs() < 60_000)
            {
                e.facts
                    .push(fact(at, "waiting", "Waiting at a dialog".into()));
            }
        }
        for a in self.store.approval_facts_since(since).await? {
            let Some(agent) = a.agent.clone() else {
                continue;
            };
            let e = slot(&mut by, &a.machine_id, &agent, Some(&a.session_id));
            if a.created_at >= since {
                e.facts.push(fact(
                    a.created_at,
                    "asked",
                    asked(&a.tool_name, &a.tool_input),
                ));
            }
            if let (Some(outcome), Some(at)) = (&a.outcome, a.decided_at) {
                let (kind, text) = match (outcome.as_str(), a.reason.as_deref()) {
                    ("allow", _) => ("allowed", "You allowed it"),
                    (_, Some("expired")) => ("denied", "Denied: no answer in time"),
                    _ => ("denied", "Denied"),
                };
                e.facts.push(fact(at, kind, text.into()));
            }
        }
        let states = self.agent_states.lock().unwrap().clone();
        let mut out: Vec<AwayAgent> = by.into_values().collect();
        if out.is_empty() {
            return Ok(out);
        }
        let (lines, _) = self.activity().await;
        let lines: HashMap<(String, String), Option<String>> = lines
            .into_iter()
            .map(|(m, a)| ((m, a.name), a.line))
            .collect();
        for a in &mut out {
            a.facts.sort_by_key(|f| f.at);
            a.state = a
                .session_id
                .as_ref()
                .and_then(|id| session_state.get(id))
                .map(|s| (*s).to_owned())
                .or_else(|| {
                    states
                        .get(&(a.machine_id.clone(), a.agent.clone()))
                        .cloned()
                });
            a.last_reply = lines
                .get(&(a.machine_id.clone(), a.agent.clone()))
                .cloned()
                .flatten()
                .map(|l| redact_str(&l).chars().take(400).collect());
        }
        // Who needs the owner first, then the most recent.
        out.sort_by_key(|a| {
            let rank = match a.state.as_deref() {
                Some("waiting") => 0,
                Some("error") => 1,
                _ => 2,
            };
            (rank, -a.facts.last().map_or(0, |f| f.at))
        });
        Ok(out)
    }

    /// The facts since `since`, summarised by `claude -p` on the default machine's runner. Only
    /// when the owner turned it on; the facts are masked before they go.
    pub async fn away_summary(&self, since: i64) -> Result<String, HubError> {
        let settings = self.away_settings().await?;
        if !settings.summary {
            return Err(HubError::BadRequest(
                "the summary is off; turn it on in Settings".into(),
            ));
        }
        let machine = self.default_machine().to_owned();
        if !self.runners.can(&machine, CAP_SUMMARY) {
            return Err(HubError::BadRequest(
                "this machine's runner is too old to summarise; update Switchboard there".into(),
            ));
        }
        let agents = self.away(since).await?;
        if agents.is_empty() {
            return Ok("Nothing happened while you were away.".into());
        }
        let mut prompt = String::from(
            "Summarise what these coding agents did while their owner was away, in at most five \
             short sentences of plain text. Lead with anything that needs the owner. Use the \
             agents' names. Don't invent anything that isn't below.\n\n",
        );
        for a in &agents {
            prompt += &format!(
                "Agent {} (now {}):\n",
                a.agent,
                a.state.as_deref().unwrap_or("unknown")
            );
            for f in &a.facts {
                prompt += &format!("- {}\n", f.text);
            }
            if let Some(r) = &a.last_reply {
                prompt += &format!("- Last said: {r}\n");
            }
        }
        let op = Op::Summarize {
            prompt: redact_str(&prompt),
            model: settings.model,
        };
        let v = self
            .runners
            .call(&machine, op, SUMMARY_TIMEOUT)
            .await
            .map_err(|e| runner_error(&machine, e))?;
        let s = v["summary"].as_str().unwrap_or_default();
        Ok(redact_str(s))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_change_of_state_records_what_happened() {
        assert_eq!(
            transition(None, "working"),
            None,
            "a first sighting says nothing"
        );
        assert_eq!(transition(Some("idle"), "working"), Some("working"));
        assert_eq!(transition(Some("working"), "idle"), Some("finished"));
        assert_eq!(transition(Some("waiting"), "idle"), None);
        assert_eq!(transition(Some("working"), "waiting"), Some("waiting"));
        assert_eq!(transition(Some("working"), "error"), Some("error"));
        assert_eq!(transition(Some("idle"), "idle"), None);
        assert_eq!(state_of("done", true), "error");
        assert_eq!(state_of("blocked", false), "waiting");
        assert_eq!(state_of("done", false), "idle");
        assert_eq!(
            asked(
                "Bash",
                r#"{"command":"curl -H 'Authorization: Bearer abcdefghijkl' x"}"#
            ),
            "Asked to run curl -H 'Authorization: Bearer [redacted]' x"
        );
        assert_eq!(asked("Edit", r#"{"file_path":"a"}"#), "Asked to run Edit");
    }
}
