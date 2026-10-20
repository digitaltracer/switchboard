//! The app API under `/app/v1`: REST and long-polls over the session service, for the phone
//! apps. Device tokens work here only, never on the bots' MCP endpoint. The OpenAPI spec is
//! generated from these handlers and committed as `api/openapi.json`.

pub mod openapi;
pub mod types;

use std::sync::Arc;
use std::time::Duration;

use axum::extract::{FromRequestParts, Path, Query, Request, State};
use axum::http::request::Parts;
use axum::http::{StatusCode, header};
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Response};
use axum::routing::{delete, get, post, put};
use axum::{Json, Router};
use tokio::time::Instant;

use super::service::{self, Caller, HubError, MAX_WAIT, SessionService};
use crate::auth::device::{AuthError, ClaimRequest, Devices, MESSAGE_VERSION};
use crate::cli::Agent;
use crate::proto::auth::bearer;
use crate::store::{DeviceRecord, DeviceState, PushEnv};
use types::*;

pub const PREFIX: &str = "/app/v1";
pub const API_VERSION: u32 = 1;
/// The oldest app build served; raise it only for a change old builds can't survive.
pub const MIN_CLIENT_BUILD: u32 = 1;
/// The header an app sends with its build number.
pub const BUILD_HEADER: &str = "x-switchboard-build";
const PAIR_POLL: Duration = Duration::from_millis(500);
const DEFAULT_OUTPUT_LINES: u32 = 80;

#[derive(Clone)]
pub struct AppState {
    pub svc: Arc<SessionService>,
    pub devices: Arc<Devices>,
}

pub fn routes(state: AppState) -> Router {
    let api = Router::new()
        .route("/pair", post(pair))
        .route("/pair/{device_id}", get(pair_status))
        .route("/auth/challenge", post(token_challenge))
        .route("/auth/token", post(issue_token))
        .route("/projects", get(list_projects))
        .route("/machines", get(list_machines))
        .route("/sessions", get(list_sessions).post(start_session))
        .route("/sessions/{id}", get(get_session))
        .route("/sessions/{id}/output", get(get_output))
        .route("/sessions/{id}/prompt", post(prompt_session))
        .route("/sessions/{id}/stop", post(stop_session))
        .route("/sessions/{id}/mute", put(mute_session))
        .route("/operations/{request_id}", get(get_operation))
        .route("/approvals/{id}/challenge", post(approval_challenge))
        .route("/approvals/{id}/decision", post(decide_approval))
        .route("/devices", get(list_devices))
        .route("/devices/{id}", delete(revoke_device))
        .route("/devices/self/push", put(set_push_token))
        .layer(middleware::from_fn(check_build))
        .route("/meta", get(get_meta));
    Router::new().nest(PREFIX, api).with_state(state)
}

/// Builds older than `MIN_CLIENT_BUILD` get `426 client_too_old`; `/meta` always answers.
async fn check_build(req: Request, next: Next) -> Response {
    let build = req
        .headers()
        .get(BUILD_HEADER)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.parse::<u32>().ok());
    if build.is_some_and(|b| b < MIN_CLIENT_BUILD) {
        return AppError::new(
            StatusCode::UPGRADE_REQUIRED,
            "client_too_old",
            format!("this hub needs app build {MIN_CLIENT_BUILD} or later"),
        )
        .into_response();
    }
    next.run(req).await
}

// Errors.

pub struct AppError {
    status: StatusCode,
    body: ErrorBody,
}

impl AppError {
    fn new(status: StatusCode, code: &str, message: impl Into<String>) -> Self {
        Self {
            status,
            body: ErrorBody {
                code: code.to_owned(),
                message: message.into(),
                operation_id: None,
                session_id: None,
            },
        }
    }
}

fn status_for(code: &str) -> StatusCode {
    match code {
        "invalid_token" => StatusCode::UNAUTHORIZED,
        "approve_not_allowed" => StatusCode::FORBIDDEN,
        "unknown_session" | "unknown_approval" | "unknown_device" | "unknown_operation"
        | "not_found" => StatusCode::NOT_FOUND,
        "session_not_running" | "request_id_reused" | "answer_in_herdr" | "agent_blocked" => {
            StatusCode::CONFLICT
        }
        "rate_limited" | "busy" => StatusCode::TOO_MANY_REQUESTS,
        "machine_offline" | "reconciling" => StatusCode::SERVICE_UNAVAILABLE,
        "outcome_unknown" | "deadline_exceeded" => StatusCode::GATEWAY_TIMEOUT,
        "internal" | "herdr" => StatusCode::INTERNAL_SERVER_ERROR,
        _ => StatusCode::BAD_REQUEST,
    }
}

impl From<HubError> for AppError {
    fn from(e: HubError) -> Self {
        let code = e.code();
        let mut err = AppError::new(status_for(&code), &code, e.to_string());
        if let HubError::OutcomeUnknown {
            operation_id,
            session_id,
        } = e
        {
            err.body.operation_id = Some(operation_id);
            err.body.session_id = session_id;
        }
        err
    }
}

impl From<AuthError> for AppError {
    fn from(e: AuthError) -> Self {
        let code = e.code();
        AppError::new(status_for(code), code, e.to_string())
    }
}

impl From<crate::store::StoreError> for AppError {
    fn from(e: crate::store::StoreError) -> Self {
        HubError::from(e).into()
    }
}

impl IntoResponse for AppError {
    fn into_response(self) -> Response {
        if self.status.is_server_error() {
            tracing::warn!(code = %self.body.code, message = %self.body.message, "app API error");
        }
        (self.status, Json(ApiError { error: self.body })).into_response()
    }
}

type ApiResult<T> = Result<Json<T>, AppError>;

/// The device behind the request's bearer token.
pub struct DeviceAuth(pub DeviceRecord);

impl DeviceAuth {
    fn caller(&self) -> Caller {
        Caller::app(&self.0.id)
    }
}

impl FromRequestParts<AppState> for DeviceAuth {
    type Rejection = AppError;

    async fn from_request_parts(parts: &mut Parts, state: &AppState) -> Result<Self, AppError> {
        let token = parts
            .headers
            .get(header::AUTHORIZATION)
            .and_then(|v| v.to_str().ok())
            .and_then(bearer)
            .ok_or(AuthError::InvalidToken)?;
        Ok(DeviceAuth(state.devices.authenticate(token).await?))
    }
}

fn wait(ms: Option<u64>) -> Duration {
    Duration::from_millis(ms.unwrap_or(0)).min(MAX_WAIT)
}

fn bad(message: &str) -> AppError {
    AppError::new(StatusCode::BAD_REQUEST, "bad_request", message)
}

// Handlers. Each `utoipa::path` is the contract; `operation_id` names the generated client call.

#[utoipa::path(get, path = "/app/v1/meta", tag = "meta",
    responses((status = 200, description = "OK", body = Meta)))]
pub async fn get_meta(State(st): State<AppState>) -> ApiResult<Meta> {
    Ok(Json(Meta {
        api_version: API_VERSION,
        hub_id: st.devices.hub_id().await?,
        min_client_build: MIN_CLIENT_BUILD,
        message_version: MESSAGE_VERSION,
        max_wait_ms: MAX_WAIT.as_millis() as u64,
    }))
}

#[utoipa::path(post, path = "/app/v1/pair", tag = "pairing", request_body = PairRequest,
    responses((status = 200, description = "OK", body = PairResponse), (status = 400, description = "Bad request", body = ApiError),
              (status = 429, description = "Rate limited", body = ApiError)))]
pub async fn pair(
    State(st): State<AppState>,
    Json(req): Json<PairRequest>,
) -> ApiResult<PairResponse> {
    let claimed = st
        .devices
        .claim(ClaimRequest {
            code: &req.code,
            name: &req.name,
            auth_key: &req.auth_key,
            approval_key: &req.approval_key,
            client_build: req.client_build,
        })
        .await?;
    Ok(Json(PairResponse {
        device_id: claimed.device_id,
        fingerprint: claimed.fingerprint,
        hub_id: claimed.hub_id,
        state: DeviceState::Pending.as_str().to_owned(),
    }))
}

#[utoipa::path(get, path = "/app/v1/pair/{device_id}", tag = "pairing",
    params(("device_id" = String, Path), PairWaitQuery),
    responses((status = 200, description = "OK", body = PairStatus), (status = 404, description = "Not found", body = ApiError)))]
pub async fn pair_status(
    State(st): State<AppState>,
    Path(device_id): Path<String>,
    Query(q): Query<PairWaitQuery>,
) -> ApiResult<PairStatus> {
    let deadline = Instant::now() + wait(q.wait_ms);
    loop {
        let d = st
            .devices
            .device(&device_id)
            .await?
            .ok_or(AuthError::UnknownDevice)?;
        if d.state != DeviceState::Pending || Instant::now() >= deadline {
            return Ok(Json(PairStatus {
                state: d.state.as_str().to_owned(),
            }));
        }
        tokio::time::sleep(PAIR_POLL.min(deadline - Instant::now())).await;
    }
}

#[utoipa::path(post, path = "/app/v1/auth/challenge", tag = "auth",
    request_body = TokenChallengeRequest,
    responses((status = 200, description = "OK", body = Challenge), (status = 404, description = "Not found", body = ApiError)))]
pub async fn token_challenge(
    State(st): State<AppState>,
    Json(req): Json<TokenChallengeRequest>,
) -> ApiResult<Challenge> {
    let ch = st.devices.token_challenge(&req.device_id).await?;
    Ok(Json(Challenge {
        nonce: ch.nonce,
        expires_at: ch.expires_at,
        message_version: MESSAGE_VERSION,
        hub_id: st.devices.hub_id().await?,
        device_id: ch.device_id,
    }))
}

#[utoipa::path(post, path = "/app/v1/auth/token", tag = "auth", request_body = TokenRequest,
    responses((status = 200, description = "OK", body = TokenResponse), (status = 400, description = "Bad request", body = ApiError),
              (status = 404, description = "Not found", body = ApiError)))]
pub async fn issue_token(
    State(st): State<AppState>,
    Json(req): Json<TokenRequest>,
) -> ApiResult<TokenResponse> {
    let t = st
        .devices
        .issue_token(
            &req.device_id,
            &req.nonce,
            &req.signature,
            req.message_version,
        )
        .await?;
    Ok(Json(TokenResponse {
        access_token: t.access_token,
        expires_at: t.expires_at,
    }))
}

#[utoipa::path(get, path = "/app/v1/projects", tag = "sessions", security(("device_token" = [])),
    responses((status = 200, description = "OK", body = Vec<Project>), (status = 401, description = "Missing or expired device token", body = ApiError)))]
pub async fn list_projects(State(st): State<AppState>, _d: DeviceAuth) -> ApiResult<Vec<Project>> {
    Ok(Json(
        st.svc.projects().into_iter().map(Into::into).collect(),
    ))
}

#[utoipa::path(get, path = "/app/v1/machines", tag = "sessions", security(("device_token" = [])),
    responses((status = 200, description = "OK", body = Vec<Machine>), (status = 401, description = "Missing or expired device token", body = ApiError)))]
pub async fn list_machines(State(st): State<AppState>, _d: DeviceAuth) -> ApiResult<Vec<Machine>> {
    let default = st.svc.default_machine().to_owned();
    Ok(Json(
        st.svc
            .machines()
            .into_iter()
            .map(|m| Machine::of(m, &default))
            .collect(),
    ))
}

#[utoipa::path(get, path = "/app/v1/sessions", tag = "sessions", security(("device_token" = [])),
    params(WaitQuery),
    responses((status = 200, description = "OK", body = Sessions), (status = 401, description = "Missing or expired device token", body = ApiError)))]
pub async fn list_sessions(
    State(st): State<AppState>,
    _d: DeviceAuth,
    Query(q): Query<WaitQuery>,
) -> ApiResult<Sessions> {
    Ok(Json(
        st.svc
            .list_wait(q.after_revision, wait(q.wait_ms))
            .await?
            .into(),
    ))
}

#[utoipa::path(post, path = "/app/v1/sessions", tag = "sessions", security(("device_token" = [])),
    request_body = types::StartRequest,
    responses((status = 200, description = "OK", body = StartResponse), (status = 400, description = "Bad request", body = ApiError),
              (status = 503, description = "Machine offline or reconciling", body = ApiError), (status = 504, description = "Outcome unknown", body = ApiError)))]
pub async fn start_session(
    State(st): State<AppState>,
    d: DeviceAuth,
    Json(req): Json<types::StartRequest>,
) -> ApiResult<StartResponse> {
    if req.request_id.trim().is_empty() {
        return Err(bad("request_id is required"));
    }
    let agent = match req.agent.as_deref() {
        None => None,
        Some("claude") => Some(Agent::Claude),
        Some("codex") => Some(Agent::Codex),
        Some(other) => return Err(bad(&format!("unknown agent {other:?}"))),
    };
    let r = st
        .svc
        .start(
            &d.caller(),
            service::StartRequest {
                project_id: req.project_id,
                prompt: req.prompt,
                machine_id: req.machine_id,
                agent,
                notify: req.notify,
                request_id: Some(req.request_id),
            },
        )
        .await?;
    Ok(Json(r.into()))
}

#[utoipa::path(get, path = "/app/v1/operations/{request_id}", tag = "sessions",
    security(("device_token" = [])), params(("request_id" = String, Path)),
    responses((status = 200, description = "OK", body = OperationStatus), (status = 404, description = "Not found", body = ApiError)))]
pub async fn get_operation(
    State(st): State<AppState>,
    _d: DeviceAuth,
    Path(request_id): Path<String>,
) -> ApiResult<OperationStatus> {
    let op = st
        .svc
        .store()
        .get_operation(&request_id)
        .await?
        .ok_or_else(|| {
            AppError::new(
                StatusCode::NOT_FOUND,
                "unknown_operation",
                "no request with this id reached the hub",
            )
        })?;
    Ok(Json(op.into()))
}

#[utoipa::path(get, path = "/app/v1/sessions/{id}", tag = "sessions",
    security(("device_token" = [])), params(("id" = String, Path), WaitQuery),
    responses((status = 200, description = "OK", body = Session), (status = 404, description = "Not found", body = ApiError)))]
pub async fn get_session(
    State(st): State<AppState>,
    _d: DeviceAuth,
    Path(id): Path<String>,
    Query(q): Query<WaitQuery>,
) -> ApiResult<Session> {
    Ok(Json(
        st.svc
            .get(&id, q.after_revision, wait(q.wait_ms))
            .await?
            .into(),
    ))
}

#[utoipa::path(get, path = "/app/v1/sessions/{id}/output", tag = "sessions",
    security(("device_token" = [])), params(("id" = String, Path), OutputQuery),
    responses((status = 200, description = "OK", body = Output), (status = 404, description = "Not found", body = ApiError),
              (status = 503, description = "Machine offline or reconciling", body = ApiError)))]
pub async fn get_output(
    State(st): State<AppState>,
    _d: DeviceAuth,
    Path(id): Path<String>,
    Query(q): Query<OutputQuery>,
) -> ApiResult<Output> {
    let r = st
        .svc
        .read(&id, q.lines.unwrap_or(DEFAULT_OUTPUT_LINES))
        .await?;
    Ok(Json(Output {
        text: r.text,
        truncated: r.truncated,
    }))
}

#[utoipa::path(post, path = "/app/v1/sessions/{id}/prompt", tag = "sessions",
    security(("device_token" = [])), params(("id" = String, Path)), request_body = PromptRequest,
    responses((status = 200, description = "OK", body = PromptResponse), (status = 409, description = "Conflict", body = ApiError)))]
pub async fn prompt_session(
    State(st): State<AppState>,
    d: DeviceAuth,
    Path(id): Path<String>,
    Json(req): Json<PromptRequest>,
) -> ApiResult<PromptResponse> {
    let r = st
        .svc
        .prompt(&d.caller(), &id, &req.text, req.request_id.as_deref())
        .await?;
    Ok(Json(r.into()))
}

#[utoipa::path(post, path = "/app/v1/sessions/{id}/stop", tag = "sessions",
    security(("device_token" = [])), params(("id" = String, Path)), request_body = StopRequest,
    responses((status = 200, description = "OK", body = StopResponse), (status = 404, description = "Not found", body = ApiError)))]
pub async fn stop_session(
    State(st): State<AppState>,
    d: DeviceAuth,
    Path(id): Path<String>,
    Json(req): Json<StopRequest>,
) -> ApiResult<StopResponse> {
    let r = st
        .svc
        .stop(&d.caller(), &id, req.remove_worktree_if_clean)
        .await?;
    Ok(Json(r.into()))
}

#[utoipa::path(put, path = "/app/v1/sessions/{id}/mute", tag = "sessions",
    security(("device_token" = [])), params(("id" = String, Path)), request_body = MuteRequest,
    responses((status = 200, description = "OK", body = MuteResponse), (status = 404, description = "Not found", body = ApiError)))]
pub async fn mute_session(
    State(st): State<AppState>,
    d: DeviceAuth,
    Path(id): Path<String>,
    Json(req): Json<MuteRequest>,
) -> ApiResult<MuteResponse> {
    let r = st.svc.notify(&d.caller(), &id, req.muted).await?;
    Ok(Json(MuteResponse {
        muted: r.muted,
        notifies: r.notifies,
    }))
}

#[utoipa::path(post, path = "/app/v1/approvals/{id}/challenge", tag = "approvals",
    security(("device_token" = [])), params(("id" = String, Path)),
    responses((status = 200, description = "OK", body = ApprovalChallenge), (status = 404, description = "Not found", body = ApiError)))]
pub async fn approval_challenge(
    State(st): State<AppState>,
    d: DeviceAuth,
    Path(id): Path<String>,
) -> ApiResult<ApprovalChallenge> {
    st.svc
        .store()
        .get_approval(&id)
        .await?
        .ok_or_else(|| HubError::UnknownApproval(id.clone()))?;
    let ch = st.devices.approval_challenge(&d.0.id, &id).await?;
    Ok(Json(ApprovalChallenge {
        nonce: ch.nonce,
        expires_at: ch.expires_at,
        message_version: MESSAGE_VERSION,
        hub_id: st.devices.hub_id().await?,
        device_id: ch.device_id,
        approval_id: id,
    }))
}

#[utoipa::path(post, path = "/app/v1/approvals/{id}/decision", tag = "approvals",
    security(("device_token" = [])), params(("id" = String, Path)), request_body = DecisionRequest,
    responses((status = 200, description = "OK", body = DecisionResponse), (status = 400, description = "Bad request", body = ApiError),
              (status = 403, description = "Not allowed", body = ApiError), (status = 404, description = "Not found", body = ApiError),
              (status = 409, description = "Conflict", body = ApiError)))]
pub async fn decide_approval(
    State(st): State<AppState>,
    d: DeviceAuth,
    Path(id): Path<String>,
    Json(req): Json<DecisionRequest>,
) -> ApiResult<DecisionResponse> {
    let a = st
        .svc
        .store()
        .get_approval(&id)
        .await?
        .ok_or_else(|| HubError::UnknownApproval(id.clone()))?;
    let rid = req.request_id.as_deref();
    let r = match req.decision {
        DecisionKind::Deny => {
            st.svc
                .decide(&d.caller(), &a.session_id, &id, false, rid)
                .await?
        }
        DecisionKind::Allow => {
            if approval_kind(&a.tool_name) != "permission" {
                return Err(AppError::new(
                    StatusCode::CONFLICT,
                    "answer_in_herdr",
                    "questions and plans can't be answered from the app yet; answer it in Herdr",
                ));
            }
            let (Some(nonce), Some(sig)) = (&req.nonce, &req.signature) else {
                return Err(bad(
                    "an approve needs the challenge nonce and the device's signature",
                ));
            };
            let version = req.message_version.unwrap_or(0);
            let proof = st
                .devices
                .verify_approval(&d.0, &id, nonce, sig, version)
                .await?;
            st.svc
                .decide_signed(&d.caller(), &a.session_id, &id, proof, rid)
                .await?
        }
    };
    Ok(Json(r.into()))
}

#[utoipa::path(get, path = "/app/v1/devices", tag = "devices", security(("device_token" = [])),
    responses((status = 200, description = "OK", body = Vec<Device>), (status = 401, description = "Missing or expired device token", body = ApiError)))]
pub async fn list_devices(State(st): State<AppState>, d: DeviceAuth) -> ApiResult<Vec<Device>> {
    Ok(Json(
        st.devices
            .list()
            .await?
            .into_iter()
            .filter(|x| matches!(x.state, DeviceState::Active | DeviceState::Pending))
            .map(|x| Device::of(x, &d.0.id))
            .collect(),
    ))
}

#[utoipa::path(delete, path = "/app/v1/devices/{id}", tag = "devices",
    security(("device_token" = [])), params(("id" = String, Path)),
    responses((status = 204, description = "Done"), (status = 404, description = "Not found", body = ApiError)))]
pub async fn revoke_device(
    State(st): State<AppState>,
    d: DeviceAuth,
    Path(id): Path<String>,
) -> Result<StatusCode, AppError> {
    if st.devices.revoke(&id, &d.caller().actor).await? {
        Ok(StatusCode::NO_CONTENT)
    } else {
        Err(AuthError::UnknownDevice.into())
    }
}

#[utoipa::path(put, path = "/app/v1/devices/self/push", tag = "devices",
    security(("device_token" = [])), request_body = PushTokenRequest,
    responses((status = 204, description = "Done"), (status = 400, description = "Bad request", body = ApiError)))]
pub async fn set_push_token(
    State(st): State<AppState>,
    d: DeviceAuth,
    Json(req): Json<PushTokenRequest>,
) -> Result<StatusCode, AppError> {
    if let Some(t) = &req.token
        && (t.is_empty() || t.len() > 200 || !t.bytes().all(|b| b.is_ascii_hexdigit()))
    {
        return Err(bad("the push token must be hex"));
    }
    let env = match req.environment {
        PushEnvironment::Sandbox => PushEnv::Sandbox,
        PushEnvironment::Production => PushEnv::Production,
    };
    let token = req.token.as_deref().map(str::to_ascii_lowercase);
    st.svc
        .store()
        .set_push_token(&d.0.id, token.as_deref(), Some(env), st.svc.now())
        .await?;
    Ok(StatusCode::NO_CONTENT)
}

#[cfg(test)]
#[path = "tests.rs"]
mod tests;
