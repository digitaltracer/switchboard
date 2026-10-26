//! The app API's OpenAPI document, generated from the handlers and committed as
//! `api/openapi.json`. A test fails when the two drift; regenerate with
//! `switchboard admin openapi > api/openapi.json`.

use utoipa::openapi::security::{HttpAuthScheme, HttpBuilder, SecurityScheme};
use utoipa::{Modify, OpenApi};

use super::types::*;

#[derive(OpenApi)]
#[openapi(
    info(
        title = "Switchboard app API",
        version = "1",
        description = "REST and long-polls for the Switchboard phone apps. Approvals need a device signature; denials need only a device token."
    ),
    paths(
        super::get_meta,
        super::pair,
        super::pair_status,
        super::token_challenge,
        super::issue_token,
        super::list_projects,
        super::list_machines,
        super::list_sessions,
        super::start_session,
        super::get_operation,
        super::get_session,
        super::get_output,
        super::get_changes,
        super::get_transcript,
        super::prompt_session,
        super::stop_session,
        super::delete_session,
        super::mute_session,
        super::approval_challenge,
        super::decide_approval,
        super::list_devices,
        super::revoke_device,
        super::set_push_token,
        super::post_message,
        super::preview_message,
        super::list_agents,
        super::get_agent_output,
        super::get_agent_transcript,
    ),
    components(schemas(
        ApiError, ErrorBody, Meta, PairRequest, PairResponse, PairStatus, TokenChallengeRequest,
        Challenge, TokenRequest, TokenResponse, Project, Machine, Approval, Session, Sessions,
        StartRequest, StartResponse, OperationStatus, Output, PromptRequest,
        PromptResponse, StopRequest, StopResponse, MuteRequest, MuteResponse, DecisionKind,
        ApprovalChallenge, DecisionRequest, DecisionResponse, Device, PushEnvironment,
        PushTokenRequest, TargetRef, TargetView, MessageRequest, MessageChoice, MessageResponse,
        AgentView, Agents, ChangedFile, Changes, MessagePreviewRequest, MessagePreview,
        DeleteResponse, Transcript, TranscriptItem,
    )),
    modifiers(&DeviceToken),
)]
pub struct ApiDoc;

struct DeviceToken;

impl Modify for DeviceToken {
    fn modify(&self, api: &mut utoipa::openapi::OpenApi) {
        let components = api.components.get_or_insert_with(Default::default);
        components.add_security_scheme(
            "device_token",
            SecurityScheme::Http(HttpBuilder::new().scheme(HttpAuthScheme::Bearer).build()),
        );
    }
}

/// The spec as committed: pretty JSON with a trailing newline.
pub fn spec_json() -> String {
    let mut s = ApiDoc::openapi()
        .to_pretty_json()
        .expect("the spec serializes");
    s.push('\n');
    s
}
