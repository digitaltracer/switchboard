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
        super::get_start_options,
        super::list_sessions,
        super::start_session,
        super::get_operation,
        super::get_session,
        super::get_output,
        super::get_changes,
        super::get_diff,
        super::get_transcript,
        super::get_skills,
        super::prompt_session,
        super::stop_session,
        super::delete_session,
        super::mute_session,
        super::approval_challenge,
        super::decide_approval,
        super::list_devices,
        super::revoke_device,
        super::set_push_token,
        super::set_live,
        super::clear_live,
        super::get_live,
        super::get_ship_buttons,
        super::get_activity,
        super::post_activity_summary,
        super::get_away_settings,
        super::set_away_settings,
        super::set_ship_buttons,
        super::post_message,
        super::preview_message,
        super::list_agents,
        super::get_agent_output,
        super::get_agent_transcript,
        super::get_agent_skills,
        super::get_agent_dialog,
        super::dialog_challenge,
        super::answer_dialog,
        super::dialog_back,
        super::get_attention,
    ),
    components(schemas(
        ApiError, ErrorBody, Meta, PairRequest, PairResponse, PairStatus, TokenChallengeRequest,
        Challenge, TokenRequest, TokenResponse, Project, Machine, StartOptions,
        BackendOption, Approval, Session, Sessions,
        StartRequest, StartResponse, OperationStatus, Output, PromptRequest,
        PromptResponse, StopRequest, StopResponse, MuteRequest, MuteResponse, DecisionKind,
        ApprovalChallenge, DecisionRequest, DecisionResponse, Device, PushEnvironment,
        PushTokenRequest, TargetRef, TargetView, MessageRequest, MessageChoice, MessageResponse,
        AgentView, Agents, AgentActivityView, EndedAgent, ChangedFile, Changes, MessagePreviewRequest, MessagePreview,
        DeleteResponse, Transcript, TranscriptItem, Skills, Skill, ApprovalQuestion, ApprovalOption,
        Dialog, DialogOption, AgentDialog, DialogChallengeRequest, DialogChallenge,
        DialogAnswerRequest, DialogAnswerResponse, DialogBackRequest, DialogBackResponse, Attention, PushProviderKind,
        LiveRegistrationRequest, LiveMode, LiveBoardView, LiveWaitingView,
        FileDiffView, DiffHunkView, DiffLineView,
        ShipButtonView, ShipScope, ShipButtons, SetShipButtonsRequest,
        Activity, AwayAgentView, AwayFactView, SummaryRequest, SummaryResponse, AwaySettingsView,
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
