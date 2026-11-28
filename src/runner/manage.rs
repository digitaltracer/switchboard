//! Renames the owner asks for from the phone: an agent Switchboard didn't start, a space, and
//! the tab of a session in the Herdr app. Each is one API call or one compare-and-set write of
//! a `name` in the app's layout. Nothing here types into a terminal.

use serde_json::{Value, json};

use super::backend::Server;
use super::core_watch::{self, Core};
use super::herdrs::qualified;
use super::sessions::{SESSION_LABEL, Sessions};
use crate::hc::layout::{self, LayoutError, Which};
use crate::hc::types::spaces;
use crate::herdr::{HerdrError, check_agent_name};
use crate::hub::route::check_name;
use crate::proto::{ErrorCode, ProtoError};

/// How often a layout write is tried again when someone else wrote in between.
const TRIES: u32 = 5;

fn err(code: ErrorCode, message: impl Into<String>) -> ProtoError {
    ProtoError::new(code, message)
}

fn layout_err(e: LayoutError) -> ProtoError {
    let code = match &e {
        LayoutError::Conflict => ErrorCode::Busy,
        LayoutError::Missing | LayoutError::NotFound(_) => ErrorCode::NotFound,
        LayoutError::Refused(_) => ErrorCode::BadRequest,
        LayoutError::Taken(_) => ErrorCode::NameTaken,
        LayoutError::Hc(e) if e.outcome_unknown() => ErrorCode::OutcomeUnknown,
        LayoutError::Hc(_) => ErrorCode::Herdr,
    };
    let message = match &e {
        LayoutError::Conflict => "the Herdr app's layout kept changing; try again".to_owned(),
        LayoutError::Missing => "the Herdr app has no layout on this machine yet".to_owned(),
        other => other.to_string(),
    };
    err(code, message)
}

fn herdr_rename_err(e: HerdrError) -> ProtoError {
    let code = match e.server_code() {
        Some("agent_name_taken") => ErrorCode::NameTaken,
        Some("invalid_agent_name") => ErrorCode::BadRequest,
        Some("agent_not_found" | "workspace_not_found") => ErrorCode::NotFound,
        // A Herdr session that isn't running has no such agent.
        _ if e.server_gone() => ErrorCode::NotFound,
        _ if e.outcome_unknown() => ErrorCode::OutcomeUnknown,
        _ if matches!(e, HerdrError::InvalidArgument(_)) => ErrorCode::BadRequest,
        _ => ErrorCode::Herdr,
    };
    err(code, e.to_string())
}

/// One layout write, tried again while someone else keeps writing. The mirror takes the result
/// at once, so the next listing shows the new name.
async fn write_layout(
    core: &Core,
    change: impl Fn(&mut Value) -> Result<(), LayoutError>,
) -> Result<(), ProtoError> {
    let client = core.client();
    let mut last = LayoutError::Conflict;
    for _ in 0..TRIES {
        match layout::update(&client, &change).await {
            Ok(doc) => {
                core.set_layout(&doc);
                return Ok(());
            }
            Err(LayoutError::Conflict) => continue,
            Err(e) => {
                last = e;
                break;
            }
        }
    }
    Err(layout_err(last))
}

impl Sessions {
    /// Renames an agent Switchboard didn't start, and returns its name as listed.
    pub(super) async fn rename_agent(&self, agent: &str, name: &str) -> Result<Value, ProtoError> {
        let (server, target, owned) = self.resolve_agent(agent);
        if owned.is_some() {
            return Err(err(
                ErrorCode::BadRequest,
                format!("{agent} is a Switchboard session; rename the session"),
            ));
        }
        match &server {
            Server::Herdr { session, client } => {
                check_agent_name(name).map_err(|_| {
                    err(
                        ErrorCode::BadRequest,
                        "a Herdr agent's name starts with a lowercase letter and has only lowercase letters, digits, - or _, at most 32",
                    )
                })?;
                let pane = client
                    .agent_get(target)
                    .await
                    .map_err(herdr_rename_err)?
                    .pane_id;
                client
                    .agent_rename(&pane, name)
                    .await
                    .map_err(herdr_rename_err)?;
                Ok(json!({ "name": qualified(session.as_deref(), name) }))
            }
            Server::Core(core) => {
                let name = check_name(name).map_err(|e| err(ErrorCode::BadRequest, e))?;
                let not_found = || err(ErrorCode::NotFound, format!("no agent {agent}"));
                let listed = server.agent(target).await.map_err(|_| not_found())?;
                // The mirror may lag: the same process must still be an agent there now.
                let now = core
                    .client()
                    .get(&listed.target)
                    .await
                    .map_err(|_| not_found())?;
                if core
                    .fresh(&now)
                    .is_none_or(|f| f.identity != listed.identity)
                {
                    return Err(not_found());
                }
                let all = core
                    .agents()
                    .await
                    .map_err(|e| err(ErrorCode::Herdr, format!("herdr-core: {e}")))?;
                // As in the Herdr app, a name need only be its own within its space.
                let same_space = |f: &super::backend::Facts| match (&f.space_id, &listed.space_id) {
                    (Some(a), Some(b)) => a == b,
                    _ => f.space == listed.space,
                };
                // Nor may it be how another is listed, as `<name>-<address>`.
                if all.iter().any(|f| {
                    f.target != listed.target
                        && ((same_space(f) && f.own_name.as_deref() == Some(name))
                            || (f.local_name() == name && f.own_name.as_deref() != Some(name)))
                }) {
                    return Err(err(
                        ErrorCode::NameTaken,
                        format!("another agent in this space is already called {name}"),
                    ));
                }
                write_layout(core, |doc| {
                    let id = core_watch::own_tab(&now, &spaces(doc))
                        .map(|tab| tab.id.clone())
                        .ok_or(LayoutError::Refused(
                            "this agent's name is fixed, or it shares its tab; rename it at the Mac",
                        ))?;
                    layout::set_name(doc, Which::Tab(&id), name)
                })
                .await?;
                Ok(json!({ "name": qualified(Some(core_watch::SERVER), name) }))
            }
        }
    }

    /// Renames a Herdr app space or a Herdr workspace, by the id the listing gave.
    pub(super) async fn rename_space(
        &self,
        herdr_session: Option<&str>,
        space_id: &str,
        name: &str,
    ) -> Result<Value, ProtoError> {
        let name = check_name(name).map_err(|e| err(ErrorCode::BadRequest, e))?;
        let gone = || err(ErrorCode::NotFound, "that space is no longer there");
        // Only a server that answers now, never a socket path built from the request.
        let server = self
            .servers()
            .await
            .into_iter()
            .find(|s| s.name() == herdr_session)
            .ok_or_else(gone)?;
        match &server {
            Server::Core(core) => {
                write_layout(core, |doc| {
                    let all = spaces(doc);
                    if core_watch::local_space(&all, space_id).is_none() {
                        return Err(LayoutError::NotFound("space"));
                    }
                    if all
                        .iter()
                        .any(|s| s.id != space_id && s.name.as_deref() == Some(name))
                    {
                        return Err(LayoutError::Taken("another space already has that name"));
                    }
                    layout::set_name(doc, Which::Space(space_id), name)
                })
                .await?;
            }
            Server::Herdr { client, .. } => {
                let snap = client.session_snapshot().await.map_err(herdr_rename_err)?;
                if !snap.workspaces.iter().any(|w| w.workspace_id == space_id) {
                    return Err(gone());
                }
                if snap
                    .workspaces
                    .iter()
                    .any(|w| w.workspace_id != space_id && w.label == name)
                {
                    return Err(err(
                        ErrorCode::NameTaken,
                        "another workspace in that Herdr session already has that name",
                    ));
                }
                client
                    .workspace_rename(space_id, name)
                    .await
                    .map_err(herdr_rename_err)?;
            }
        }
        Ok(json!({ "name": name }))
    }

    /// Names the tab a Herdr app session's terminal sits in. A Herdr terminal session has none.
    pub(super) async fn rename_session_tab(
        &self,
        session_id: &str,
        name: &str,
    ) -> Result<Value, ProtoError> {
        let name = check_name(name).map_err(|e| err(ErrorCode::BadRequest, e))?;
        let s = self
            .get(session_id)
            .ok_or_else(|| err(ErrorCode::NotFound, format!("no session {session_id}")))?;
        let unchanged = Ok(json!({ "renamed": false }));
        let (Server::Core(core), Some(address)) = (self.server_for(&s), s.pane_id.clone()) else {
            return unchanged;
        };
        // Only the terminal that still carries this session's label.
        let Ok(t) = core.client().get(&address).await else {
            return unchanged;
        };
        if t.labels.get(SESSION_LABEL) != Some(&s.session_id) || !t.is_local() {
            return unchanged;
        }
        write_layout(&core, |doc| {
            let id = tab_holding(&spaces(doc), &t.address, t.labels.get("herdr.tab"))
                .ok_or(LayoutError::NotFound("tab"))?;
            layout::set_name(doc, Which::Tab(&id), name)
        })
        .await?;
        Ok(json!({ "renamed": true }))
    }
}

/// The tab a terminal sits in: the one its `herdr.tab` label names, else the one whose panes
/// hold its address.
pub(super) fn tab_holding(
    spaces: &[crate::hc::types::SpaceView],
    address: &str,
    label: Option<&String>,
) -> Option<String> {
    let tabs = || spaces.iter().flat_map(|s| &s.tabs);
    label
        .and_then(|id| tabs().find(|t| &t.id == id))
        .or_else(|| tabs().find(|t| t.terminals.iter().any(|a| a == address)))
        .map(|t| t.id.clone())
}
