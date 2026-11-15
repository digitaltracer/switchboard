//! The owner's signed choice of one listed option on a dialog drawn on an agent's screen, sent
//! as keys. With `codex.rs` this is the only key path, and it only acts on a dialog a
//! recogniser in `src/agents/` recognised by its layout. Arrows move the selection; a fresh read
//! must then show the same occurrence with that option selected, and Herdr the same process,
//! before the single Enter. Esc is never sent, and a send that may have happened is never retried.
//! A group of questions is answered one question at a time, and one left arrow shows its
//! previous question again; neither submits anything.
//!
//! Residual risk, accepted by the owner: input at the agent's own terminal within milliseconds
//! of the Enter can make it answer the next dialog.

use std::collections::HashMap;
use std::future::Future;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use super::backend::{Read, Server};
use super::logcache::{self, LogCache};
use crate::agents::dialog::{Dialog, DialogKind, DialogStep, Recognised};
use crate::agents::{claude, codex};
use crate::auth::device::label_sha256;
use crate::proto::{DialogOptionView, DialogView, ErrorCode, ProtoError};
use crate::redact::redact_str;

/// How long the screen gets to redraw after the arrows, per read.
pub const SETTLE: Duration = Duration::from_millis(200);
pub const SELECT_READS: u32 = 5;
pub const CONFIRM_ATTEMPTS: u32 = 6;
pub const CONFIRM_INTERVAL: Duration = Duration::from_millis(500);
/// Answered occurrences kept on disk.
const KEEP_ATTEMPTS: usize = 1000;

/// What the server says about the process in the pane, as Herdr's `agent.get` gives it.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Process {
    pub kind: Option<String>,
    pub terminal_id: Option<String>,
    pub agent_session: Option<String>,
    pub state_change_seq: u64,
    pub blocked: bool,
}

impl Process {
    fn same(&self, other: &Process) -> bool {
        (
            &self.terminal_id,
            &self.agent_session,
            self.state_change_seq,
        ) == (
            &other.terminal_id,
            &other.agent_session,
            other.state_change_seq,
        )
    }
}

pub trait DialogIo {
    fn process(&self) -> impl Future<Output = Result<Process, String>> + Send;
    fn screen(&self) -> impl Future<Output = Result<String, String>> + Send;
    /// The pending tool call's id from the agent's log, when it shows one.
    fn pending_call(&self) -> impl Future<Output = Option<String>> + Send;
    fn send(&self, keys: &[&'static str]) -> impl Future<Output = Result<(), String>> + Send;
    /// What the server said a blocked agent waits for at the last `process`; Herdr never says.
    fn blocked_kind(&self) -> Option<String> {
        None
    }
}

/// Whether the server's own word for a block fits the dialog recognised. A server that didn't
/// say agrees; Claude calls its plan approval a permission.
fn kinds_agree(said: Option<&str>, kind: DialogKind) -> bool {
    match said {
        None => true,
        Some("permission") => matches!(kind, DialogKind::Permission | DialogKind::Plan),
        Some("question") => kind == DialogKind::Question,
        Some(_) => false,
    }
}

/// Why keys may not go to a pane, asked at the first read and again just before Enter.
pub type Refusal<'a> = &'a (dyn Fn() -> Option<&'static str> + Send + Sync);

pub fn recognise(kind: Option<&str>, text: &str) -> Recognised {
    match kind {
        Some("claude") => claude::screen::recognise(text),
        Some("codex") => codex::screen::recognise(text),
        _ => Recognised::Unknown,
    }
}

fn sha256_hex(s: &str) -> String {
    hex::encode(Sha256::digest(s.as_bytes()))
}

/// The dialog's raw text without its selection marker and footer, both of which follow the
/// selection. Runner-private: only its hash leaves this module.
fn fingerprint(d: &Dialog) -> String {
    let lines: Vec<&str> = d.region.lines().collect();
    let body = lines[..lines.len().saturating_sub(1)]
        .iter()
        .map(|l| l.trim_start_matches(['❯', '›']).trim())
        .collect::<Vec<_>>()
        .join("\n");
    sha256_hex(&body)
}

/// One read of the pane: the process, the dialog if one is recognised, and its occurrence.
#[derive(Debug, Clone)]
pub struct Seen {
    pub process: Process,
    pub dialog: Option<Dialog>,
    /// The pending call's id when the log shows one, the pane incarnation and Herdr's state
    /// sequence, and the dialog's fingerprint, hashed. Two identical dialogs in a row differ by
    /// their call ids.
    pub occurrence: String,
}

impl Seen {
    pub fn option_id(&self, number: u32) -> String {
        sha256_hex(&format!("{}\n{number}", self.occurrence))[..16].to_owned()
    }
}

pub async fn look<I: DialogIo>(io: &I) -> Result<Seen, ProtoError> {
    let herdr = |e: String| ProtoError::new(ErrorCode::Herdr, e);
    let process = io.process().await.map_err(herdr)?;
    let said = io.blocked_kind();
    let screen = io.screen().await.map_err(herdr)?;
    let call = io.pending_call().await;
    let dialog = match recognise(process.kind.as_deref(), &screen) {
        // A server that says the agent waits for something else: shown, never answered.
        Recognised::Dialog(mut d) => {
            if !kinds_agree(said.as_deref(), d.kind) {
                d.actionable = false;
            }
            Some(d)
        }
        Recognised::Unknown => None,
    };
    let material = format!(
        "{}\n{}\n{}\n{}",
        process.terminal_id.as_deref().unwrap_or_default(),
        process.state_change_seq,
        dialog.as_ref().map(fingerprint).unwrap_or_default(),
        call.as_deref().unwrap_or_default(),
    );
    Ok(Seen {
        process,
        dialog,
        occurrence: sha256_hex(&material)[..32].to_owned(),
    })
}

/// Which of a group's steps is open: where the runner's own Enter or left arrow last left it,
/// which a Claude screen doesn't say once several questions are answered; else what it says.
fn open_step(seen: &Seen, answered: &Answered) -> Option<usize> {
    let d = seen.dialog.as_ref()?;
    answered
        .open(&seen.occurrence)
        .filter(|&i| i < d.steps.len())
        .or_else(|| d.steps.iter().position(|s| s.current))
}

/// Whether a left arrow may show the group's previous question: the open one is known and not
/// the first, and a listed option is selected, since on a free-text row the key edits text.
fn may_go_back(seen: &Seen, answered: &Answered) -> bool {
    let Some(d) = &seen.dialog else { return false };
    d.actionable
        && open_step(seen, answered).is_some_and(|i| i > 0)
        && d.options.get(d.selected).is_some_and(|o| !o.excluded)
}

/// Whether keys may answer what `seen` shows, and if not, why.
fn refusal(seen: &Seen, refuse: Refusal<'_>, answered: &Answered) -> Option<&'static str> {
    let d = seen.dialog.as_ref()?;
    if !d.actionable {
        return Some("read_only");
    }
    if let Some(why) = refuse() {
        return Some(why);
    }
    if !seen.process.blocked {
        return Some("not_blocked");
    }
    if answered.blocks(&seen.occurrence) {
        return Some("answered");
    }
    None
}

/// The masked dialog for the hub, or `None` when no dialog is recognised.
pub fn view(seen: &Seen, refuse: Refusal<'_>, answered: &Answered) -> Option<DialogView> {
    let d = seen.dialog.as_ref()?;
    let why = refusal(seen, refuse, answered);
    let open = open_step(seen, answered);
    let options = d
        .options
        .iter()
        .map(|o| {
            let label = redact_str(&o.label);
            DialogOptionView {
                id: seen.option_id(o.number),
                label_sha256: label_sha256(std::slice::from_ref(&label)),
                label,
                is_deny: o.is_deny,
                offered: !o.excluded && d.actionable,
                provisional: o.provisional,
            }
        })
        .collect();
    Some(DialogView {
        kind: d.kind,
        prompt: redact_str(&d.prompt),
        options,
        selected: d.options.get(d.selected).map(|o| seen.option_id(o.number)),
        occurrence: seen.occurrence.clone(),
        answerable: why.is_none(),
        reason: why.map(str::to_owned),
        steps: d
            .steps
            .iter()
            .enumerate()
            .map(|(i, s)| DialogStep {
                label: redact_str(&s.label),
                answered: s.answered,
                current: open == Some(i),
            })
            .collect(),
        back: why.is_none() && may_go_back(seen, answered),
    })
}

#[derive(Debug, Clone)]
pub struct AnswerRequest {
    pub occurrence: String,
    pub option: String,
    pub label_sha256: String,
    pub deny_only: bool,
}

fn changed(why: impl Into<String>) -> ProtoError {
    ProtoError::new(ErrorCode::DialogChanged, why)
}

/// Picks one option by keys, and returns what the screen shows afterwards. The caller holds the
/// pane's action lock throughout.
pub async fn answer<I: DialogIo>(
    io: &I,
    refuse: Refusal<'_>,
    answered: &Answered,
    req: &AnswerRequest,
) -> Result<Seen, ProtoError> {
    let first = look(io).await?;
    let Some(dialog) = &first.dialog else {
        return Err(changed("no dialog is showing"));
    };
    if first.occurrence != req.occurrence {
        return Err(changed("the dialog changed"));
    }
    if let Some(why) = refusal(&first, refuse, answered) {
        return Err(changed(format!("keys may not answer this dialog: {why}")));
    }
    let (target, option) = dialog
        .options
        .iter()
        .enumerate()
        .find(|(_, o)| first.option_id(o.number) == req.option)
        .ok_or_else(|| changed("no such option on this dialog"))?;
    if option.excluded {
        return Err(ProtoError::new(
            ErrorCode::BadRequest,
            "that option can't be chosen from the phone",
        ));
    }
    if label_sha256(&[redact_str(&option.label)]) != req.label_sha256 {
        return Err(changed("the option's label changed"));
    }
    // A group's own question submits nothing; the choice that submits is never provisional.
    if req.deny_only && !option.is_deny && !option.provisional {
        return Err(ProtoError::new(
            ErrorCode::ApproveNotAllowed,
            "only the deny option, or one question of a group, may be chosen without the approve right",
        ));
    }
    answered.begin(&first.occurrence).map_err(changed)?;
    let abort = |why: &str| {
        answered.settle(&first.occurrence, Attempt::Aborted);
        changed(why)
    };

    let steps = target as i64 - dialog.selected as i64;
    let arrow = if steps > 0 { "down" } else { "up" };
    let arrows: Vec<&'static str> = (0..steps.unsigned_abs()).map(|_| arrow).collect();
    if !arrows.is_empty() && io.send(&arrows).await.is_err() {
        // Arrows commit nothing; the selection may have moved, which the next answer re-reads.
        return Err(abort("sending the arrows failed"));
    }
    let mut verified = false;
    for _ in 0..SELECT_READS {
        tokio::time::sleep(SETTLE).await;
        let Ok(again) = look(io).await else {
            continue;
        };
        let same = again.occurrence == first.occurrence
            && again.process.same(&first.process)
            && again
                .dialog
                .as_ref()
                .is_some_and(|d| d.options.len() == dialog.options.len());
        if !same {
            return Err(abort("the dialog changed before Enter"));
        }
        if again.dialog.as_ref().is_some_and(|d| d.selected == target) {
            verified = true;
            break;
        }
    }
    if !verified {
        return Err(abort("the option never showed as selected"));
    }
    if let Some(why) = refuse() {
        return Err(abort(why));
    }
    answered.settle(&first.occurrence, Attempt::Sent);
    if let Err(e) = io.send(&["enter"]).await {
        return Err(ProtoError::new(
            ErrorCode::OutcomeUnknown,
            format!("Enter may have been sent: {e}"),
        ));
    }
    for _ in 0..CONFIRM_ATTEMPTS {
        tokio::time::sleep(CONFIRM_INTERVAL).await;
        if let Ok(after) = look(io).await
            && (after.dialog.is_none() || after.occurrence != first.occurrence)
        {
            if option.provisional {
                // Nothing was submitted, so this question may be picked again if it is shown again.
                answered.settle(&first.occurrence, Attempt::Stepped);
                if let Some(at) = open_step(&first, answered) {
                    answered.note_open(&after.occurrence, at + 1);
                }
            }
            return Ok(after);
        }
    }
    Err(ProtoError::new(
        ErrorCode::OutcomeUnknown,
        "Enter was sent but the dialog still shows",
    ))
}

/// Shows a group's previous question again with one left arrow, which submits nothing, and
/// returns what the screen then shows. The caller holds the pane's action lock throughout.
pub async fn back<I: DialogIo>(
    io: &I,
    refuse: Refusal<'_>,
    answered: &Answered,
    occurrence: &str,
) -> Result<Seen, ProtoError> {
    let first = look(io).await?;
    let Some(dialog) = &first.dialog else {
        return Err(changed("no dialog is showing"));
    };
    if first.occurrence != occurrence {
        return Err(changed("the dialog changed"));
    }
    if let Some(why) = refusal(&first, refuse, answered) {
        return Err(changed(format!("keys may not answer this dialog: {why}")));
    }
    let at = open_step(&first, answered)
        .filter(|_| may_go_back(&first, answered))
        .ok_or_else(|| {
            ProtoError::new(
                ErrorCode::BadRequest,
                "there is no earlier question to show",
            )
        })?;
    if io.send(&["left"]).await.is_err() {
        return Err(changed("sending the key failed"));
    }
    for _ in 0..SELECT_READS {
        tokio::time::sleep(SETTLE).await;
        let Ok(again) = look(io).await else {
            continue;
        };
        if !again.process.same(&first.process) {
            return Err(changed("the agent changed"));
        }
        let steps = again.dialog.as_ref().map_or(0, |d| d.steps.len());
        if again.occurrence != first.occurrence && steps == dialog.steps.len() {
            answered.note_open(&again.occurrence, at - 1);
            return Ok(again);
        }
    }
    Err(changed("the earlier question did not show"))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Attempt {
    /// Recorded before the first key; a runner that restarts here never retries it.
    Started,
    /// Nothing was committed: no Enter was sent.
    Aborted,
    /// Enter was or may have been sent.
    Sent,
    /// Enter went and a group moved to its next question: nothing was submitted.
    Stepped,
}

impl Attempt {
    /// Whether the occurrence may be answered again.
    fn open(self) -> bool {
        matches!(self, Attempt::Aborted | Attempt::Stepped)
    }
}

/// Every answer attempt by occurrence, written to disk before any key is sent.
#[derive(Default)]
pub struct Answered {
    path: Option<PathBuf>,
    attempts: Mutex<HashMap<String, (Attempt, u64)>>,
    /// Which step of its group an occurrence showed, where this runner's own key left it.
    open: Mutex<HashMap<String, usize>>,
}

impl Answered {
    pub fn load(path: PathBuf) -> Arc<Self> {
        let attempts = std::fs::read(&path)
            .ok()
            .and_then(|b| serde_json::from_slice(&b).ok())
            .unwrap_or_default();
        Arc::new(Self {
            path: Some(path),
            attempts: Mutex::new(attempts),
            open: Default::default(),
        })
    }

    fn open(&self, occurrence: &str) -> Option<usize> {
        self.open.lock().unwrap().get(occurrence).copied()
    }

    fn note_open(&self, occurrence: &str, step: usize) {
        let mut open = self.open.lock().unwrap();
        if open.len() >= KEEP_ATTEMPTS {
            open.clear();
        }
        open.insert(occurrence.to_owned(), step);
    }

    pub fn blocks(&self, occurrence: &str) -> bool {
        self.attempts
            .lock()
            .unwrap()
            .get(occurrence)
            .is_some_and(|(a, _)| !a.open())
    }

    /// Records a started attempt; `Err` if one already started or sent, or it can't be saved.
    pub fn begin(&self, occurrence: &str) -> Result<(), &'static str> {
        let mut attempts = self.attempts.lock().unwrap();
        if attempts.get(occurrence).is_some_and(|(a, _)| !a.open()) {
            return Err("this dialog was already answered");
        }
        attempts.insert(occurrence.to_owned(), (Attempt::Started, now_ms()));
        self.save(&mut attempts).map_err(|()| {
            attempts.remove(occurrence);
            "the attempt could not be recorded"
        })
    }

    pub fn settle(&self, occurrence: &str, attempt: Attempt) {
        let mut attempts = self.attempts.lock().unwrap();
        attempts.insert(occurrence.to_owned(), (attempt, now_ms()));
        if self.save(&mut attempts).is_err() {
            tracing::warn!("could not save a dialog answer attempt");
        }
    }

    /// A failed write means the attempt may not survive a restart, so nothing is sent.
    fn save(&self, attempts: &mut HashMap<String, (Attempt, u64)>) -> Result<(), ()> {
        if attempts.len() > KEEP_ATTEMPTS {
            let mut by_age: Vec<(String, u64)> = attempts
                .iter()
                .map(|(k, (_, at))| (k.clone(), *at))
                .collect();
            by_age.sort_by_key(|(_, at)| *at);
            for (k, _) in by_age.into_iter().take(attempts.len() - KEEP_ATTEMPTS) {
                attempts.remove(&k);
            }
        }
        let Some(path) = &self.path else {
            return Ok(());
        };
        let tmp = path.with_extension("json.tmp");
        let bytes = serde_json::to_vec(attempts).map_err(drop)?;
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir).map_err(drop)?;
        }
        std::fs::write(&tmp, bytes)
            .and_then(|()| std::fs::rename(&tmp, path))
            .map_err(drop)
    }
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as u64)
}

/// One action at a time per pane: dialog answers, prompts and the Codex key paths.
#[derive(Default)]
pub struct PaneLocks(Mutex<HashMap<String, Arc<tokio::sync::Mutex<()>>>>);

impl PaneLocks {
    pub async fn lock(
        &self,
        server: Option<&str>,
        pane_id: &str,
    ) -> tokio::sync::OwnedMutexGuard<()> {
        let key = format!("{}/{pane_id}", server.unwrap_or_default());
        let lock = self.0.lock().unwrap().entry(key).or_default().clone();
        lock.lock_owned().await
    }
}

/// An agent on a server, and its log, if it has one.
pub struct ServerPane {
    pub server: Server,
    pub target: String,
    pub log: Option<PathBuf>,
    /// Where the pending call is remembered, under the pane's key.
    pub logs: Arc<LogCache>,
    pub key: logcache::Key,
    /// The server's word for the block at the last `process`.
    pub said: Mutex<Option<String>>,
}

impl DialogIo for ServerPane {
    async fn process(&self) -> Result<Process, String> {
        let (p, said) = self
            .server
            .process(&self.target)
            .await
            .map_err(|e| e.to_string())?;
        *self.said.lock().unwrap() = said;
        Ok(p)
    }

    fn blocked_kind(&self) -> Option<String> {
        self.said.lock().unwrap().clone()
    }

    async fn screen(&self) -> Result<String, String> {
        self.server
            .read(&self.target, Read::Visible)
            .await
            .map(|r| r.text)
            .map_err(|e| e.to_string())
    }

    async fn pending_call(&self) -> Option<String> {
        let (log, logs, key) = (self.log.clone()?, self.logs.clone(), self.key.clone());
        tokio::task::spawn_blocking(move || logs.pending_call(&key, &log))
            .await
            .ok()
            .flatten()
    }

    async fn send(&self, keys: &[&'static str]) -> Result<(), String> {
        match &self.server {
            Server::Herdr { client, .. } => client
                .agent_send_keys(&self.target, keys.iter().map(|k| (*k).to_owned()).collect())
                .await
                .map_err(|e| e.to_string()),
            Server::Core(core) => {
                let address = self
                    .server
                    .address(&self.target)
                    .await
                    .map_err(|e| e.to_string())?;
                let keys: Vec<String> = keys.iter().map(|k| (*k).to_owned()).collect();
                core.client()
                    .send_keys(&address, &keys)
                    .await
                    .map(drop)
                    .map_err(|e| e.to_string())
            }
        }
    }
}

#[cfg(test)]
#[path = "dialogs_tests.rs"]
mod tests;
