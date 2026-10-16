use std::{collections::HashSet, io, time::Duration};

use aioduct::{
    RequestBuilderSend, SseDecoder, SseEvent,
    runtime::{ConnectorSend, RuntimePoll},
};
use axum::{
    Json, Router,
    body::{Body, Bytes},
    extract::{Path, Query, State},
    http::{HeaderMap, HeaderName, HeaderValue, StatusCode, Uri, header},
    middleware,
    response::{Html, Response},
    routing::{get, patch, post},
};
use bytes::BytesMut;
use chrono::Utc;
use futures_util::{Stream, StreamExt, stream};
use serde::Deserialize;
use serde_json::{Value, json};
use tokio::{
    sync::mpsc,
    time::{self, Instant},
};
use uuid::Uuid;

use crate::{
    AppState,
    antigravity_oauth::{
        apply_antigravity_headers, begin_antigravity_oauth, complete_antigravity_oauth,
        gemini_account_models, gemini_account_quota,
    },
    auth::{generate_secret, login, logout, me, require_admin},
    codex_catalog::{remote as codex_catalog_remote, upstream as codex_catalog_upstream},
    codex_context, codex_device_oauth,
    codex_subscription::{
        CodexSubscriptionAuthorization, codex_account_quota, codex_models_endpoint,
        codex_subscription_authorization, is_codex_device_oauth_auth, is_codex_subscription_auth,
    },
    error::AppError,
    gemini_code_assist::{
        build_code_assist_request, gemini_code_assist_authorization, gemini_code_assist_endpoint,
        is_antigravity_oauth_auth, unwrap_code_assist_response_bytes, unwrap_code_assist_sse_data,
    },
    models::{
        AnthropicModel, AnthropicModelListResponse, AntigravityOAuthStartRequest,
        AntigravityOAuthStartResponse, ApiKeyListResponse, ApiKeyResponse,
        CodexAccountQuotaResponse, CodexDeviceOAuthStartRequest, CodexDeviceOAuthStartResponse,
        CodexDeviceOAuthStatusResponse, CreateApiKeyRequest, CreateApiKeyResponse,
        CreateModelCatalogEntryRequest, CreateProviderAccountRequest,
        CreateProviderModelRouteRequest, Dashboard, GeminiAccountModelsResponse,
        GeminiAccountQuotaResponse, GeminiModel, GeminiModelListResponse, HealthResponse,
        MetricsResponse, ModelCatalogEntryResponse, ModelCatalogListResponse, OpenAiModel,
        OpenAiModelListResponse, ProviderAccount, ProviderAccountListResponse,
        ProviderAccountResponse, ProviderModelRouteListResponse, ProviderModelRouteResponse,
        ProviderPresetListResponse, RequestLogListResponse, RequestSummary,
        RoutableModelCatalogEntry, RoutableModelCatalogListResponse, UpdateApiKeyRequest,
        UpdateModelCatalogEntryRequest, UpdateProviderAccountRequest,
        UpdateProviderModelRouteRequest,
    },
    provider_catalog::provider_presets,
    relay_attempt::{
        HTTP_SSE_TRANSPORT, HTTP_TRANSPORT, RelayAttempt, RelayAttemptLog, TokenUsage,
        authenticate_relay_api_key,
    },
    relay_retry,
    routing::{RouteFailure, classify_transport_failure, classify_upstream_application_failure},
    session_affinity::{SessionAffinity, client_kind, extract as extract_session_affinity},
};

const DEFAULT_ANTHROPIC_VERSION: &str = "2023-06-01";
const MAX_OVERLOAD_RETRIES: u8 = relay_retry::MAX_RETRIES;
const FAILOVER_START_BUDGET: Duration = Duration::from_secs(30);
const STREAM_FAILOVER_BUFFER_BYTES: usize = 64 * 1024;
const STREAM_FAILOVER_WAIT: Duration = Duration::from_secs(1);

pub fn admin_routes(state: AppState) -> Router<AppState> {
    let protected = Router::new()
        .route("/auth/me", get(me))
        .route("/auth/logout", post(logout))
        .route("/dashboard", get(admin_dashboard))
        .route("/api-keys", get(list_api_keys).post(create_api_key))
        .route(
            "/api-keys/{id}",
            patch(update_api_key).delete(delete_api_key),
        )
        .route(
            "/provider-accounts",
            get(list_provider_accounts).post(create_provider_account),
        )
        .route(
            "/provider-accounts/{id}",
            patch(update_provider_account).delete(delete_provider_account),
        )
        .route(
            "/provider-accounts/{id}/gemini/models",
            get(get_gemini_account_models),
        )
        .route(
            "/provider-accounts/{id}/gemini/quota",
            get(get_gemini_account_quota),
        )
        .route(
            "/provider-accounts/{id}/codex/quota",
            get(get_codex_account_quota),
        )
        .route("/oauth/antigravity/start", post(start_antigravity_oauth))
        .route("/oauth/codex/device/start", post(start_codex_device_oauth))
        .route(
            "/oauth/codex/device/{flow_id}",
            get(get_codex_device_oauth_status),
        )
        .route("/provider-presets", get(list_provider_presets))
        .route(
            "/model-catalog",
            get(list_model_catalog).post(create_model_catalog_entry),
        )
        .route("/routable-model-catalog", get(list_routable_model_catalog))
        .route("/model-catalog/{id}", patch(update_model_catalog_entry))
        .route(
            "/provider-model-routes",
            get(list_provider_model_routes).post(create_provider_model_route),
        )
        .route(
            "/provider-model-routes/{id}",
            patch(update_provider_model_route).delete(delete_provider_model_route),
        )
        .route("/request-logs", get(list_request_logs))
        .route_layer(middleware::from_fn_with_state(state, require_admin));

    Router::new()
        .route("/auth/login", post(login))
        .route(
            "/oauth/antigravity/callback",
            get(antigravity_oauth_callback),
        )
        .merge(protected)
}

pub async fn health(State(state): State<AppState>) -> Json<HealthResponse> {
    Json(HealthResponse {
        status: "healthy".to_string(),
        service: "token-toxication".to_string(),
        version: env!("CARGO_PKG_VERSION").to_string(),
        uptime_seconds: (Utc::now() - state.started_at).num_seconds(),
        timestamp: Utc::now(),
    })
}

pub async fn metrics(State(state): State<AppState>) -> Result<Json<MetricsResponse>, AppError> {
    let dashboard = state.db.dashboard().await?;
    let (session_affinity, route_selections, request_outcomes) = state.relay_metrics.snapshot();
    Ok(Json(MetricsResponse {
        active_api_keys: dashboard.active_api_keys,
        total_api_keys: dashboard.total_api_keys,
        healthy_accounts: dashboard.healthy_accounts,
        total_accounts: dashboard.total_accounts,
        usage: dashboard.usage,
        session_affinity,
        route_selections,
        request_outcomes,
        timestamp: Utc::now(),
    }))
}

pub async fn relay_messages(
    State(state): State<AppState>,
    headers: HeaderMap,
    uri: Uri,
    body: Bytes,
) -> Result<Response, AppError> {
    relay_json_endpoint(state, headers, uri, body, WireApi::AnthropicMessages).await
}

pub async fn relay_openai_responses(
    State(state): State<AppState>,
    headers: HeaderMap,
    uri: Uri,
    body: Bytes,
) -> Result<Response, AppError> {
    relay_json_endpoint(state, headers, uri, body, WireApi::OpenAiResponses).await
}

pub async fn relay_openai_chat(
    State(state): State<AppState>,
    headers: HeaderMap,
    uri: Uri,
    body: Bytes,
) -> Result<Response, AppError> {
    relay_json_endpoint(state, headers, uri, body, WireApi::OpenAiChat).await
}

pub async fn relay_gemini_generate_content(
    State(state): State<AppState>,
    headers: HeaderMap,
    uri: Uri,
    Path(operation): Path<String>,
    body: Bytes,
) -> Result<Response, AppError> {
    let (model, method) = parse_gemini_model_operation(&operation)?;
    relay_gemini_endpoint(state, headers, uri, body, model, method).await
}

pub(crate) async fn resolve_codex_authorization(
    state: &AppState,
    account: &crate::models::ProviderAccountRecord,
) -> Result<CodexSubscriptionAuthorization, AppError> {
    if is_codex_device_oauth_auth(&account.account.auth_mode) {
        codex_device_oauth::authorization(
            &state.codex_device_oauth,
            &state.db,
            &state.http,
            account,
        )
        .await
    } else {
        codex_subscription_authorization(&state.db, &state.http, account).await
    }
}

/// Deterministically choose the subscription account whose catalog TT proxies.
///
/// Every candidate is an active, non-blocked Codex subscription account, so the
/// choice only needs to be stable across calls; the oldest account wins.
async fn codex_catalog_account(
    state: &AppState,
) -> Result<crate::models::ProviderAccountRecord, AppError> {
    let id = state
        .db
        .list_provider_accounts()
        .await?
        .into_iter()
        .filter(|account| {
            account.is_active
                && account.status != "blocked"
                && is_codex_subscription_auth(&account.auth_mode)
        })
        .min_by(|left, right| left.created_at.cmp(&right.created_at))
        .map(|account| account.id)
        .ok_or_else(|| {
            AppError::ServiceUnavailable("no Codex subscription account is available".into())
        })?;

    state
        .db
        .get_provider_account_record(&id)
        .await?
        .ok_or_else(|| {
            AppError::ServiceUnavailable("no Codex subscription account is available".into())
        })
}

/// `GET /openai/v1/codex/models`
///
/// Serves a Codex-native catalog so the client reports the account's real model
/// metadata instead of its stale bundled values. The payload is the account's own
/// upstream catalog with only the entries a client could not accept removed.
///
/// A client that cannot load this catalog has no models at all, because an
/// explicit catalog replaces the bundled one. The flow is built around that:
/// serve the stored copy while it is fresh, revalidate it after that, and fall
/// back to it whenever upstream fails.
pub async fn list_codex_models(
    State(state): State<AppState>,
    headers: HeaderMap,
    uri: Uri,
    Query(query): Query<CodexCatalogQuery>,
) -> Result<Response, AppError> {
    authenticate_relay_api_key(&state, &headers, uri.query()).await?;
    let account = codex_catalog_account(&state).await?;
    let account_id = account.account.id.clone();

    if let Some(cached) = state.codex_catalog_cache.fresh(&account_id) {
        tracing::debug!(
            account_id,
            age_ms = cached.age.as_millis() as u64,
            bytes = cached.body.len(),
            "served codex catalog from cache"
        );
        return Ok(json_body(cached.body));
    }

    let client_version =
        codex_catalog_remote::upstream_client_version(query.client_version.as_deref());
    let started = std::time::Instant::now();
    let fetched = fetch_and_render_catalog(&state, &account, &client_version).await;

    match fetched {
        Ok(Refreshed::Unchanged) => {
            state.codex_catalog_cache.touch(&account_id);
            let cached = state
                .codex_catalog_cache
                .get(&account_id)
                .expect("a 304 response implies a stored catalog");
            tracing::info!(
                account_id,
                client_version = client_version.as_str(),
                duration_ms = started.elapsed().as_millis() as u64,
                bytes = cached.body.len(),
                "codex catalog revalidated upstream"
            );
            Ok(json_body(cached.body))
        }
        Ok(Refreshed::Rendered { rendered, etag }) => {
            log_served_catalog(&account_id, &client_version, started, &rendered);
            let body = bytes::Bytes::from(rendered.body);
            state
                .codex_catalog_cache
                .store(&account_id, body.clone(), etag);
            Ok(json_body(body))
        }
        Err(error) => {
            // A transient upstream problem must not empty the client's model list.
            if let Some(cached) = state.codex_catalog_cache.get(&account_id) {
                tracing::warn!(
                    account_id,
                    client_version = client_version.as_str(),
                    duration_ms = started.elapsed().as_millis() as u64,
                    age_ms = cached.age.as_millis() as u64,
                    reason = %error,
                    "served the stored codex catalog after an upstream failure"
                );
                return Ok(json_body(cached.body));
            }
            tracing::warn!(
                account_id,
                client_version = client_version.as_str(),
                duration_ms = started.elapsed().as_millis() as u64,
                reason = %error,
                "codex catalog unavailable and no stored copy exists"
            );
            Err(AppError::ServiceUnavailable(
                "codex catalog is unavailable".to_string(),
            ))
        }
    }
}

/// Outcome of asking upstream for the catalog.
enum Refreshed {
    /// Upstream reported the stored copy is still current.
    Unchanged,
    Rendered {
        rendered: codex_catalog_upstream::RenderedCatalog,
        etag: Option<String>,
    },
}

/// Fetch the upstream catalog and reduce it to the body TT serves.
async fn fetch_and_render_catalog(
    state: &AppState,
    account: &crate::models::ProviderAccountRecord,
    client_version: &str,
) -> Result<Refreshed, AppError> {
    let authorization = resolve_codex_authorization(state, account).await?;
    let endpoint = codex_models_endpoint(&account.account.base_url)?;
    let etag = state.codex_catalog_cache.etag(&account.account.id);

    match codex_catalog_remote::fetch(
        &state.http,
        &endpoint,
        &authorization,
        client_version,
        etag.as_deref(),
    )
    .await?
    {
        codex_catalog_remote::FetchedCatalog::NotModified => Ok(Refreshed::Unchanged),
        codex_catalog_remote::FetchedCatalog::Ok { body, etag } => {
            let decoded: Value = serde_json::from_slice(&body)
                .map_err(|_| AppError::ServiceUnavailable("codex catalog was not JSON".into()))?;
            let models = codex_catalog_upstream::models_from_body(&decoded)
                .map_err(|error| AppError::ServiceUnavailable(error.to_string()))?;
            Ok(Refreshed::Rendered {
                rendered: codex_catalog_upstream::render_catalog(models),
                etag,
            })
        }
    }
}

/// Report what was served without logging any model body.
fn log_served_catalog(
    account_id: &str,
    client_version: &str,
    started: std::time::Instant,
    rendered: &codex_catalog_upstream::RenderedCatalog,
) {
    let dropped: Vec<&str> = rendered
        .dropped
        .iter()
        .map(|entry| entry.reason.code())
        .collect();
    if rendered.dropped.is_empty() {
        tracing::info!(
            account_id,
            client_version,
            duration_ms = started.elapsed().as_millis() as u64,
            bytes = rendered.body.len(),
            models = rendered.kept,
            "served codex catalog from upstream"
        );
    } else {
        tracing::warn!(
            account_id,
            client_version,
            duration_ms = started.elapsed().as_millis() as u64,
            bytes = rendered.body.len(),
            models = rendered.kept,
            dropped_models = rendered.dropped.len(),
            dropped_slugs = %rendered
                .dropped
                .iter()
                .map(|entry| entry.slug.as_str())
                .collect::<Vec<_>>()
                .join(","),
            dropped_reasons = %dropped.join(","),
            "served codex catalog with entries the client could not accept dropped"
        );
    }
}

/// A JSON response built from an already encoded catalog body.
///
/// `Bytes` moves into the body without copying, so serving a cached catalog is
/// just the response write.
fn json_body(body: bytes::Bytes) -> Response {
    Response::builder()
        .header(header::CONTENT_TYPE, "application/json")
        .body(axum::body::Body::from(body))
        .expect("a response with a static header always builds")
}

#[derive(Debug, Deserialize)]
pub struct CodexCatalogQuery {
    pub client_version: Option<String>,
}

pub async fn list_openai_models(
    State(state): State<AppState>,
    headers: HeaderMap,
    uri: Uri,
) -> Result<Json<OpenAiModelListResponse>, AppError> {
    authenticate_relay_api_key(&state, &headers, uri.query()).await?;
    Ok(Json(OpenAiModelListResponse {
        object: "list".to_string(),
        data: openai_models(&state).await?,
    }))
}

pub async fn get_openai_model(
    State(state): State<AppState>,
    headers: HeaderMap,
    uri: Uri,
    Path(model): Path<String>,
) -> Result<Json<OpenAiModel>, AppError> {
    authenticate_relay_api_key(&state, &headers, uri.query()).await?;
    let model = openai_models(&state)
        .await?
        .into_iter()
        .find(|entry| entry.id == model)
        .ok_or_else(|| AppError::NotFound("model not found".into()))?;
    Ok(Json(model))
}

pub async fn list_gemini_models(
    State(state): State<AppState>,
    headers: HeaderMap,
    uri: Uri,
) -> Result<Json<GeminiModelListResponse>, AppError> {
    authenticate_relay_api_key(&state, &headers, uri.query()).await?;
    Ok(Json(GeminiModelListResponse {
        models: gemini_models(&state).await?,
    }))
}

pub async fn get_gemini_model(
    State(state): State<AppState>,
    headers: HeaderMap,
    uri: Uri,
    Path(model): Path<String>,
) -> Result<Json<GeminiModel>, AppError> {
    authenticate_relay_api_key(&state, &headers, uri.query()).await?;
    let model_name = format!("models/{model}");
    let model = gemini_models(&state)
        .await?
        .into_iter()
        .find(|entry| entry.name == model_name)
        .ok_or_else(|| AppError::NotFound("model not found".into()))?;
    Ok(Json(model))
}

pub async fn list_anthropic_models(
    State(state): State<AppState>,
    headers: HeaderMap,
    uri: Uri,
) -> Result<Json<AnthropicModelListResponse>, AppError> {
    authenticate_relay_api_key(&state, &headers, uri.query()).await?;
    let data = anthropic_models(&state).await?;
    Ok(Json(AnthropicModelListResponse {
        first_id: data.first().map(|model| model.id.clone()),
        last_id: data.last().map(|model| model.id.clone()),
        data,
        has_more: false,
    }))
}

pub async fn get_anthropic_model(
    State(state): State<AppState>,
    headers: HeaderMap,
    uri: Uri,
    Path(model): Path<String>,
) -> Result<Json<AnthropicModel>, AppError> {
    authenticate_relay_api_key(&state, &headers, uri.query()).await?;
    let model = anthropic_models(&state)
        .await?
        .into_iter()
        .find(|entry| entry.id == model)
        .ok_or_else(|| AppError::NotFound("model not found".into()))?;
    Ok(Json(model))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum WireApi {
    AnthropicMessages,
    OpenAiChat,
    OpenAiResponses,
    GeminiGenerateContent,
}

impl WireApi {
    fn account_value(self) -> &'static str {
        match self {
            Self::AnthropicMessages => "anthropic-messages",
            Self::OpenAiChat => "openai-chat",
            Self::OpenAiResponses => "openai-responses",
            Self::GeminiGenerateContent => "gemini-generate-content",
        }
    }

    fn upstream_path(self) -> &'static str {
        match self {
            Self::AnthropicMessages => "/v1/messages",
            Self::OpenAiChat => "/chat/completions",
            Self::OpenAiResponses => "/v1/responses",
            Self::GeminiGenerateContent => "/v1beta/models",
        }
    }

    fn public_path(self) -> &'static str {
        match self {
            Self::AnthropicMessages => "/anthropic/v1/messages",
            Self::OpenAiChat => "/openai/v1/chat/completions",
            Self::OpenAiResponses => "/openai/v1/responses",
            Self::GeminiGenerateContent => "/gemini/v1beta/models",
        }
    }

    fn validate(self, value: &Value) -> Result<(), AppError> {
        match self {
            Self::AnthropicMessages | Self::OpenAiChat => validate_messages_request(value),
            Self::OpenAiResponses => validate_responses_request(value),
            Self::GeminiGenerateContent => validate_gemini_generate_content_request(value),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum GeminiMethod {
    GenerateContent,
    StreamGenerateContent,
}

impl GeminiMethod {
    fn as_str(self) -> &'static str {
        match self {
            Self::GenerateContent => "generateContent",
            Self::StreamGenerateContent => "streamGenerateContent",
        }
    }

    fn is_stream(self) -> bool {
        matches!(self, Self::StreamGenerateContent)
    }
}

async fn relay_json_endpoint(
    state: AppState,
    headers: HeaderMap,
    uri: Uri,
    body: Bytes,
    wire_api: WireApi,
) -> Result<Response, AppError> {
    let authenticated_attempt = RelayAttempt::authenticate(&state, &headers, uri.query()).await?;
    let original_request_json: Value = serde_json::from_slice(&body)
        .map_err(|error| AppError::BadRequest(format!("invalid JSON body: {error}")))?;
    wire_api.validate(&original_request_json)?;
    if wire_api == WireApi::OpenAiResponses {
        validate_responses_protocol(&headers, &original_request_json)?;
    }
    let model = original_request_json
        .get("model")
        .and_then(Value::as_str)
        .ok_or_else(|| AppError::BadRequest("request model is required".into()))?
        .to_string();
    let affinity = relay_session_affinity(
        &state,
        wire_api.account_value(),
        &headers,
        &original_request_json,
    );
    let is_stream = original_request_json
        .get("stream")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let replay_safe = request_is_replay_safe(wire_api, &original_request_json);
    let request_id = Uuid::new_v4().to_string();
    let retry_deadline = Instant::now() + state.relay_stream_max_duration;
    // Started at the first slot acquisition and extended by later queue waits,
    // so time spent queued for a running-request slot never eats the budget.
    let mut failover_deadline: Option<Instant> = None;
    let mut excluded_account_ids = HashSet::new();
    let mut last_rate_limit_response = None;
    let mut overload_retry_count = 0u8;
    let mut retry_account_id: Option<String> = None;

    'attempts: for attempt_index in 0..=MAX_OVERLOAD_RETRIES {
        let attempt = match if let Some(account_id) = retry_account_id.as_deref() {
            authenticated_attempt
                .select_account(
                    wire_api.account_value(),
                    &model,
                    affinity.as_ref(),
                    account_id,
                )
                .await
        } else {
            authenticated_attempt
                .select_excluding(
                    wire_api.account_value(),
                    &model,
                    affinity.as_ref(),
                    &excluded_account_ids,
                )
                .await
        } {
            Ok(attempt) => attempt,
            Err(error @ AppError::ServiceUnavailable(_)) => return Err(error),
            Err(error) => {
                if let Some((status, content_type, bytes, headers)) = last_rate_limit_response {
                    return response_with_upstream_headers(
                        status,
                        content_type,
                        &headers,
                        bytes,
                        "no-store",
                    );
                }
                return Err(error);
            }
        };
        let failover_deadline = *failover_deadline.insert(match failover_deadline {
            Some(deadline) => deadline + attempt.queue_wait(),
            None => Instant::now() + FAILOVER_START_BUDGET,
        });
        let upstream_model_id = attempt.selection().upstream_model_id.clone();
        let strip_params = attempt.selection().strip_params.clone();
        let account = attempt.selection().account.clone();
        let codex_context_headers = if is_codex_device_oauth_auth(&account.account.auth_mode) {
            codex_context::request_headers(&headers)
                .map_err(|error| AppError::BadRequest(error.to_string()))?
        } else {
            HeaderMap::new()
        };
        let mut request_json = original_request_json.clone();
        request_json["model"] = Value::String(upstream_model_id);
        enable_stream_usage_if_supported(&mut request_json, wire_api, &account.account.provider);
        let stripped_params = strip_upstream_params(
            &mut request_json,
            &strip_params,
            wire_api,
            &account.account.auth_mode,
        );
        let body = serde_json::to_vec(&request_json)
            .map_err(|error| AppError::Internal(format!("serialize upstream request: {error}")))?;
        let request_summary =
            build_request_summary(&request_json, body.len() as u64, stripped_params);
        let base_upstream_url = upstream_url(&account.account.base_url, wire_api.upstream_path());
        let mut log = RelayAttemptLog {
            request_id: request_id.clone(),
            path: wire_api.public_path().to_string(),
            upstream_url: Some(base_upstream_url.clone()),
            request_summary: Some(request_summary),
            transport: if wire_api == WireApi::OpenAiResponses && is_stream {
                HTTP_SSE_TRANSPORT
            } else {
                HTTP_TRANSPORT
            },
            overload_retry_count,
            overload_retry_started: overload_retry_count > 0,
            overload_retry_exhausted: false,
            overload_retry_skip_reason: None,
            client_retry_advice_injected: false,
            attempt_index,
            output_started: false,
            retry_after_ms: None,
        };
        let codex_auth = if is_codex_subscription_auth(&account.account.auth_mode) {
            if wire_api != WireApi::OpenAiResponses {
                return Err(AppError::BadRequest(
                    "Codex subscription providers only support openai-responses routes".into(),
                ));
            }
            match resolve_codex_authorization(&state, &account).await {
                Ok(auth) => Some(auth),
                Err(error) => {
                    let status = error.status();
                    let failure =
                        if matches!(status, StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN) {
                            RouteFailure {
                                provider_status: Some("blocked"),
                                route_status: "degraded",
                                cooldown_until: None,
                                limit_kind: None,
                                error: error.to_string(),
                                status_code: Some(status.as_u16()),
                            }
                        } else {
                            classify_transport_failure(error.to_string(), Utc::now())
                        };
                    attempt.record_failure(&log, failure).await?;
                    return Err(error);
                }
            }
        } else {
            None
        };

        let upstream_url = codex_auth
            .as_ref()
            .map(|auth| auth.endpoint.clone())
            .unwrap_or(base_upstream_url);
        log.upstream_url = Some(upstream_url.clone());
        let request = match state.http.post(&upstream_url) {
            Ok(request) => request
                .header(
                    header::CONTENT_TYPE,
                    HeaderValue::from_static("application/json"),
                )
                .body(body),
            Err(error) => {
                attempt
                    .record_failure(
                        &log,
                        classify_transport_failure(error.to_string(), Utc::now()),
                    )
                    .await?;
                return Err(AppError::Upstream(error));
            }
        };
        let request = apply_protocol_headers(request, wire_api, &codex_context_headers, &headers);
        let request = match codex_auth.as_ref() {
            Some(auth) => apply_codex_subscription_auth(request, auth),
            None => apply_provider_auth(request, &account.account.auth_mode, &account.api_key),
        };
        let request = match request {
            Ok(request) => request,
            Err(error) => {
                attempt
                    .record_failure(
                        &log,
                        classify_transport_failure(error.to_string(), Utc::now()),
                    )
                    .await?;
                return Err(error);
            }
        };

        let response = match attempt.send_unrecorded(request).await {
            Ok(response) => response,
            Err(error)
                if codex_auth.is_some()
                    && replay_safe
                    && attempt_index < MAX_OVERLOAD_RETRIES
                    && Instant::now() <= retry_deadline
                    && Instant::now()
                        .checked_add(relay_retry::maximum_delay(attempt_index + 1, None))
                        .is_some_and(|deadline| deadline <= retry_deadline) =>
            {
                let error_text = "upstream transport failed before response";
                attempt
                    .record_application_failure(
                        &log,
                        StatusCode::BAD_GATEWAY,
                        TokenUsage::default(),
                        error_text.into(),
                    )
                    .await?;
                overload_retry_count += 1;
                retry_account_id = Some(account.account.id.clone());
                drop(attempt);
                if !wait_for_codex_transient_retry(
                    &state,
                    attempt_index + 1,
                    Some("transport"),
                    None,
                    retry_deadline,
                )
                .await?
                {
                    return Err(AppError::Upstream(error));
                }
                continue;
            }
            Err(error) => {
                attempt
                    .record_failure(
                        &log,
                        classify_transport_failure(error.to_string(), Utc::now()),
                    )
                    .await?;
                return Err(AppError::Upstream(error));
            }
        };
        let status = response.status();
        let response_headers = response.headers().clone();
        log.retry_after_ms = relay_retry::retry_after(&response_headers, Utc::now())
            .map(|delay| delay.as_millis() as u64);
        let content_type = response
            .headers()
            .get(header::CONTENT_TYPE)
            .cloned()
            .unwrap_or_else(|| HeaderValue::from_static("application/json"));

        if !status.is_success() {
            let bytes = response.bytes().await?;
            let usage = parse_usage(&bytes);
            let failure_code = upstream_failure_code(&bytes);
            let transient = codex_auth.is_some()
                && status.is_server_error()
                && relay_retry::retryable_provider_code(failure_code.as_deref());
            if transient {
                let retry_after = relay_retry::retry_after(&response_headers, Utc::now());
                if attempt_index < MAX_OVERLOAD_RETRIES
                    && replay_safe
                    && Instant::now() <= retry_deadline
                    && Instant::now()
                        .checked_add(relay_retry::maximum_delay(attempt_index + 1, retry_after))
                        .is_some_and(|deadline| deadline <= retry_deadline)
                {
                    let error = format!(
                        "upstream request failed ({})",
                        failure_code.as_deref().unwrap_or("transient")
                    );
                    attempt
                        .record_application_failure(&log, status, usage, error)
                        .await?;
                    overload_retry_count += 1;
                    retry_account_id = Some(account.account.id.clone());
                    drop(attempt);
                    if !wait_for_codex_transient_retry(
                        &state,
                        attempt_index + 1,
                        failure_code.as_deref(),
                        retry_after,
                        retry_deadline,
                    )
                    .await?
                    {
                        return response_with_upstream_headers(
                            status,
                            content_type,
                            &response_headers,
                            bytes,
                            "no-store",
                        );
                    }
                    continue;
                }
                log.overload_retry_started = overload_retry_count > 0;
                log.overload_retry_exhausted = attempt_index >= MAX_OVERLOAD_RETRIES;
                log.overload_retry_skip_reason = Some(
                    if log.overload_retry_exhausted {
                        "retry_budget_exhausted"
                    } else if Instant::now() > failover_deadline {
                        "retry_deadline_exhausted"
                    } else if !replay_safe {
                        "request_not_replay_safe"
                    } else {
                        "overload_not_replayable"
                    }
                    .to_string(),
                );
                attempt
                    .record_application_failure(
                        &log,
                        status,
                        usage,
                        format!(
                            "upstream request failed ({})",
                            failure_code.as_deref().unwrap_or("transient")
                        ),
                    )
                    .await?;
                return response_with_upstream_headers(
                    status,
                    content_type,
                    &response_headers,
                    bytes,
                    "no-store",
                );
            }
            attempt
                .record_response(&log, status, &response_headers, &bytes, usage)
                .await?;
            if status == StatusCode::TOO_MANY_REQUESTS
                && attempt_index < 2
                && replay_safe
                && Instant::now() <= failover_deadline
            {
                excluded_account_ids.insert(account.account.id);
                retry_account_id = None;
                last_rate_limit_response = Some((
                    status,
                    content_type,
                    bytes,
                    codex_response_headers(&response_headers),
                ));
                continue;
            }
            return response_with_upstream_headers(
                status,
                content_type,
                &response_headers,
                bytes,
                "no-store",
            );
        }

        if is_stream {
            let mut upstream_stream = response.into_bytes_stream();
            let client_response_headers = codex_response_headers(&response_headers);
            let mut buffered = Vec::new();
            let mut buffered_bytes = 0;
            let mut observation = StreamingResponseObservation::default();
            let preflight_deadline = Instant::now() + STREAM_FAILOVER_WAIT;
            let mut preflight_timed_out = false;
            while buffered_bytes < STREAM_FAILOVER_BUFFER_BYTES
                && !observation.output_started
                && observation.application_failure.is_none()
            {
                let next = match time::timeout_at(preflight_deadline, upstream_stream.next()).await
                {
                    Ok(next) => next,
                    Err(_) => {
                        preflight_timed_out = true;
                        break;
                    }
                };
                match next {
                    Some(Ok(chunk)) => {
                        buffered_bytes += chunk.len();
                        observation.observe(&chunk, Instant::now());
                        buffered.push(chunk);
                    }
                    Some(Err(error)) => {
                        if codex_auth.is_some()
                            && replay_safe
                            && !observation.output_started
                            && attempt_index < MAX_OVERLOAD_RETRIES
                            && Instant::now() <= retry_deadline
                            && Instant::now()
                                .checked_add(relay_retry::maximum_delay(attempt_index + 1, None))
                                .is_some_and(|deadline| deadline <= retry_deadline)
                        {
                            attempt
                                .record_application_failure(
                                    &log,
                                    StatusCode::BAD_GATEWAY,
                                    observation.tokens(),
                                    "upstream stream transport failed before output".into(),
                                )
                                .await?;
                            drop(upstream_stream);
                            drop(attempt);
                            overload_retry_count += 1;
                            retry_account_id = Some(account.account.id.clone());
                            if !wait_for_codex_transient_retry(
                                &state,
                                attempt_index + 1,
                                Some("transport"),
                                None,
                                retry_deadline,
                            )
                            .await?
                            {
                                return Err(AppError::Upstream(error));
                            }
                            continue 'attempts;
                        }
                        attempt
                            .record_failure(
                                &log,
                                classify_transport_failure(error.to_string(), Utc::now()),
                            )
                            .await?;
                        return Err(AppError::Upstream(error));
                    }
                    None => break,
                }
            }
            if let Some(failure) = observation
                .replayable_application_failure(wire_api)
                .cloned()
            {
                if codex_auth.is_some()
                    && failure.retryable
                    && !observation.output_started
                    && attempt_index < MAX_OVERLOAD_RETRIES
                    && replay_safe
                    && Instant::now() <= retry_deadline
                    && Instant::now()
                        .checked_add(relay_retry::maximum_delay(
                            attempt_index + 1,
                            failure.retry_after,
                        ))
                        .is_some_and(|deadline| deadline <= retry_deadline)
                    && (!is_limit_failure_code(failure.code.as_deref())
                        || Instant::now() <= failover_deadline)
                {
                    log.retry_after_ms = failure.retry_after.map(|delay| delay.as_millis() as u64);
                    let status = classify_upstream_application_failure(
                        failure.code.as_deref(),
                        failure.log_error(),
                        Utc::now(),
                    )
                    .and_then(|route_failure| route_failure.status_code)
                    .and_then(|status| StatusCode::from_u16(status).ok())
                    .unwrap_or(StatusCode::BAD_GATEWAY);
                    attempt
                        .record_application_failure(
                            &log,
                            status,
                            observation.tokens(),
                            failure.log_error(),
                        )
                        .await?;
                    drop(upstream_stream);
                    drop(attempt);
                    overload_retry_count += 1;
                    retry_account_id = Some(account.account.id.clone());
                    if !wait_for_codex_transient_retry(
                        &state,
                        attempt_index + 1,
                        failure.code.as_deref(),
                        failure.retry_after,
                        retry_deadline,
                    )
                    .await?
                    {
                        break;
                    }
                    continue;
                }
                if codex_auth.is_some() && failure.retryable {
                    log.overload_retry_exhausted = attempt_index >= MAX_OVERLOAD_RETRIES;
                    log.overload_retry_skip_reason = Some(
                        if log.overload_retry_exhausted {
                            "retry_budget_exhausted"
                        } else if Instant::now() > failover_deadline {
                            "retry_deadline_exhausted"
                        } else if !replay_safe {
                            "request_not_replay_safe"
                        } else {
                            "overload_not_replayable"
                        }
                        .to_string(),
                    );
                }
                let error = failure.log_error();
                if let Some(route_failure) = classify_upstream_application_failure(
                    failure.code.as_deref(),
                    error,
                    Utc::now(),
                ) && route_failure.limit_kind.is_some()
                    && attempt_index < 2
                    && replay_safe
                    && Instant::now() <= failover_deadline
                {
                    attempt
                        .record_failure_with_usage(&log, route_failure, observation.tokens())
                        .await?;
                    excluded_account_ids.insert(account.account.id);
                    last_rate_limit_response = Some((
                        StatusCode::TOO_MANY_REQUESTS,
                        HeaderValue::from_static("application/json"),
                        Bytes::from_static(
                            br#"{"error":{"type":429,"message":"all matching provider accounts are rate limited"}}"#,
                        ),
                        HeaderMap::new(),
                    ));
                    continue;
                }
            }

            let buffered_stream =
                stream::iter(buffered.into_iter().map(Ok::<Bytes, std::io::Error>));
            let remaining_stream = stream::unfold(upstream_stream, |mut body| async move {
                body.next().await.map(|chunk| {
                    let chunk = chunk.map_err(std::io::Error::other);
                    (chunk, body)
                })
            });
            let body_stream = buffered_stream.chain(remaining_stream);
            let body_stream = record_streaming_response(
                body_stream,
                StreamingResponseContext {
                    attempt,
                    log,
                    status,
                    response_headers,
                    wire_api,
                    codex_subscription: codex_auth.is_some(),
                    preflight_timed_out,
                    first_output_at: observation.first_output_at,
                    idle_timeout: state.relay_stream_idle_timeout,
                    max_duration: state.relay_stream_max_duration,
                    shutdown: state.shutdown.clone(),
                },
            );
            let body = Body::from_stream(body_stream);
            let mut relay = Response::builder()
                .status(status)
                .header(header::CONTENT_TYPE, content_type)
                .body(body)
                .map_err(|error| AppError::Internal(error.to_string()))?;
            relay.headers_mut().insert(
                header::CACHE_CONTROL,
                HeaderValue::from_static("no-cache, no-transform"),
            );
            codex_context::copy_response_headers(&client_response_headers, relay.headers_mut());
            return Ok(relay);
        }

        let bytes = response.bytes().await?;
        let usage = parse_usage(&bytes);
        attempt
            .record_response(&log, status, &response_headers, &bytes, usage)
            .await?;
        return response_with_upstream_headers(
            status,
            content_type,
            &response_headers,
            bytes,
            "no-store",
        );
    }

    Err(AppError::TooManyRequests(
        "all matching provider accounts are rate limited".to_string(),
    ))
}

fn request_is_replay_safe(wire_api: WireApi, request: &Value) -> bool {
    wire_api != WireApi::OpenAiResponses
        || request
            .get("previous_response_id")
            .is_none_or(Value::is_null)
}

fn upstream_failure_code(bytes: &[u8]) -> Option<String> {
    let value = serde_json::from_slice::<Value>(bytes).ok()?;
    let error = value.get("error").unwrap_or(&value);
    error
        .get("code")
        .or_else(|| error.get("type"))
        .and_then(Value::as_str)
        .and_then(relay_retry::normalize_provider_code)
}

fn is_limit_failure_code(code: Option<&str>) -> bool {
    matches!(
        code,
        Some(
            "rate_limit_exceeded"
                | "too_many_requests"
                | "usage_limit_reached"
                | "quota_exhausted"
                | "insufficient_quota"
        )
    )
}

fn response_with_bytes(
    status: StatusCode,
    content_type: HeaderValue,
    bytes: Bytes,
    cache_control: &'static str,
) -> Result<Response, AppError> {
    let mut relay = Response::builder()
        .status(status)
        .header(header::CONTENT_TYPE, content_type)
        .body(Body::from(bytes))
        .map_err(|error| AppError::Internal(error.to_string()))?;
    relay.headers_mut().insert(
        header::CACHE_CONTROL,
        HeaderValue::from_static(cache_control),
    );
    Ok(relay)
}

fn response_with_upstream_headers(
    status: StatusCode,
    content_type: HeaderValue,
    upstream_headers: &HeaderMap,
    bytes: Bytes,
    cache_control: &'static str,
) -> Result<Response, AppError> {
    let mut relay = response_with_bytes(status, content_type, bytes, cache_control)?;
    codex_context::copy_response_headers(upstream_headers, relay.headers_mut());
    Ok(relay)
}

fn codex_response_headers(headers: &HeaderMap) -> HeaderMap {
    let mut copied = HeaderMap::new();
    codex_context::copy_response_headers(headers, &mut copied);
    copied
}

async fn relay_gemini_endpoint(
    state: AppState,
    headers: HeaderMap,
    uri: Uri,
    body: Bytes,
    model: String,
    method: GeminiMethod,
) -> Result<Response, AppError> {
    let authenticated_attempt = RelayAttempt::authenticate(&state, &headers, uri.query()).await?;
    let mut request_json: Value = serde_json::from_slice(&body)
        .map_err(|error| AppError::BadRequest(format!("invalid JSON body: {error}")))?;
    validate_gemini_generate_content_request(&request_json)?;
    let affinity = relay_session_affinity(
        &state,
        WireApi::GeminiGenerateContent.account_value(),
        &headers,
        &request_json,
    );
    let attempt = authenticated_attempt
        .select(
            WireApi::GeminiGenerateContent.account_value(),
            &model,
            affinity.as_ref(),
        )
        .await?;
    let upstream_model_id = attempt.selection().upstream_model_id.clone();
    let public_path = gemini_public_path(&attempt.selection().public_model_id, method);
    let strip_params = attempt.selection().strip_params.clone();
    let account = attempt.selection().account.clone();
    let stripped_params = strip_top_level_params(&mut request_json, &strip_params);
    if !is_antigravity_oauth_auth(&account.account.auth_mode) {
        return Err(AppError::BadRequest(
            "Gemini native providers require Antigravity OAuth credentials".into(),
        ));
    }
    let fallback_upstream_url = gemini_code_assist_upstream_url(
        &gemini_code_assist_endpoint(&account.account.base_url),
        method,
        uri.query(),
    );
    let mut log = RelayAttemptLog {
        request_id: Uuid::new_v4().to_string(),
        path: public_path,
        upstream_url: Some(fallback_upstream_url),
        request_summary: None,
        transport: HTTP_TRANSPORT,
        overload_retry_count: 0,
        overload_retry_started: false,
        overload_retry_exhausted: false,
        overload_retry_skip_reason: None,
        client_retry_advice_injected: false,
        attempt_index: 0,
        output_started: false,
        retry_after_ms: None,
    };

    let authorization =
        match gemini_code_assist_authorization(&state.db, &state.gemini_http, &account).await {
            Ok(authorization) => authorization,
            Err(error) => {
                let status = error.status();
                let failure = if matches!(status, StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN)
                {
                    RouteFailure {
                        provider_status: Some("blocked"),
                        route_status: "degraded",
                        cooldown_until: None,
                        limit_kind: None,
                        error: error.to_string(),
                        status_code: Some(status.as_u16()),
                    }
                } else {
                    classify_transport_failure(error.to_string(), Utc::now())
                };
                attempt.record_failure(&log, failure).await?;
                return Err(error);
            }
        };

    let session_id = request_json
        .get("sessionId")
        .or_else(|| request_json.get("session_id"))
        .and_then(Value::as_str);
    let code_assist_json = build_code_assist_request(
        &request_json,
        &upstream_model_id,
        authorization.project.as_deref(),
        session_id,
    );
    let body = serde_json::to_vec(&code_assist_json)
        .map_err(|error| AppError::Internal(format!("serialize upstream request: {error}")))?;
    let mut request_summary =
        build_request_summary(&request_json, body.len() as u64, stripped_params);
    request_summary.stream = method.is_stream();
    let upstream_url =
        gemini_code_assist_upstream_url(&authorization.endpoint, method, uri.query());
    log.upstream_url = Some(upstream_url.clone());
    log.request_summary = Some(request_summary.clone());

    let request = match state.gemini_http.post(&upstream_url) {
        Ok(request) => request
            .header(
                header::CONTENT_TYPE,
                HeaderValue::from_static("application/json"),
            )
            .body(body),
        Err(error) => {
            attempt
                .record_failure(
                    &log,
                    classify_transport_failure(error.to_string(), Utc::now()),
                )
                .await?;
            return Err(AppError::Upstream(error));
        }
    };
    let request = request.bearer_auth(&authorization.access_token);
    let request = apply_antigravity_headers(request)?;

    let response = attempt.send(request, &log).await?;
    let status = response.status();
    let response_headers = response.headers().clone();
    let content_type = response
        .headers()
        .get(header::CONTENT_TYPE)
        .cloned()
        .unwrap_or_else(|| HeaderValue::from_static("application/json"));

    if method.is_stream() {
        let stream_idle_timeout = state.relay_stream_idle_timeout;
        let stream_max_duration = state.relay_stream_max_duration;
        let shutdown = state.shutdown.clone();
        let body_stream = stream::unfold(
            (
                response.into_bytes_stream(),
                String::new(),
                status.is_success(),
            ),
            |(mut body, mut buffer, unwrap_events)| async move {
                loop {
                    if unwrap_events && let Some((event, rest)) = take_sse_event(&buffer) {
                        buffer = rest;
                        let chunk = transform_code_assist_sse_event(&event);
                        return Some((Ok(Bytes::from(chunk)), (body, buffer, unwrap_events)));
                    }

                    match body.next().await {
                        Some(Ok(chunk)) if unwrap_events => {
                            buffer.push_str(&String::from_utf8_lossy(&chunk));
                        }
                        Some(Ok(chunk)) => {
                            return Some((Ok(chunk), (body, buffer, unwrap_events)));
                        }
                        Some(Err(error)) => {
                            return Some((
                                Err(std::io::Error::other(error)),
                                (body, buffer, unwrap_events),
                            ));
                        }
                        None if unwrap_events && !buffer.is_empty() => {
                            let chunk = transform_code_assist_sse_event(&buffer);
                            buffer.clear();
                            return Some((Ok(Bytes::from(chunk)), (body, buffer, unwrap_events)));
                        }
                        None => return None,
                    }
                }
            },
        );
        let body_stream = record_streaming_response(
            body_stream,
            StreamingResponseContext {
                attempt,
                log,
                status,
                response_headers,
                wire_api: WireApi::GeminiGenerateContent,
                codex_subscription: false,
                preflight_timed_out: false,
                first_output_at: None,
                idle_timeout: stream_idle_timeout,
                max_duration: stream_max_duration,
                shutdown,
            },
        );
        let body = Body::from_stream(body_stream);
        let mut relay = Response::builder()
            .status(status)
            .header(header::CONTENT_TYPE, content_type)
            .body(body)
            .map_err(|error| AppError::Internal(error.to_string()))?;
        relay.headers_mut().insert(
            header::CACHE_CONTROL,
            HeaderValue::from_static("no-cache, no-transform"),
        );
        Ok(relay)
    } else {
        let upstream_bytes = response.bytes().await?;
        let relay_bytes = if status.is_success() {
            unwrap_code_assist_response_bytes(&upstream_bytes)?
        } else {
            upstream_bytes.to_vec()
        };
        let usage = parse_usage(&relay_bytes);
        let health_body = if status.is_success() {
            relay_bytes.as_slice()
        } else {
            upstream_bytes.as_ref()
        };
        attempt
            .record_response(&log, status, &response_headers, health_body, usage)
            .await?;
        let mut relay = Response::builder()
            .status(status)
            .header(header::CONTENT_TYPE, content_type)
            .body(Body::from(relay_bytes))
            .map_err(|error| AppError::Internal(error.to_string()))?;
        relay
            .headers_mut()
            .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
        Ok(relay)
    }
}

pub async fn admin_dashboard(State(state): State<AppState>) -> Result<Json<Dashboard>, AppError> {
    Ok(Json(state.db.dashboard().await?))
}

pub async fn list_api_keys(
    State(state): State<AppState>,
) -> Result<Json<ApiKeyListResponse>, AppError> {
    Ok(Json(ApiKeyListResponse {
        data: state.db.list_api_keys().await?,
    }))
}

pub async fn create_api_key(
    State(state): State<AppState>,
    Json(input): Json<CreateApiKeyRequest>,
) -> Result<(StatusCode, Json<CreateApiKeyResponse>), AppError> {
    if input.name.trim().is_empty() {
        return Err(AppError::BadRequest("API key name is required".into()));
    }
    let secret = generate_secret(&state.config.api_key_prefix);
    let key = state.db.create_api_key(input, &secret).await?;
    Ok((
        StatusCode::CREATED,
        Json(CreateApiKeyResponse { key, secret }),
    ))
}

pub async fn update_api_key(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(input): Json<UpdateApiKeyRequest>,
) -> Result<Json<ApiKeyResponse>, AppError> {
    let key = state
        .db
        .update_api_key(&id, input)
        .await
        .map_err(map_not_found)?;
    Ok(Json(ApiKeyResponse { data: key }))
}

pub async fn delete_api_key(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<StatusCode, AppError> {
    if !state.db.delete_api_key(&id).await? {
        return Err(AppError::NotFound("API key not found".into()));
    }
    Ok(StatusCode::NO_CONTENT)
}

pub async fn list_provider_accounts(
    State(state): State<AppState>,
) -> Result<Json<ProviderAccountListResponse>, AppError> {
    let mut accounts = state.db.list_provider_accounts().await?;
    for account in &mut accounts {
        with_account_load(&state, account);
    }
    Ok(Json(ProviderAccountListResponse { data: accounts }))
}

pub async fn start_antigravity_oauth(
    State(state): State<AppState>,
    Json(input): Json<AntigravityOAuthStartRequest>,
) -> Result<Json<AntigravityOAuthStartResponse>, AppError> {
    Ok(Json(
        begin_antigravity_oauth(&state.antigravity_oauth, input).await?,
    ))
}

pub async fn start_codex_device_oauth(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(input): Json<CodexDeviceOAuthStartRequest>,
) -> Result<Json<CodexDeviceOAuthStartResponse>, AppError> {
    let owner_token = crate::auth::extract_bearer(&headers)
        .ok_or_else(|| AppError::Unauthorized("missing admin bearer token".into()))?;
    Ok(Json(
        codex_device_oauth::begin(
            &state.codex_device_oauth,
            &state.http,
            &owner_token,
            input.name,
        )
        .await?,
    ))
}

pub async fn get_codex_device_oauth_status(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(flow_id): Path<String>,
) -> Result<Json<CodexDeviceOAuthStatusResponse>, AppError> {
    let owner_token = crate::auth::extract_bearer(&headers)
        .ok_or_else(|| AppError::Unauthorized("missing admin bearer token".into()))?;
    Ok(Json(
        codex_device_oauth::status(
            &state.codex_device_oauth,
            &state.db,
            &state.http,
            &owner_token,
            &flow_id,
        )
        .await?,
    ))
}

#[derive(Debug, Deserialize)]
pub struct AntigravityOAuthCallbackQuery {
    code: Option<String>,
    state: Option<String>,
    error: Option<String>,
    error_description: Option<String>,
}

pub async fn antigravity_oauth_callback(
    State(state): State<AppState>,
    Query(query): Query<AntigravityOAuthCallbackQuery>,
) -> Html<String> {
    let outcome = complete_antigravity_oauth(
        &state.antigravity_oauth,
        &state.db,
        &state.gemini_http,
        query.state.as_deref(),
        query.code.as_deref(),
        query.error.as_deref(),
        query.error_description.as_deref(),
    )
    .await;
    let success = outcome.error.is_none();
    let payload = json!({
        "type": "token-toxication:antigravity-oauth",
        "success": success,
        "accountId": outcome.account_id,
        "error": outcome.error,
    });
    let payload = serde_json::to_string(&payload).unwrap_or_else(|_| "{}".to_string());
    let notify_opener = outcome.opener_origin.map_or_else(String::new, |origin| {
        let origin = serde_json::to_string(&origin).unwrap_or_else(|_| "null".to_string());
        format!("if (window.opener) {{ window.opener.postMessage(payload, {origin}); }}")
    });
    Html(format!(
        r#"<!doctype html>
<html lang="en">
<head><meta charset="utf-8"><meta name="viewport" content="width=device-width"><title>Antigravity OAuth</title></head>
<body style="font:14px system-ui,sans-serif;margin:40px;color:#18181b">
  <strong id="title"></strong>
  <p id="message"></p>
  <script>
    const payload = {payload};
    document.getElementById("title").textContent = payload.success ? "Antigravity connected" : "Antigravity sign-in failed";
    document.getElementById("message").textContent = payload.success ? "This window can close now." : payload.error;
    {notify_opener}
    if (payload.success) setTimeout(() => window.close(), 800);
  </script>
</body>
</html>"#
    ))
}

pub async fn get_gemini_account_models(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<GeminiAccountModelsResponse>, AppError> {
    Ok(Json(
        gemini_account_models(&state.db, &state.gemini_http, &id).await?,
    ))
}

pub async fn get_gemini_account_quota(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<GeminiAccountQuotaResponse>, AppError> {
    Ok(Json(
        gemini_account_quota(&state.db, &state.gemini_http, &id).await?,
    ))
}

pub async fn get_codex_account_quota(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<CodexAccountQuotaResponse>, AppError> {
    Ok(Json(
        codex_account_quota(&state.db, &state.http, &state.codex_device_oauth, &id).await?,
    ))
}

pub async fn list_provider_presets() -> Json<ProviderPresetListResponse> {
    Json(ProviderPresetListResponse {
        data: provider_presets(),
    })
}

pub async fn create_provider_account(
    State(state): State<AppState>,
    Json(input): Json<CreateProviderAccountRequest>,
) -> Result<(StatusCode, Json<ProviderAccountResponse>), AppError> {
    if input.name.trim().is_empty()
        || input.base_url.trim().is_empty()
        || input.api_key.trim().is_empty()
    {
        return Err(AppError::BadRequest(
            "provider account name, base URL, and API key are required".into(),
        ));
    }
    let mut account = state.db.create_provider_account(input).await?;
    with_account_load(&state, &mut account);
    Ok((
        StatusCode::CREATED,
        Json(ProviderAccountResponse { data: account }),
    ))
}

pub async fn update_provider_account(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(input): Json<UpdateProviderAccountRequest>,
) -> Result<Json<ProviderAccountResponse>, AppError> {
    if input
        .base_url
        .as_deref()
        .is_some_and(|base_url| base_url.trim().is_empty())
    {
        return Err(AppError::BadRequest(
            "provider account base URL cannot be empty".into(),
        ));
    }
    let mut account = state
        .db
        .update_provider_account(&id, input)
        .await
        .map_err(map_not_found)?;
    state
        .account_concurrency
        .set_limit(&account.id, account.max_running_requests);
    with_account_load(&state, &mut account);
    Ok(Json(ProviderAccountResponse { data: account }))
}

fn with_account_load(state: &AppState, account: &mut ProviderAccount) {
    let load = state.account_concurrency.load(&account.id);
    account.running_requests = load.running;
    account.queued_requests = load.queued;
}

pub async fn delete_provider_account(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<StatusCode, AppError> {
    state.db.delete_provider_account(&id).await?;
    state.account_concurrency.forget(&id);
    Ok(StatusCode::NO_CONTENT)
}

pub async fn list_model_catalog(
    State(state): State<AppState>,
) -> Result<Json<ModelCatalogListResponse>, AppError> {
    Ok(Json(ModelCatalogListResponse {
        data: state.db.list_model_catalog().await?,
    }))
}

pub async fn list_routable_model_catalog(
    State(state): State<AppState>,
) -> Result<Json<RoutableModelCatalogListResponse>, AppError> {
    let data = state
        .db
        .list_routable_model_catalog_by_wire()
        .await?
        .into_iter()
        .map(|model| RoutableModelCatalogEntry {
            id: model.id,
            wire_api: model.wire_api,
        })
        .collect();
    Ok(Json(RoutableModelCatalogListResponse { data }))
}

pub async fn create_model_catalog_entry(
    State(state): State<AppState>,
    Json(input): Json<CreateModelCatalogEntryRequest>,
) -> Result<(StatusCode, Json<ModelCatalogEntryResponse>), AppError> {
    if input.id.trim().is_empty() {
        return Err(AppError::BadRequest("model id is required".into()));
    }
    let entry = state
        .db
        .create_model_catalog_entry(input)
        .await
        .map_err(map_write_error)?;
    Ok((
        StatusCode::CREATED,
        Json(ModelCatalogEntryResponse { data: entry }),
    ))
}

pub async fn update_model_catalog_entry(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(input): Json<UpdateModelCatalogEntryRequest>,
) -> Result<Json<ModelCatalogEntryResponse>, AppError> {
    let entry = state
        .db
        .update_model_catalog_entry(&id, input)
        .await
        .map_err(map_write_or_not_found)?;
    Ok(Json(ModelCatalogEntryResponse { data: entry }))
}

pub async fn list_provider_model_routes(
    State(state): State<AppState>,
) -> Result<Json<ProviderModelRouteListResponse>, AppError> {
    Ok(Json(ProviderModelRouteListResponse {
        data: state.db.list_provider_model_routes().await?,
    }))
}

pub async fn create_provider_model_route(
    State(state): State<AppState>,
    Json(input): Json<CreateProviderModelRouteRequest>,
) -> Result<(StatusCode, Json<ProviderModelRouteResponse>), AppError> {
    validate_provider_model_route_input(
        &input.public_model_id,
        &input.provider_account_id,
        &input.upstream_model_id,
    )?;
    validate_route_weight(input.weight)?;
    let route = state
        .db
        .create_provider_model_route(input)
        .await
        .map_err(map_write_error)?;
    Ok((
        StatusCode::CREATED,
        Json(ProviderModelRouteResponse { data: route }),
    ))
}

pub async fn update_provider_model_route(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(input): Json<UpdateProviderModelRouteRequest>,
) -> Result<Json<ProviderModelRouteResponse>, AppError> {
    if input
        .public_model_id
        .as_deref()
        .is_some_and(|value| value.trim().is_empty())
        || input
            .provider_account_id
            .as_deref()
            .is_some_and(|value| value.trim().is_empty())
        || input
            .upstream_model_id
            .as_deref()
            .is_some_and(|value| value.trim().is_empty())
    {
        return Err(AppError::BadRequest(
            "public model, provider account, and upstream model cannot be empty".into(),
        ));
    }
    if let Some(weight) = input.weight {
        validate_route_weight(weight)?;
    }
    let route = state
        .db
        .update_provider_model_route(&id, input)
        .await
        .map_err(map_write_or_not_found)?;
    Ok(Json(ProviderModelRouteResponse { data: route }))
}

fn validate_route_weight(weight: u32) -> Result<(), AppError> {
    if !(1..=10_000).contains(&weight) {
        return Err(AppError::BadRequest(
            "provider model route weight must be between 1 and 10000".into(),
        ));
    }
    Ok(())
}

fn relay_session_affinity(
    state: &AppState,
    wire_api: &str,
    headers: &HeaderMap,
    body: &Value,
) -> Option<SessionAffinity> {
    let affinity = extract_session_affinity(wire_api, headers, body);
    let client = affinity
        .as_ref()
        .map(|value| value.client_kind)
        .unwrap_or_else(|| client_kind(headers, wire_api));
    let status = match affinity.as_ref() {
        Some(value) if value.conflict => "conflict",
        Some(_) => "present",
        None => "missing",
    };
    state
        .relay_metrics
        .record_affinity(client, wire_api, status);
    affinity
}

pub async fn delete_provider_model_route(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<StatusCode, AppError> {
    if !state.db.delete_provider_model_route(&id).await? {
        return Err(AppError::NotFound("provider model route not found".into()));
    }
    Ok(StatusCode::NO_CONTENT)
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LogsQuery {
    limit: Option<u32>,
}

pub async fn list_request_logs(
    State(state): State<AppState>,
    Query(query): Query<LogsQuery>,
) -> Result<Json<RequestLogListResponse>, AppError> {
    Ok(Json(RequestLogListResponse {
        data: state
            .db
            .list_request_logs(query.limit.unwrap_or(50).min(200))
            .await?,
    }))
}

fn validate_messages_request(value: &Value) -> Result<(), AppError> {
    if !value.is_object() {
        return Err(AppError::BadRequest(
            "request body must be an object".into(),
        ));
    }
    match value.get("messages").and_then(Value::as_array) {
        Some(messages) if !messages.is_empty() => Ok(()),
        _ => Err(AppError::BadRequest(
            "missing or invalid messages array".into(),
        )),
    }
}

mod responses_websocket;
pub use responses_websocket::relay_responses_websocket;

fn validate_responses_request(value: &Value) -> Result<(), AppError> {
    if !value.is_object() {
        return Err(AppError::BadRequest(
            "request body must be an object".into(),
        ));
    }
    if value.get("input").is_some()
        || value.get("messages").is_some()
        || value.get("prompt").is_some()
    {
        Ok(())
    } else {
        Err(AppError::BadRequest(
            "missing Responses API input field".into(),
        ))
    }
}

fn validate_gemini_generate_content_request(value: &Value) -> Result<(), AppError> {
    if !value.is_object() {
        return Err(AppError::BadRequest(
            "request body must be an object".into(),
        ));
    }
    if value
        .get("contents")
        .is_some_and(|contents| !contents.is_null())
    {
        Ok(())
    } else {
        Err(AppError::BadRequest("missing Gemini contents field".into()))
    }
}

fn apply_protocol_headers<'a, R, C>(
    mut request: RequestBuilderSend<'a, R, C>,
    wire_api: WireApi,
    codex_context_headers: &HeaderMap,
    headers: &HeaderMap,
) -> RequestBuilderSend<'a, R, C>
where
    R: RuntimePoll,
    C: ConnectorSend,
{
    if wire_api == WireApi::OpenAiResponses {
        request = request.headers(codex_context_headers.clone());
        if let Some(lite) = headers.get(RESPONSES_LITE_HEADER) {
            request = request.header(HeaderName::from_static(RESPONSES_LITE_HEADER), lite.clone());
        }
    }
    if matches!(wire_api, WireApi::AnthropicMessages) {
        request = request.header(
            HeaderName::from_static("anthropic-version"),
            forwarded_anthropic_version(headers),
        );
        if let Some(beta) = headers.get("anthropic-beta") {
            request = request.header(HeaderName::from_static("anthropic-beta"), beta.clone());
        }
    }
    request
}

const RESPONSES_LITE_HEADER: &str = codex_context::RESPONSES_LITE_HEADER;

fn validate_responses_protocol(headers: &HeaderMap, value: &Value) -> Result<(), AppError> {
    let mut values = headers.get_all(RESPONSES_LITE_HEADER).iter();
    let lite = values.next();
    if values.next().is_some() || lite.is_some_and(|value| value != "true") {
        return Err(AppError::BadRequest(
            "Responses Lite header must have exactly one value: true".into(),
        ));
    }
    let has_tools_item = value
        .get("input")
        .and_then(Value::as_array)
        .is_some_and(|items| {
            items
                .iter()
                .any(|item| item.get("type").and_then(Value::as_str) == Some("additional_tools"))
        });
    if has_tools_item && lite.is_none() {
        return Err(AppError::BadRequest(
            "additional_tools input requires the Responses Lite header".into(),
        ));
    }
    Ok(())
}

fn apply_provider_auth<'a, R, C>(
    request: RequestBuilderSend<'a, R, C>,
    auth_mode: &str,
    api_key: &str,
) -> Result<RequestBuilderSend<'a, R, C>, AppError>
where
    R: RuntimePoll,
    C: ConnectorSend,
{
    if auth_mode == "bearer" {
        Ok(request.bearer_auth(api_key))
    } else if auth_mode == "x-goog-api-key" {
        let api_key = HeaderValue::from_str(api_key)
            .map_err(|error| AppError::Internal(format!("invalid provider API key: {error}")))?;
        Ok(request.header(HeaderName::from_static("x-goog-api-key"), api_key))
    } else if is_codex_subscription_auth(auth_mode) {
        Err(AppError::Internal(
            "Codex OAuth provider auth must be resolved before proxying".into(),
        ))
    } else {
        let api_key = HeaderValue::from_str(api_key)
            .map_err(|error| AppError::Internal(format!("invalid provider API key: {error}")))?;
        Ok(request.header(HeaderName::from_static("x-api-key"), api_key))
    }
}

fn apply_codex_subscription_auth<'a, R, C>(
    request: RequestBuilderSend<'a, R, C>,
    auth: &CodexSubscriptionAuthorization,
) -> Result<RequestBuilderSend<'a, R, C>, AppError>
where
    R: RuntimePoll,
    C: ConnectorSend,
{
    let mut request = request.bearer_auth(&auth.access_token).header(
        HeaderName::from_static("originator"),
        HeaderValue::from_static(auth.originator),
    );
    if let Some(account_id) = &auth.account_id {
        let account_id = HeaderValue::from_str(account_id).map_err(|error| {
            AppError::Internal(format!("invalid ChatGPT account id header: {error}"))
        })?;
        request = request.header(HeaderName::from_static("chatgpt-account-id"), account_id);
    }
    Ok(request)
}

fn upstream_url(base_url: &str, path: &str) -> String {
    let base_url = base_url.trim_end_matches('/');
    let path = path.trim_start_matches('/');
    if let Some((version, rest)) = path.split_once('/')
        && version.starts_with('v')
        && base_url.ends_with(&format!("/{version}"))
    {
        format!("{base_url}/{rest}")
    } else {
        format!("{base_url}/{path}")
    }
}

fn gemini_code_assist_upstream_url(
    endpoint: &str,
    method: GeminiMethod,
    query: Option<&str>,
) -> String {
    let endpoint = endpoint.trim().trim_end_matches('/');
    let endpoint = endpoint
        .strip_suffix("/v1internal")
        .unwrap_or(endpoint)
        .trim_end_matches('/');
    let url = format!("{endpoint}/v1internal:{}", method.as_str());
    match forwarded_code_assist_query(query, method) {
        Some(query) => format!("{url}?{query}"),
        None => url,
    }
}

fn gemini_public_path(model: &str, method: GeminiMethod) -> String {
    format!("/gemini/v1beta/models/{model}:{}", method.as_str())
}

fn parse_gemini_model_operation(operation: &str) -> Result<(String, GeminiMethod), AppError> {
    let (model, method) = operation.rsplit_once(':').ok_or_else(|| {
        AppError::BadRequest(
            "Gemini path must end with :generateContent or :streamGenerateContent".into(),
        )
    })?;
    if model.trim().is_empty() {
        return Err(AppError::BadRequest("Gemini model is required".into()));
    }
    let method = match method {
        "generateContent" => GeminiMethod::GenerateContent,
        "streamGenerateContent" => GeminiMethod::StreamGenerateContent,
        _ => {
            return Err(AppError::BadRequest(
                "unsupported Gemini generateContent method".into(),
            ));
        }
    };
    Ok((model.to_string(), method))
}

fn forwarded_code_assist_query(query: Option<&str>, method: GeminiMethod) -> Option<String> {
    let mut forwarded = Vec::new();
    if method.is_stream() {
        forwarded.push("alt=sse");
    }
    if let Some(query) = query {
        forwarded.extend(
            query
                .split('&')
                .filter(|part| !part.is_empty())
                .filter(|part| {
                    let key = part.split_once('=').map_or(*part, |(key, _)| key);
                    key != "key" && key != "alt"
                }),
        );
    }
    if forwarded.is_empty() {
        None
    } else {
        Some(forwarded.join("&"))
    }
}

fn take_sse_event(buffer: &str) -> Option<(String, String)> {
    if let Some(index) = buffer.find("\n\n") {
        let event = buffer[..index].to_string();
        let rest = buffer[index + 2..].to_string();
        return Some((event, rest));
    }
    if let Some(index) = buffer.find("\r\n\r\n") {
        let event = buffer[..index].to_string();
        let rest = buffer[index + 4..].to_string();
        return Some((event, rest));
    }
    None
}

#[derive(Default)]
struct CodexRetryAdviceRewriter {
    buffer: Vec<u8>,
    response_events_seen: usize,
}

impl CodexRetryAdviceRewriter {
    fn process(&mut self, chunk: &Bytes, codex_subscription: bool) -> (Bytes, bool) {
        self.buffer.extend_from_slice(chunk.as_ref());
        let mut output = String::new();
        let mut injected = false;
        while let Some((event_end, delimiter_len)) = find_sse_event(&self.buffer) {
            let event = String::from_utf8_lossy(&self.buffer[..event_end]).into_owned();
            self.buffer.drain(..event_end + delimiter_len);
            let response_started = self.response_events_seen > 0;
            let (event, event_injected, is_response_event) =
                rewrite_codex_retry_advice_event(&event, codex_subscription, response_started);
            injected |= event_injected;
            if is_response_event {
                self.response_events_seen += 1;
            }
            output.push_str(&event);
            output.push_str("\n\n");
        }
        (Bytes::from(output), injected)
    }

    fn finish(&mut self) -> Bytes {
        Bytes::from(std::mem::take(&mut self.buffer))
    }
}

fn find_sse_event(buffer: &[u8]) -> Option<(usize, usize)> {
    buffer.windows(2).enumerate().find_map(|(index, pair)| {
        if pair == b"\n\n" {
            Some((index, 2))
        } else if pair == b"\r\n" && buffer.get(index + 2..index + 4) == Some(b"\r\n") {
            Some((index, 4))
        } else {
            None
        }
    })
}

fn rewrite_codex_retry_advice_event(
    event: &str,
    codex_subscription: bool,
    response_started: bool,
) -> (String, bool, bool) {
    let mut lines = event.lines().map(str::to_owned).collect::<Vec<_>>();
    let Some(data_index) = lines.iter().position(|line| line.starts_with("data:")) else {
        return (event.to_owned(), false, false);
    };
    let data = lines[data_index]["data:".len()..].trim();
    let Ok(mut value) = serde_json::from_str::<Value>(data) else {
        return (event.to_owned(), false, false);
    };
    let is_response_event = value.get("type").and_then(Value::as_str).is_some();
    let overload = matches!(
        value.get("type").and_then(Value::as_str),
        Some("error" | "response.failed")
    ) && value
        .pointer("/error/code")
        .or_else(|| value.pointer("/response/error/code"))
        .and_then(Value::as_str)
        == Some("server_is_overloaded");
    if !(codex_subscription && response_started && overload) {
        return (event.to_owned(), false, is_response_event);
    }
    let error = if value.get("type").and_then(Value::as_str) == Some("response.failed") {
        value.pointer_mut("/response/error")
    } else {
        value.pointer_mut("/error")
    };
    if let Some(error) = error.and_then(Value::as_object_mut) {
        let headers = error
            .entry("headers")
            .or_insert_with(|| Value::Object(Default::default()));
        if let Some(headers) = headers.as_object_mut() {
            headers.insert("Retry-After".to_string(), Value::String("0".to_string()));
        }
    }
    lines[data_index] = format!("data: {value}");
    (lines.join("\n"), true, is_response_event)
}

fn transform_code_assist_sse_event(event: &str) -> String {
    let mut transformed = String::new();
    for line in event.lines() {
        if let Some(data) = line.strip_prefix("data:") {
            transformed.push_str("data: ");
            transformed.push_str(&unwrap_code_assist_sse_data(data.trim_start()));
            transformed.push('\n');
        } else {
            transformed.push_str(line);
            transformed.push('\n');
        }
    }
    transformed.push('\n');
    transformed
}

fn strip_top_level_params(value: &mut Value, strip_params: &[String]) -> Vec<String> {
    let Some(object) = value.as_object_mut() else {
        return Vec::new();
    };
    let mut stripped = Vec::new();
    for param in strip_params {
        if object.remove(param).is_some() {
            stripped.push(param.clone());
        }
    }
    stripped
}

fn strip_upstream_params(
    value: &mut Value,
    configured_strip_params: &[String],
    wire_api: WireApi,
    auth_mode: &str,
) -> Vec<String> {
    let mut stripped = strip_top_level_params(value, configured_strip_params);
    if wire_api != WireApi::OpenAiResponses || !is_codex_subscription_auth(auth_mode) {
        return stripped;
    }

    // Codex subscription endpoints follow the Codex CLI request shape, which
    // omits max_output_tokens even though the public Responses API accepts it.
    let Some(object) = value.as_object_mut() else {
        return stripped;
    };
    if object.remove("max_output_tokens").is_some() {
        stripped.push("max_output_tokens".to_string());
    }
    stripped
}

fn build_request_summary(
    value: &Value,
    body_bytes: u64,
    stripped_params: Vec<String>,
) -> RequestSummary {
    let mut top_level_keys = value
        .as_object()
        .map(|object| object.keys().cloned().collect::<Vec<_>>())
        .unwrap_or_default();
    top_level_keys.sort();
    RequestSummary {
        top_level_keys,
        body_bytes,
        stream: value
            .get("stream")
            .and_then(Value::as_bool)
            .unwrap_or(false),
        stripped_params,
    }
}

fn forwarded_anthropic_version(headers: &HeaderMap) -> HeaderValue {
    headers
        .get("anthropic-version")
        .filter(|value| {
            value
                .to_str()
                .is_ok_and(|version| !version.trim().is_empty())
        })
        .cloned()
        .unwrap_or_else(|| HeaderValue::from_static(DEFAULT_ANTHROPIC_VERSION))
}

fn enable_stream_usage_if_supported(request: &mut Value, wire_api: WireApi, provider: &str) {
    if wire_api != WireApi::OpenAiChat
        || provider != "openai"
        || request.get("stream").and_then(Value::as_bool) != Some(true)
    {
        return;
    }

    let Some(request) = request.as_object_mut() else {
        return;
    };
    let stream_options = request.entry("stream_options").or_insert_with(|| json!({}));
    if let Some(stream_options) = stream_options.as_object_mut() {
        stream_options
            .entry("include_usage")
            .or_insert(Value::Bool(true));
    }
}

enum StreamEnd {
    Complete,
    UpstreamError(io::Error),
    ClientDisconnected,
    IdleTimeout(Duration),
    MaximumDuration(Duration),
    ServerShuttingDown,
}

struct StreamingResponseContext {
    attempt: RelayAttempt,
    log: RelayAttemptLog,
    status: StatusCode,
    response_headers: HeaderMap,
    wire_api: WireApi,
    codex_subscription: bool,
    preflight_timed_out: bool,
    first_output_at: Option<Instant>,
    idle_timeout: Duration,
    max_duration: Duration,
    shutdown: crate::server::ShutdownSignal,
}

fn record_streaming_response<S>(
    body_stream: S,
    context: StreamingResponseContext,
) -> impl Stream<Item = Result<Bytes, io::Error>> + Send + 'static
where
    S: Stream<Item = Result<Bytes, io::Error>> + Send + 'static,
{
    const BUFFERED_CHUNKS: usize = 1;

    let StreamingResponseContext {
        attempt,
        mut log,
        status,
        response_headers,
        wire_api,
        codex_subscription,
        preflight_timed_out,
        first_output_at,
        idle_timeout,
        max_duration,
        shutdown,
    } = context;

    // The owned task can finalize the attempt after upstream EOF while the
    // bounded channel preserves downstream backpressure.
    let (sender, receiver) = mpsc::channel(BUFFERED_CHUNKS);
    tokio::spawn(async move {
        let mut body_stream = Box::pin(body_stream);
        let mut observation = StreamingResponseObservation {
            first_output_at,
            ..StreamingResponseObservation::default()
        };
        let mut retry_advice_rewriter = CodexRetryAdviceRewriter::default();
        let deadline = Instant::now() + max_duration;
        let mut shutdown_rx = shutdown.subscribe();
        let stream_end = if *shutdown_rx.borrow() {
            StreamEnd::ServerShuttingDown
        } else {
            loop {
                let next = tokio::select! {
                    _ = sender.closed() => break StreamEnd::ClientDisconnected,
                    changed = shutdown_rx.changed() => match changed {
                        Ok(()) if *shutdown_rx.borrow_and_update() => break StreamEnd::ServerShuttingDown,
                        Ok(()) => continue,
                        Err(_) => break StreamEnd::ServerShuttingDown,
                    },
                    _ = time::sleep_until(deadline) => break StreamEnd::MaximumDuration(max_duration),
                    next = time::timeout(idle_timeout, body_stream.next()) => match next {
                        Ok(next) => next,
                        Err(_) => break StreamEnd::IdleTimeout(idle_timeout),
                    },
                };
                match next {
                    Some(Ok(chunk)) => {
                        observation.observe(&chunk, Instant::now());
                        let (chunk, injected) =
                            retry_advice_rewriter.process(&chunk, codex_subscription);
                        if injected {
                            log.client_retry_advice_injected = true;
                        }
                        if !chunk.is_empty() && sender.send(Ok(chunk)).await.is_err() {
                            break StreamEnd::ClientDisconnected;
                        }
                    }
                    Some(Err(error)) => break StreamEnd::UpstreamError(error),
                    None => {
                        let chunk = retry_advice_rewriter.finish();
                        if !chunk.is_empty() && sender.send(Ok(chunk)).await.is_err() {
                            break StreamEnd::ClientDisconnected;
                        }
                        break StreamEnd::Complete;
                    }
                }
            }
        };

        log.output_started = observation.output_started;
        if let Some(failure) = observation.application_failure(wire_api) {
            if codex_subscription && failure.retryable && log.overload_retry_skip_reason.is_none() {
                log.overload_retry_skip_reason = Some(
                    if observation.output_started {
                        "response_already_started"
                    } else if preflight_timed_out {
                        "preflight_timeout"
                    } else {
                        "overload_not_replayable"
                    }
                    .to_string(),
                );
            }
            let usage = observation.tokens();
            tracing::warn!(
                codex_subscription,
                event_type = failure.event_type,
                error_code = failure.code.as_deref().unwrap_or("<none>"),
                response_events = observation.message_count,
                output_tokens = usage.output_tokens,
                "upstream application stream failure"
            );
            let error = failure.log_error();
            let route_failure = classify_upstream_application_failure(
                failure.code.as_deref(),
                error.clone(),
                Utc::now(),
            );
            let result = match route_failure {
                Some(route_failure) => {
                    attempt
                        .record_failure_with_usage(&log, route_failure, usage)
                        .await
                }
                None => {
                    attempt
                        .record_application_failure(&log, StatusCode::BAD_GATEWAY, usage, error)
                        .await
                }
            };
            if let Err(record_error) = result {
                tracing::error!(
                    error = %record_error,
                    path = %log.path,
                    "failed to record upstream application stream failure"
                );
                let _ = sender.send(Err(io::Error::other(record_error))).await;
            }
            return;
        }

        match stream_end {
            StreamEnd::UpstreamError(error) => {
                let failure = classify_transport_failure(error.to_string(), Utc::now());
                if let Err(record_error) = attempt
                    .record_failure_with_usage(&log, failure, observation.tokens())
                    .await
                {
                    tracing::error!(
                        error = %record_error,
                        path = %log.path,
                        "failed to record upstream stream failure"
                    );
                }
                let _ = sender.send(Err(error)).await;
                return;
            }
            StreamEnd::ClientDisconnected => {
                if let Err(error) = attempt
                    .record_client_disconnect(&log, observation.tokens())
                    .await
                {
                    tracing::error!(
                        %error,
                        path = %log.path,
                        "failed to record client-disconnected stream"
                    );
                }
                return;
            }
            StreamEnd::IdleTimeout(duration) => {
                let error = stream_timeout_error("was idle", duration);
                let failure = classify_transport_failure(error.to_string(), Utc::now());
                if let Err(record_error) = attempt
                    .record_failure_with_usage(&log, failure, observation.tokens())
                    .await
                {
                    tracing::error!(
                        error = %record_error,
                        path = %log.path,
                        "failed to record upstream stream idle timeout"
                    );
                }
                let _ = sender.send(Err(error)).await;
                return;
            }
            StreamEnd::MaximumDuration(duration) => {
                let error = stream_timeout_error("exceeded its maximum lifetime of", duration);
                let failure = classify_transport_failure(error.to_string(), Utc::now());
                if let Err(record_error) = attempt
                    .record_failure_with_usage(&log, failure, observation.tokens())
                    .await
                {
                    tracing::error!(
                        error = %record_error,
                        path = %log.path,
                        "failed to record upstream stream maximum duration"
                    );
                }
                let _ = sender.send(Err(error)).await;
                return;
            }
            StreamEnd::ServerShuttingDown => {
                if let Err(error) = attempt
                    .record_server_shutdown(&log, observation.tokens())
                    .await
                {
                    tracing::error!(
                        %error,
                        path = %log.path,
                        "failed to record stream interrupted by server shutdown"
                    );
                }
                return;
            }
            StreamEnd::Complete => {}
        }

        let usage = observation.tokens();
        if let Err(error) = attempt
            .record_response(&log, status, &response_headers, b"", usage)
            .await
        {
            tracing::error!(%error, path = %log.path, "failed to record streaming response");
            let _ = sender.send(Err(io::Error::other(error))).await;
        }
    });

    stream::unfold(receiver, |mut receiver| async move {
        receiver.recv().await.map(|item| (item, receiver))
    })
}

fn stream_timeout_error(reason: &str, duration: Duration) -> io::Error {
    io::Error::new(
        io::ErrorKind::TimedOut,
        format!("upstream stream {reason} {} seconds", duration.as_secs()),
    )
}

fn is_output_delta(value: &Value) -> bool {
    let event_type = value
        .get("type")
        .and_then(Value::as_str)
        .unwrap_or_default();
    if event_type.ends_with(".delta") {
        return value
            .get("delta")
            .is_some_and(|delta| !delta.is_null() && delta != "");
    }
    if event_type == "content_block_delta" {
        return value
            .pointer("/delta/text")
            .or_else(|| value.pointer("/delta/thinking"))
            .or_else(|| value.pointer("/delta/partial_json"))
            .is_some_and(|delta| delta.as_str().is_some_and(|text| !text.is_empty()));
    }
    if let Some(delta) = value.pointer("/choices/0/delta") {
        return delta.as_object().is_some_and(|delta| {
            delta.iter().any(|(key, value)| {
                key != "role"
                    && !value.is_null()
                    && match value {
                        Value::String(text) => !text.is_empty(),
                        Value::Array(items) => !items.is_empty(),
                        Value::Object(fields) => !fields.is_empty(),
                        _ => true,
                    }
            })
        });
    }
    value
        .pointer("/candidates/0/content/parts")
        .and_then(Value::as_array)
        .is_some_and(|parts| !parts.is_empty())
}

fn is_output_started(value: &Value) -> bool {
    if is_output_delta(value) {
        return true;
    }
    match value
        .get("type")
        .and_then(Value::as_str)
        .unwrap_or_default()
    {
        "response.reasoning_summary_part.added"
        | "response.reasoning_summary_text.done"
        | "response.reasoning_text.done"
        | "response.reasoning.done"
        | "response.output_item.added"
        | "content_block_start"
        | "tool_call" => true,
        "response.output_item.done" => value
            .pointer("/item/type")
            .and_then(Value::as_str)
            .is_some_and(|kind| matches!(kind, "function_call" | "custom_tool_call" | "reasoning")),
        _ => false,
    }
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
struct UsageUpdate {
    input_tokens: Option<u64>,
    cached_input_tokens: Option<u64>,
    cache_read_input_tokens: Option<u64>,
    cache_creation_input_tokens: Option<u64>,
    output_tokens: Option<u64>,
}

impl UsageUpdate {
    fn merge(&mut self, update: Self) {
        if update.input_tokens.is_some() {
            self.input_tokens = update.input_tokens;
        }
        if update.cached_input_tokens.is_some() {
            self.cached_input_tokens = update.cached_input_tokens;
        }
        if update.cache_read_input_tokens.is_some() {
            self.cache_read_input_tokens = update.cache_read_input_tokens;
        }
        if update.cache_creation_input_tokens.is_some() {
            self.cache_creation_input_tokens = update.cache_creation_input_tokens;
        }
        if update.output_tokens.is_some() {
            self.output_tokens = update.output_tokens;
        }
    }

    fn tokens(self) -> TokenUsage {
        self.tokens_with_generation(None)
    }

    fn tokens_with_generation(self, first_output_at: Option<Instant>) -> TokenUsage {
        let mut input_tokens = self.input_tokens.unwrap_or(0);
        let cached_input_tokens = self
            .cache_read_input_tokens
            .or(self.cached_input_tokens)
            .unwrap_or(0);

        // Anthropic reports uncached input, cache reads, and cache writes as
        // separate values. Other supported APIs include cache hits in their
        // input-token total already.
        if self.cache_read_input_tokens.is_some() || self.cache_creation_input_tokens.is_some() {
            input_tokens = input_tokens
                .saturating_add(cached_input_tokens)
                .saturating_add(self.cache_creation_input_tokens.unwrap_or(0));
        }

        TokenUsage {
            input_tokens,
            cached_input_tokens,
            output_tokens: self.output_tokens.unwrap_or(0),
            generation_ms: first_output_at
                .map(|started| Instant::now().duration_since(started).as_millis() as u64),
        }
    }
}

#[derive(Debug)]
struct StreamingResponseObservation {
    decoder: SseDecoder,
    buffer: BytesMut,
    usage: UsageUpdate,
    output_started: bool,
    first_output_at: Option<Instant>,
    message_count: usize,
    application_failure: Option<StreamingApplicationFailure>,
    replayable_application_failure: Option<StreamingApplicationFailure>,
}

impl Default for StreamingResponseObservation {
    fn default() -> Self {
        Self {
            // Responses completion events can contain the full generated
            // response, so the default 512 KiB SSE limit is too small.
            decoder: SseDecoder::with_max_payload_size(0),
            buffer: BytesMut::new(),
            usage: UsageUpdate::default(),
            output_started: false,
            first_output_at: None,
            message_count: 0,
            application_failure: None,
            replayable_application_failure: None,
        }
    }
}

impl StreamingResponseObservation {
    fn observe(&mut self, chunk: &[u8], observed_at: Instant) {
        self.buffer.extend_from_slice(chunk);
        while let Some(event) = self.decoder.decode(&mut self.buffer) {
            let Ok(SseEvent::Message(message)) = event else {
                continue;
            };
            self.message_count += 1;
            let Ok(value) = serde_json::from_str::<Value>(&message.data) else {
                continue;
            };
            self.output_started |= is_output_started(&value);
            if self.first_output_at.is_none() && is_output_delta(&value) {
                self.first_output_at = Some(observed_at);
            }
            self.usage.merge(usage_update(&value));
            if let Some(error) = openai_responses_stream_failure(&value) {
                if !self.output_started {
                    self.replayable_application_failure = Some(error.clone());
                }
                self.application_failure = Some(error);
            }
        }
    }

    fn tokens(&self) -> TokenUsage {
        self.usage.tokens_with_generation(self.first_output_at)
    }

    fn application_failure(&self, wire_api: WireApi) -> Option<&StreamingApplicationFailure> {
        if wire_api == WireApi::OpenAiResponses {
            self.application_failure.as_ref()
        } else {
            None
        }
    }

    fn replayable_application_failure(
        &self,
        wire_api: WireApi,
    ) -> Option<&StreamingApplicationFailure> {
        if wire_api == WireApi::OpenAiResponses {
            self.replayable_application_failure.as_ref()
        } else {
            None
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct StreamingApplicationFailure {
    event_type: &'static str,
    code: Option<String>,
    retryable: bool,
    retry_after: Option<Duration>,
}

impl StreamingApplicationFailure {
    fn log_error(&self) -> String {
        self.code.as_ref().map_or_else(
            || "upstream stream failed".to_string(),
            |code| format!("upstream stream failed ({code})"),
        )
    }
}

fn openai_responses_stream_failure(value: &Value) -> Option<StreamingApplicationFailure> {
    let (event_type, error) = match value.get("type").and_then(Value::as_str)? {
        "error" => ("error", value.get("error").unwrap_or(value)),
        "response.failed" => (
            "response.failed",
            value.pointer("/response/error").unwrap_or(value),
        ),
        _ => return None,
    };

    let code = normalized_stream_error_code(error);
    let disabled = error
        .get("message")
        .and_then(Value::as_str)
        .is_some_and(|message| message.to_ascii_lowercase().contains("disabled"));
    Some(StreamingApplicationFailure {
        event_type,
        retryable: relay_retry::retryable_provider_code(code.as_deref()) && !disabled,
        code,
        retry_after: retry_after_from_event(value),
    })
}

fn retry_after_from_event(value: &Value) -> Option<Duration> {
    let error = match value.get("type").and_then(Value::as_str) {
        Some("error") => value.get("error").unwrap_or(value),
        Some("response.failed") => value.pointer("/response/error").unwrap_or(value),
        _ => value,
    };
    let mut headers = HeaderMap::new();
    if let Some(value) = error
        .pointer("/headers/Retry-After")
        .or_else(|| error.get("retry_after"))
        .and_then(Value::as_str)
        && let Ok(value) = HeaderValue::from_str(value)
    {
        headers.insert("retry-after", value);
    }
    if let Some(value) = error
        .pointer("/headers/Retry-After-Ms")
        .or_else(|| error.get("retry_after_ms"))
        .and_then(Value::as_u64)
        && let Ok(value) = HeaderValue::from_str(&value.to_string())
    {
        headers.insert("retry-after-ms", value);
    }
    relay_retry::retry_after(&headers, Utc::now())
}

fn normalized_stream_error_code(error: &Value) -> Option<String> {
    error
        .get("code")
        .and_then(Value::as_str)
        .and_then(relay_retry::normalize_provider_code)
}

fn parse_usage(bytes: &[u8]) -> TokenUsage {
    let Ok(value) = serde_json::from_slice::<Value>(bytes) else {
        return TokenUsage::default();
    };
    usage_update(&value).tokens()
}

fn usage_update(value: &Value) -> UsageUpdate {
    let mut update = UsageUpdate::default();
    for usage in [
        value.get("usage"),
        value.get("usageMetadata"),
        value.pointer("/response/usage"),
        value.pointer("/response/usageMetadata"),
        value.pointer("/message/usage"),
        value.pointer("/message/usageMetadata"),
    ]
    .into_iter()
    .flatten()
    {
        update.merge(UsageUpdate {
            input_tokens: token_field(
                usage,
                &["input_tokens", "prompt_tokens", "promptTokenCount"],
            ),
            cached_input_tokens: token_field(usage, &["cachedContentTokenCount"])
                .or_else(|| nested_token_field(usage, "input_tokens_details", "cached_tokens"))
                .or_else(|| nested_token_field(usage, "prompt_tokens_details", "cached_tokens")),
            cache_read_input_tokens: token_field(usage, &["cache_read_input_tokens"]),
            cache_creation_input_tokens: token_field(usage, &["cache_creation_input_tokens"]),
            output_tokens: token_field(
                usage,
                &["output_tokens", "completion_tokens", "candidatesTokenCount"],
            ),
        });
    }
    update
}

fn nested_token_field(value: &Value, object: &str, field: &str) -> Option<u64> {
    value.get(object)?.get(field)?.as_u64()
}

fn token_field(value: &Value, names: &[&str]) -> Option<u64> {
    names
        .iter()
        .find_map(|name| value.get(*name).and_then(Value::as_u64))
}

async fn openai_models(state: &AppState) -> Result<Vec<OpenAiModel>, AppError> {
    let models = state
        .db
        .list_routable_model_catalog(&["openai-chat", "openai-responses"])
        .await?;
    Ok(models
        .into_iter()
        .map(|model| OpenAiModel {
            id: model.id,
            object: "model".to_string(),
            created: model.created_at.timestamp(),
            owned_by: model.provider,
        })
        .collect())
}

async fn gemini_models(state: &AppState) -> Result<Vec<GeminiModel>, AppError> {
    let models = state
        .db
        .list_routable_model_catalog(&[WireApi::GeminiGenerateContent.account_value()])
        .await?;
    Ok(models
        .into_iter()
        .map(|model| GeminiModel {
            name: format!("models/{}", model.id),
            display_name: model.display_name,
            supported_generation_methods: vec![
                "generateContent".to_string(),
                "streamGenerateContent".to_string(),
            ],
        })
        .collect())
}

async fn anthropic_models(state: &AppState) -> Result<Vec<AnthropicModel>, AppError> {
    let models = state
        .db
        .list_routable_model_catalog(&["anthropic-messages"])
        .await?;
    Ok(models
        .into_iter()
        .map(|model| AnthropicModel {
            r#type: "model".to_string(),
            display_name: model.display_name,
            id: model.id,
            created_at: model.created_at,
        })
        .collect())
}

fn map_not_found(error: rusqlite::Error) -> AppError {
    match error {
        rusqlite::Error::QueryReturnedNoRows => AppError::NotFound("record not found".into()),
        error => AppError::Database(error),
    }
}

fn map_write_or_not_found(error: rusqlite::Error) -> AppError {
    match error {
        rusqlite::Error::QueryReturnedNoRows => AppError::NotFound("record not found".into()),
        _ => map_write_error(error),
    }
}

fn map_write_error(error: rusqlite::Error) -> AppError {
    let message = error.to_string();
    if message.contains("UNIQUE constraint failed")
        || message.contains("FOREIGN KEY constraint failed")
        || message.contains("NOT NULL constraint failed")
    {
        AppError::BadRequest(
            "invalid model catalog or route data; check duplicate primary routes and references"
                .into(),
        )
    } else {
        AppError::Database(error)
    }
}

fn validate_provider_model_route_input(
    public_model_id: &str,
    provider_account_id: &str,
    upstream_model_id: &str,
) -> Result<(), AppError> {
    if public_model_id.trim().is_empty()
        || provider_account_id.trim().is_empty()
        || upstream_model_id.trim().is_empty()
    {
        return Err(AppError::BadRequest(
            "public model, provider account, and upstream model are required".into(),
        ));
    }
    Ok(())
}

async fn wait_for_codex_transient_retry(
    state: &AppState,
    retry: u8,
    error_code: Option<&str>,
    retry_after: Option<Duration>,
    deadline: Instant,
) -> Result<bool, AppError> {
    let delay = relay_retry::delay(retry, retry_after, &mut rand::rng());
    let remaining = deadline.saturating_duration_since(Instant::now());
    if remaining < delay {
        return Ok(false);
    }
    tracing::info!(
        retry,
        delay_seconds = delay.as_secs(),
        error_code = error_code.unwrap_or("<none>"),
        "retrying Codex subscription transient stream failure"
    );
    let mut shutdown = state.shutdown.subscribe();
    if *shutdown.borrow() {
        return Err(crate::relay_attempt::shutting_down_error());
    }
    tokio::select! {
        _ = tokio::time::sleep(delay) => Ok(true),
        _ = shutdown.changed() => Err(crate::relay_attempt::shutting_down_error()),
    }
}

#[cfg(test)]
mod tests {
    mod account_concurrency;
    mod responses_contract;
    mod responses_websocket;

    use std::{convert::Infallible, net::SocketAddr, path::PathBuf, sync::Arc, time::Duration};

    use super::*;
    use aioduct::TokioClient;
    use axum::{
        Json, Router,
        body::{Bytes, to_bytes},
        extract::State,
        http::{HeaderMap, HeaderValue, StatusCode, Uri, header},
        routing::post,
    };
    use chrono::Utc;
    use serde_json::{Value, json};
    use tokio::{net::TcpListener, sync::Mutex};
    use uuid::Uuid;

    use crate::{
        antigravity_oauth::AntigravityOAuthStore,
        config::{Config, HttpsMode, LETS_ENCRYPT_PRODUCTION_DIRECTORY},
        db::Db,
        models::{
            CreateApiKeyRequest, CreateModelCatalogEntryRequest, CreateProviderAccountRequest,
            CreateProviderModelRouteRequest,
        },
    };

    fn test_database_path() -> PathBuf {
        std::env::temp_dir().join(format!("token-toxication-relay-{}.sqlite3", Uuid::new_v4()))
    }

    fn remove_test_database(path: &PathBuf) {
        let _ = std::fs::remove_file(path);
        let _ = std::fs::remove_file(path.with_extension("sqlite3-shm"));
        let _ = std::fs::remove_file(path.with_extension("sqlite3-wal"));
    }

    fn test_http_client() -> TokioClient {
        TokioClient::builder()
            .tls(aioduct::tls::RustlsConnector::with_webpki_roots())
            .user_agent("token-toxication-test")
            .timeout(Duration::from_secs(5))
            .read_timeout(Duration::from_secs(5))
            .build()
            .expect("build test HTTP client")
    }

    fn test_state(db: Db, database_path: PathBuf) -> AppState {
        AppState {
            config: Arc::new(Config {
                bind_addr: "127.0.0.1:3000".parse().expect("parse bind address"),
                https_mode: HttpsMode::Off,
                tls_cert_path: None,
                tls_key_path: None,
                acme_identifier: None,
                acme_email: None,
                acme_http_bind_addr: "127.0.0.1:3001".parse().expect("parse ACME bind address"),
                acme_allow_nonstandard_http_port: false,
                acme_cert_dir: PathBuf::from("data/acme"),
                acme_directory_url: LETS_ENCRYPT_PRODUCTION_DIRECTORY.to_string(),
                database_path,
                static_dir: PathBuf::from("apps/admin/dist"),
                relay_stream_idle_timeout_secs: 60,
                relay_stream_max_duration_secs: 900,
                admin_username: "admin".to_string(),
                admin_password: "test-password".to_string(),
                api_key_prefix: "tokentoxication-".to_string(),
                allow_default_admin_password: false,
            }),
            db,
            http: test_http_client(),
            gemini_http: test_http_client(),
            websocket_http: crate::websocket_transport::build_client(Duration::from_secs(3))
                .unwrap(),
            antigravity_oauth: AntigravityOAuthStore::default(),
            codex_device_oauth: crate::codex_device_oauth::CodexDeviceOAuthStore::default(),
            codex_catalog_cache: Default::default(),
            relay_metrics: Default::default(),
            account_concurrency: Default::default(),
            relay_stream_idle_timeout: Duration::from_secs(5),
            relay_stream_max_duration: Duration::from_secs(900),
            shutdown: crate::server::ShutdownSignal::for_test(),
            started_at: Utc::now(),
        }
    }

    struct RelayRouteSeed<'a> {
        base_url: String,
        provider: &'a str,
        auth_mode: &'a str,
        provider_secret: String,
        wire_api: &'a str,
        public_model: &'a str,
        upstream_model: &'a str,
    }

    struct SeededRelayRoute {
        relay_secret: String,
        account_id: String,
        route_id: String,
    }

    fn gemini_relay_route_seed(address: SocketAddr) -> RelayRouteSeed<'static> {
        let base_url = format!("http://{address}");
        let credential = json!({
            "type": "token-toxication-antigravity-oauth-v1",
            "refresh": "refresh-token",
            "access": "access-token",
            "expires": Utc::now().timestamp_millis() + 3_600_000,
            "project": "test-project",
            "endpoint": base_url,
        })
        .to_string();

        RelayRouteSeed {
            base_url: format!("http://{address}"),
            provider: "gemini",
            auth_mode: "antigravity-oauth",
            provider_secret: credential,
            wire_api: "gemini-generate-content",
            public_model: "public-gemini-model",
            upstream_model: "upstream-gemini-model",
        }
    }

    async fn seed_relay_route(db: &Db, seed: RelayRouteSeed<'_>) -> SeededRelayRoute {
        let RelayRouteSeed {
            base_url,
            provider,
            auth_mode,
            provider_secret,
            wire_api,
            public_model,
            upstream_model,
        } = seed;
        let relay_secret = "tokentoxication-relay-test-key".to_string();
        db.create_api_key(
            CreateApiKeyRequest {
                name: "relay test key".to_string(),
                description: String::new(),
                permissions: Vec::new(),
                rate_limit_per_minute: 0,
                concurrency_limit: 0,
                daily_cost_limit: 0.0,
                expires_at: None,
            },
            &relay_secret,
        )
        .await
        .expect("create relay key");
        let account = db
            .create_provider_account(CreateProviderAccountRequest {
                name: "test provider".to_string(),
                provider: provider.to_string(),
                base_url,
                auth_mode: auth_mode.to_string(),
                wire_api: wire_api.to_string(),
                api_key: provider_secret,
                is_active: true,
                max_running_requests: 0,
            })
            .await
            .expect("create provider account");
        db.create_model_catalog_entry(CreateModelCatalogEntryRequest {
            id: public_model.to_string(),
            display_name: String::new(),
            family: "test".to_string(),
            enabled: true,
        })
        .await
        .expect("create model catalog entry");
        let route = db
            .create_provider_model_route(CreateProviderModelRouteRequest {
                public_model_id: public_model.to_string(),
                provider_account_id: account.id.clone(),
                upstream_model_id: upstream_model.to_string(),
                wire_api: wire_api.to_string(),
                role: "primary".to_string(),
                enabled: true,
                weight: 100,
                strip_params: Vec::new(),
            })
            .await
            .expect("create provider route");
        SeededRelayRoute {
            relay_secret,
            account_id: account.id,
            route_id: route.id,
        }
    }

    fn relay_headers(secret: &str) -> HeaderMap {
        let mut headers = HeaderMap::new();
        headers.insert(
            header::AUTHORIZATION,
            HeaderValue::from_str(&format!("Bearer {secret}")).expect("relay key header"),
        );
        headers
    }

    #[tokio::test]
    async fn unauthenticated_requests_are_rejected_before_wire_validation() {
        let database_path = test_database_path();
        let db = Db::open(&database_path).await.expect("open test database");
        let state = test_state(db, database_path.clone());

        let error = relay_openai_chat(
            State(state.clone()),
            HeaderMap::new(),
            Uri::from_static("/openai/v1/chat/completions"),
            Bytes::from_static(b"not valid JSON"),
        )
        .await
        .expect_err("missing relay credentials must win over an invalid request body");

        assert_eq!(error.status(), StatusCode::UNAUTHORIZED);
        assert_eq!(error.to_string(), "missing API key");

        drop(state);
        remove_test_database(&database_path);
    }

    #[tokio::test]
    async fn provider_route_api_defaults_and_validates_weight_boundaries() {
        let database_path = test_database_path();
        let db = Db::open(&database_path).await.expect("open test database");
        let account = db
            .create_provider_account(CreateProviderAccountRequest {
                name: "weighted provider".to_string(),
                provider: "openai".to_string(),
                base_url: "https://example.com".to_string(),
                auth_mode: "bearer".to_string(),
                wire_api: "openai-responses".to_string(),
                api_key: "synthetic-provider-key".to_string(),
                is_active: true,
                max_running_requests: 0,
            })
            .await
            .expect("create provider account");
        db.create_model_catalog_entry(CreateModelCatalogEntryRequest {
            id: "weighted-model".to_string(),
            display_name: String::new(),
            family: "openai".to_string(),
            enabled: true,
        })
        .await
        .expect("create model");
        let state = test_state(db, database_path.clone());

        let defaulted: CreateProviderModelRouteRequest = serde_json::from_value(json!({
            "publicModelId": "weighted-model",
            "providerAccountId": account.id,
            "upstreamModelId": "weighted-upstream",
            "wireApi": "openai-responses",
            "role": "primary",
            "enabled": true
        }))
        .expect("deserialize default weight");
        assert_eq!(defaulted.weight, 100);

        let (_, Json(created)) = create_provider_model_route(State(state.clone()), Json(defaulted))
            .await
            .expect("create route with default weight");
        assert_eq!(created.data.weight, 100);

        for weight in [0, 10_001] {
            let error = create_provider_model_route(
                State(state.clone()),
                Json(CreateProviderModelRouteRequest {
                    public_model_id: "weighted-model".to_string(),
                    provider_account_id: account.id.clone(),
                    upstream_model_id: format!("invalid-{weight}"),
                    wire_api: "openai-responses".to_string(),
                    role: "primary".to_string(),
                    enabled: true,
                    weight,
                    strip_params: Vec::new(),
                }),
            )
            .await
            .expect_err("out-of-range route weight must fail");
            assert_eq!(error.status(), StatusCode::BAD_REQUEST);
        }

        for weight in [1, 10_000] {
            let (_, Json(created)) = create_provider_model_route(
                State(state.clone()),
                Json(CreateProviderModelRouteRequest {
                    public_model_id: "weighted-model".to_string(),
                    provider_account_id: account.id.clone(),
                    upstream_model_id: format!("valid-{weight}"),
                    wire_api: "openai-responses".to_string(),
                    role: "primary".to_string(),
                    enabled: true,
                    weight,
                    strip_params: Vec::new(),
                }),
            )
            .await
            .expect("boundary route weight must succeed");
            assert_eq!(created.data.weight, weight);
        }

        drop(state);
        remove_test_database(&database_path);
    }

    #[tokio::test]
    async fn responses_relay_accepts_a_32_mebibyte_request_body() {
        let database_path = test_database_path();
        let db = Db::open(&database_path).await.expect("open test database");
        let state = test_state(db, database_path.clone());
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
        let address = listener.local_addr().expect("address");
        let server = tokio::spawn(async move {
            axum::serve(
                listener,
                crate::app(state, PathBuf::from("missing-admin-assets")),
            )
            .await
            .expect("serve");
        });
        let endpoint = format!("http://{address}/openai/v1/responses");

        let response = test_http_client()
            .post(&endpoint)
            .expect("build request")
            .body(vec![b'x'; crate::RELAY_BODY_LIMIT_BYTES])
            .send()
            .await
            .expect("send request");

        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);

        server.abort();
        let _ = server.await;
        remove_test_database(&database_path);
    }

    #[tokio::test]
    async fn routable_model_catalog_requires_admin_authentication() {
        let database_path = test_database_path();
        let db = Db::open(&database_path).await.expect("open test database");
        let state = test_state(db, database_path.clone());
        state
            .db
            .create_admin_session(
                "test-admin-token",
                "admin",
                Utc::now() + chrono::Duration::hours(1),
            )
            .await
            .expect("create admin session");

        let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
        let address = listener.local_addr().expect("address");
        let server = tokio::spawn(async move {
            axum::serve(
                listener,
                crate::app(state, PathBuf::from("missing-admin-assets")),
            )
            .await
            .expect("serve");
        });
        let endpoint = format!("http://{address}/admin/api/routable-model-catalog");
        let http = test_http_client();

        let unauthenticated = http
            .get(&endpoint)
            .expect("build unauthenticated request")
            .send()
            .await
            .expect("send unauthenticated request");
        assert_eq!(unauthenticated.status(), StatusCode::UNAUTHORIZED);

        let authenticated = http
            .get(&endpoint)
            .expect("build authenticated request")
            .bearer_auth("test-admin-token")
            .send()
            .await
            .expect("send authenticated request");
        assert_eq!(authenticated.status(), StatusCode::OK);

        server.abort();
        let _ = server.await;
        remove_test_database(&database_path);
    }

    fn codex_catalog_body() -> Value {
        let instructions = "x".repeat(64);
        json!({
            "models": [
                {
                    "slug": "gpt-6.1-sol",
                    "display_name": "GPT-6.1-Sol",
                    "visibility": "list",
                    "supported_in_api": true,
                    "priority": 2,
                    "context_window": 500_000,
                    "max_context_window": 872_000,
                    "model_messages": {"instructions_template": instructions},
                },
                {
                    "slug": "gpt-6-astra",
                    "display_name": "GPT-6-Astra",
                    "visibility": "list",
                    "supported_in_api": true,
                    "priority": 2,
                    "context_window": 272_000,
                    "max_context_window": 872_000,
                    "model_messages": {"instructions_template": instructions},
                },
                {
                    // Beyond TT's relay routes, but the client's picker needs it.
                    "slug": "gpt-5.6-sol",
                    "display_name": "GPT-5.6-Sol",
                    "visibility": "list",
                    "supported_in_api": true,
                    "priority": 3,
                    "context_window": 272_000,
                    "model_messages": {"instructions_template": instructions},
                },
                {
                    // No instructions: the client rejects a whole catalog for this,
                    // so TT must drop the entry rather than fail the request.
                    "slug": "gpt-reserve",
                    "display_name": "GPT-Reserve",
                    "visibility": "hide",
                    "supported_in_api": true,
                    "priority": 9,
                    "context_window": 272_000,
                },
            ]
        })
    }

    #[tokio::test]
    async fn codex_catalog_endpoint_serves_the_account_catalog_verbatim() {
        let captured: Arc<Mutex<Vec<(String, HeaderMap)>>> = Arc::new(Mutex::new(Vec::new()));
        let capture = Arc::clone(&captured);
        let upstream = Router::new().route(
            "/codex/models",
            get(move |uri: Uri, headers: HeaderMap| {
                let capture = Arc::clone(&capture);
                async move {
                    capture.lock().await.push((uri.to_string(), headers));
                    Json(codex_catalog_body())
                }
            }),
        );
        let upstream_listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind upstream");
        let upstream_address = upstream_listener.local_addr().expect("upstream address");
        let upstream_server =
            tokio::spawn(async move { axum::serve(upstream_listener, upstream).await });
        let upstream_url = format!("http://{upstream_address}");

        let database_path = test_database_path();
        let db = Db::open(&database_path).await.expect("open test database");
        let seed = seed_relay_route(
            &db,
            RelayRouteSeed {
                base_url: upstream_url,
                provider: "openai-compatible",
                auth_mode: "codex-oauth",
                provider_secret: json!({
                    "type": "token-toxication-codex-oauth-v1",
                    "refresh": "synthetic-refresh",
                    "access": "synthetic-provider-key",
                    "expires": Utc::now().timestamp_millis() + 3_600_000,
                    "accountId": "synthetic-account"
                })
                .to_string(),
                wire_api: "openai-responses",
                public_model: "gpt-6.1-sol",
                upstream_model: "gpt-6.1-sol",
            },
        )
        .await;
        let state = test_state(db, database_path.clone());

        let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
        let address = listener.local_addr().expect("address");
        let server = tokio::spawn(async move {
            axum::serve(
                listener,
                crate::app(state, PathBuf::from("missing-admin-assets")),
            )
            .await
            .expect("serve");
        });

        let http = test_http_client();
        let endpoint = format!("http://{address}/openai/v1/codex/models?client_version=0.162.1");
        let response = http
            .get(&endpoint)
            .expect("build catalog request")
            .bearer_auth(&seed.relay_secret)
            .send()
            .await
            .expect("send catalog request");
        assert_eq!(response.status(), StatusCode::OK);

        let body: Value = serde_json::from_slice(&response.bytes().await.expect("read catalog"))
            .expect("decode catalog");
        let slugs = body["models"]
            .as_array()
            .expect("models array")
            .iter()
            .map(|model| model["slug"].as_str().unwrap_or_default().to_string())
            .collect::<Vec<_>>();
        assert_eq!(
            slugs,
            vec!["gpt-6.1-sol", "gpt-6-astra", "gpt-5.6-sol"],
            "every model the client can parse is served; only the entry without \
             instructions is dropped"
        );

        // Upstream values are what the client sees. TT neither rewrites the
        // context window nor invents `-fast` models or a default service tier.
        assert_eq!(body["models"][0]["context_window"], json!(500_000));
        assert_eq!(body["models"][0]["default_service_tier"], Value::Null);
        assert_eq!(body["models"][1]["context_window"], json!(272_000));
        assert_eq!(body["models"][2]["context_window"], json!(272_000));

        // The upstream call carried the relay's own account identity, not client input.
        let (uri, headers) = captured
            .lock()
            .await
            .first()
            .cloned()
            .expect("captured upstream request");
        assert_eq!(uri, "/codex/models?client_version=0.162.1");
        assert_eq!(headers["authorization"], "Bearer synthetic-provider-key");
        assert_eq!(headers["chatgpt-account-id"], "synthetic-account");
        assert_eq!(headers["originator"], "opencode");

        upstream_server.abort();
        server.abort();
        let _ = server.await;
        remove_test_database(&database_path);
    }

    /// Force the catalog cache to expire between requests so a test can observe
    /// what happens on the revalidation path.
    fn expire_catalog_cache(state: &mut AppState) {
        state.codex_catalog_cache = Arc::new(crate::codex_catalog::cache::CatalogCache::new(
            Duration::ZERO,
        ));
    }

    #[tokio::test]
    async fn codex_catalog_endpoint_serves_the_stored_catalog_when_upstream_fails() {
        let calls = Arc::new(Mutex::new(0usize));
        let counter = Arc::clone(&calls);
        let upstream = Router::new().route(
            "/codex/models",
            get(move || {
                let counter = Arc::clone(&counter);
                async move {
                    let mut calls = counter.lock().await;
                    *calls += 1;
                    if *calls == 1 {
                        (StatusCode::OK, Json(codex_catalog_body()))
                    } else {
                        // The client would lose every model if this reached it.
                        (StatusCode::BAD_GATEWAY, Json(json!({"error": "upstream"})))
                    }
                }
            }),
        );
        let upstream_listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind upstream");
        let upstream_address = upstream_listener.local_addr().expect("upstream address");
        let upstream_server =
            tokio::spawn(async move { axum::serve(upstream_listener, upstream).await });

        let database_path = test_database_path();
        let db = Db::open(&database_path).await.expect("open test database");
        let seed = seed_relay_route(
            &db,
            RelayRouteSeed {
                base_url: format!("http://{upstream_address}"),
                provider: "openai-compatible",
                auth_mode: "codex-oauth",
                provider_secret: json!({
                    "type": "token-toxication-codex-oauth-v1",
                    "refresh": "synthetic-refresh",
                    "access": "synthetic-provider-key",
                    "expires": Utc::now().timestamp_millis() + 3_600_000,
                    "accountId": "synthetic-account"
                })
                .to_string(),
                wire_api: "openai-responses",
                public_model: "gpt-6.1-sol",
                upstream_model: "gpt-6.1-sol",
            },
        )
        .await;
        let mut state = test_state(db, database_path.clone());
        expire_catalog_cache(&mut state);

        let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
        let address = listener.local_addr().expect("address");
        let server = tokio::spawn(async move {
            axum::serve(
                listener,
                crate::app(state, PathBuf::from("missing-admin-assets")),
            )
            .await
            .expect("serve");
        });

        let http = test_http_client();
        let endpoint = format!("http://{address}/openai/v1/codex/models?client_version=0.162.1");
        let mut bodies = Vec::new();
        for _ in 0..2 {
            let response = http
                .get(&endpoint)
                .expect("build catalog request")
                .bearer_auth(&seed.relay_secret)
                .send()
                .await
                .expect("send catalog request");
            assert_eq!(
                response.status(),
                StatusCode::OK,
                "an upstream failure must not empty the client's model list"
            );
            bodies.push(response.bytes().await.expect("read catalog"));
        }
        assert_eq!(*calls.lock().await, 2, "both requests reached upstream");
        assert_eq!(
            bodies[0], bodies[1],
            "the stored catalog must be served unchanged"
        );
        let body: Value = serde_json::from_slice(&bodies[1]).expect("decode served catalog");
        assert_eq!(body["models"][0]["slug"], json!("gpt-6.1-sol"));

        upstream_server.abort();
        server.abort();
        let _ = server.await;
        remove_test_database(&database_path);
    }

    #[tokio::test]
    async fn codex_catalog_endpoint_always_sends_a_usable_client_version_upstream() {
        let captured: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
        let capture = Arc::clone(&captured);
        let upstream = Router::new().route(
            "/codex/models",
            get(move |uri: Uri| {
                let capture = Arc::clone(&capture);
                async move {
                    capture.lock().await.push(uri.to_string());
                    Json(codex_catalog_body())
                }
            }),
        );
        let upstream_listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind upstream");
        let upstream_address = upstream_listener.local_addr().expect("upstream address");
        let upstream_server =
            tokio::spawn(async move { axum::serve(upstream_listener, upstream).await });

        let database_path = test_database_path();
        let db = Db::open(&database_path).await.expect("open test database");
        let seed = seed_relay_route(
            &db,
            RelayRouteSeed {
                base_url: format!("http://{upstream_address}"),
                provider: "openai-compatible",
                auth_mode: "codex-oauth",
                provider_secret: json!({
                    "type": "token-toxication-codex-oauth-v1",
                    "refresh": "synthetic-refresh",
                    "access": "synthetic-provider-key",
                    "expires": Utc::now().timestamp_millis() + 3_600_000,
                    "accountId": "synthetic-account"
                })
                .to_string(),
                wire_api: "openai-responses",
                public_model: "gpt-6.1-sol",
                upstream_model: "gpt-6.1-sol",
            },
        )
        .await;
        let mut state = test_state(db, database_path.clone());
        expire_catalog_cache(&mut state);

        let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
        let address = listener.local_addr().expect("address");
        let server = tokio::spawn(async move {
            axum::serve(
                listener,
                crate::app(state, PathBuf::from("missing-admin-assets")),
            )
            .await
            .expect("serve");
        });

        let http = test_http_client();
        // A client with no version and a client with an unexpected one must both
        // get a catalog: the backend 400s on an absent parameter, and rejecting
        // the odd value would cost the client every model.
        for query in ["", "?client_version=%2Bweird+build"] {
            let response = http
                .get(&format!("http://{address}/openai/v1/codex/models{query}"))
                .expect("build catalog request")
                .bearer_auth(&seed.relay_secret)
                .send()
                .await
                .expect("send catalog request");
            assert_eq!(response.status(), StatusCode::OK, "query: {query}");
        }

        let upstream_uris = captured.lock().await.clone();
        assert_eq!(upstream_uris.len(), 2);
        assert!(
            upstream_uris[0].contains("client_version=0.0.0"),
            "a missing client version must still be sent upstream: {}",
            upstream_uris[0]
        );
        assert!(
            upstream_uris[1].contains("client_version=weirdbuild"),
            "an unexpected client version must be sanitized, not rejected: {}",
            upstream_uris[1]
        );

        upstream_server.abort();
        server.abort();
        let _ = server.await;
        remove_test_database(&database_path);
    }

    #[tokio::test]
    async fn codex_fast_variant_requests_route_through_their_base_model() {
        let captured: Arc<Mutex<Vec<Value>>> = Arc::new(Mutex::new(Vec::new()));
        let capture = Arc::clone(&captured);
        let upstream = Router::new().route(
            "/codex/responses",
            axum::routing::post(move |Json(body): Json<Value>| {
                let capture = Arc::clone(&capture);
                async move {
                    capture.lock().await.push(body);
                    (
                        StatusCode::OK,
                        Json(json!({"id": "resp_probe", "output": []})),
                    )
                }
            }),
        );
        let upstream_listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind upstream");
        let upstream_address = upstream_listener.local_addr().expect("upstream address");
        let upstream_server =
            tokio::spawn(async move { axum::serve(upstream_listener, upstream).await });

        let database_path = test_database_path();
        let db = Db::open(&database_path).await.expect("open test database");
        let seed = seed_relay_route(
            &db,
            RelayRouteSeed {
                base_url: format!("http://{upstream_address}"),
                provider: "openai-compatible",
                auth_mode: "codex-oauth",
                provider_secret: json!({
                    "type": "token-toxication-codex-oauth-v1",
                    "refresh": "synthetic-refresh",
                    "access": "synthetic-provider-key",
                    "expires": Utc::now().timestamp_millis() + 3_600_000,
                    "accountId": "synthetic-account"
                })
                .to_string(),
                wire_api: "openai-responses",
                public_model: "gpt-6.1-sol",
                upstream_model: "gpt-6.1-sol",
            },
        )
        .await;
        let state = test_state(db, database_path.clone());
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
        let address = listener.local_addr().expect("address");
        let server = tokio::spawn(async move {
            axum::serve(
                listener,
                crate::app(state, PathBuf::from("missing-admin-assets")),
            )
            .await
            .expect("serve");
        });

        let http = test_http_client();
        for model in ["gpt-6.1-sol", "gpt-6.1-sol-fast"] {
            let response = http
                .post(&format!("http://{address}/openai/v1/responses"))
                .expect("build request")
                .bearer_auth(&seed.relay_secret)
                .header(
                    header::CONTENT_TYPE,
                    HeaderValue::from_static("application/json"),
                )
                .body(
                    serde_json::to_vec(&json!({
                        "model": model,
                        "input": "hi",
                        "stream": false,
                        "service_tier": "priority"
                    }))
                    .expect("encode body"),
                )
                .send()
                .await
                .expect("send request");
            assert_eq!(
                response.status(),
                StatusCode::OK,
                "the catalog advertises {model}, so it must be routable"
            );
        }

        // The alias reaches the upstream as its base model, exactly as OpenCode
        // resolves `-fast` before sending.
        let bodies = captured.lock().await;
        assert_eq!(bodies.len(), 2);
        assert_eq!(bodies[0]["model"], json!("gpt-6.1-sol"));
        assert_eq!(bodies[1]["model"], json!("gpt-6.1-sol"));
        // The client's Fast selection is forwarded, not rewritten by routing.
        assert_eq!(bodies[1]["service_tier"], json!("priority"));

        upstream_server.abort();
        server.abort();
        let _ = server.await;
        remove_test_database(&database_path);
    }

    #[tokio::test]
    async fn codex_catalog_endpoint_requires_a_subscription_account() {
        let database_path = test_database_path();
        let db = Db::open(&database_path).await.expect("open test database");
        db.create_api_key(
            CreateApiKeyRequest {
                name: "relay test key".to_string(),
                description: String::new(),
                permissions: Vec::new(),
                rate_limit_per_minute: 0,
                concurrency_limit: 0,
                daily_cost_limit: 0.0,
                expires_at: None,
            },
            "tokentoxication-relay-test-key",
        )
        .await
        .expect("create relay key");
        let state = test_state(db, database_path.clone());

        let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
        let address = listener.local_addr().expect("address");
        let server = tokio::spawn(async move {
            axum::serve(
                listener,
                crate::app(state, PathBuf::from("missing-admin-assets")),
            )
            .await
            .expect("serve");
        });

        let http = test_http_client();
        let endpoint = format!("http://{address}/openai/v1/codex/models");
        let response = http
            .get(&endpoint)
            .expect("build catalog request")
            .bearer_auth("tokentoxication-relay-test-key")
            .send()
            .await
            .expect("send catalog request");
        assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);

        let unauthenticated = http
            .get(&endpoint)
            .expect("build unauthenticated request")
            .send()
            .await
            .expect("send unauthenticated request");
        assert_eq!(unauthenticated.status(), StatusCode::UNAUTHORIZED);

        server.abort();
        let _ = server.await;
        remove_test_database(&database_path);
    }

    #[tokio::test]
    async fn openai_chat_relay_rewrites_the_model_and_records_the_attempt() {
        let seen = Arc::new(Mutex::new(None));
        let handler_seen = Arc::clone(&seen);
        let upstream = Router::new().route(
            "/chat/completions",
            post(move |headers: HeaderMap, Json(body): Json<Value>| {
                let handler_seen = Arc::clone(&handler_seen);
                async move {
                    *handler_seen.lock().await = Some((headers, body));
                    (
                        StatusCode::OK,
                        Json(json!({
                            "id": "chat-1",
                            "usage": {
                                "prompt_tokens": 3,
                                "prompt_tokens_details": {"cached_tokens": 2},
                                "completion_tokens": 5
                            }
                        })),
                    )
                }
            }),
        );
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind upstream");
        let address = listener.local_addr().expect("upstream address");
        let server = tokio::spawn(async move {
            axum::serve(listener, upstream)
                .await
                .expect("serve upstream");
        });

        let database_path = test_database_path();
        let db = Db::open(&database_path).await.expect("open test database");
        let SeededRelayRoute {
            relay_secret,
            account_id,
            route_id,
        } = seed_relay_route(
            &db,
            RelayRouteSeed {
                base_url: format!("http://{address}"),
                provider: "openai-compatible",
                auth_mode: "bearer",
                provider_secret: "provider-secret".to_string(),
                wire_api: "openai-chat",
                public_model: "public-chat-model",
                upstream_model: "upstream-chat-model",
            },
        )
        .await;
        let state = test_state(db, database_path.clone());

        let response = relay_openai_chat(
            State(state.clone()),
            relay_headers(&relay_secret),
            Uri::from_static("/openai/v1/chat/completions"),
            Bytes::from_static(
                br#"{"model":"public-chat-model","messages":[{"role":"user","content":"hello"}]}"#,
            ),
        )
        .await
        .expect("relay response");
        server.abort();

        assert_eq!(response.status(), StatusCode::OK);
        let relay_body: Value = serde_json::from_slice(
            &to_bytes(response.into_body(), usize::MAX)
                .await
                .expect("read relay response"),
        )
        .expect("decode relay response");
        assert_eq!(relay_body["id"], "chat-1");

        let (headers, body) = seen.lock().await.take().expect("capture upstream request");
        assert_eq!(body["model"], "upstream-chat-model");
        assert_eq!(
            headers
                .get(header::AUTHORIZATION)
                .and_then(|value| value.to_str().ok()),
            Some("Bearer provider-secret")
        );

        let logs = state
            .db
            .list_request_logs(1)
            .await
            .expect("list request logs");
        assert_eq!(logs.len(), 1);
        assert_eq!(logs[0].status_code, 200);
        assert_eq!(logs[0].model.as_deref(), Some("public-chat-model"));
        assert_eq!(
            logs[0].upstream_model.as_deref(),
            Some("upstream-chat-model")
        );
        assert_eq!((logs[0].input_tokens, logs[0].output_tokens), (3, 5));
        assert_eq!(logs[0].cached_input_tokens, 2);
        assert!(logs[0].error.is_none());
        assert_eq!(
            state
                .db
                .get_provider_account(&account_id)
                .await
                .expect("get provider account")
                .expect("provider account")
                .status,
            "healthy"
        );
        assert_eq!(
            state
                .db
                .get_provider_model_route(&route_id)
                .await
                .expect("get provider route")
                .expect("provider route")
                .last_status_code,
            Some(200)
        );

        drop(state);
        remove_test_database(&database_path);
    }

    #[tokio::test]
    async fn streamed_openai_responses_record_completion_usage() {
        const STREAM: &str = concat!(
            "event: response.completed\n",
            "data: {\"type\":\"response.completed\",\"response\":{\"usage\":{\"input_tokens\":13,\"input_tokens_details\":{\"cached_tokens\":10},\"output_tokens\":8,\"total_tokens\":21}}}\n\n"
        );
        let upstream = Router::new().route(
            "/v1/responses",
            post(|| async {
                let chunks = stream::iter([
                    Ok::<_, Infallible>(Bytes::from_static(
                        b"event: response.completed\ndata: {\"type\":\"response.comp",
                    )),
                    Ok(Bytes::from_static(
                        b"leted\",\"response\":{\"usage\":{\"input_tok",
                    )),
                    Ok(Bytes::from_static(
                        b"ens\":13,\"input_tokens_details\":{\"cached_tokens\":10},\"output_tokens\":8,\"total_tokens\":21}}}\n\n",
                    )),
                ]);
                Response::builder()
                    .status(StatusCode::OK)
                    .header(header::CONTENT_TYPE, "text/event-stream")
                    .body(Body::from_stream(chunks))
                    .expect("build upstream response")
            }),
        );
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind upstream");
        let address = listener.local_addr().expect("upstream address");
        let server = tokio::spawn(async move {
            axum::serve(listener, upstream)
                .await
                .expect("serve upstream");
        });

        let database_path = test_database_path();
        let db = Db::open(&database_path).await.expect("open test database");
        let SeededRelayRoute { relay_secret, .. } = seed_relay_route(
            &db,
            RelayRouteSeed {
                base_url: format!("http://{address}"),
                provider: "openai-compatible",
                auth_mode: "bearer",
                provider_secret: "provider-secret".to_string(),
                wire_api: "openai-responses",
                public_model: "public-responses-model",
                upstream_model: "upstream-responses-model",
            },
        )
        .await;
        let state = test_state(db, database_path.clone());

        let response = relay_openai_responses(
            State(state.clone()),
            relay_headers(&relay_secret),
            Uri::from_static("/openai/v1/responses"),
            Bytes::from_static(
                br#"{"model":"public-responses-model","input":"hello","stream":true}"#,
            ),
        )
        .await
        .expect("relay response");
        assert_eq!(response.status(), StatusCode::OK);
        let relay_body = to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("read relay response");
        assert_eq!(relay_body.as_ref(), STREAM.as_bytes());
        server.abort();

        let logs = state
            .db
            .list_request_logs(1)
            .await
            .expect("list request logs");
        assert_eq!(logs.len(), 1);
        assert_eq!((logs[0].input_tokens, logs[0].output_tokens), (13, 8));
        assert_eq!(logs[0].cached_input_tokens, 10);

        drop(state);
        remove_test_database(&database_path);
    }

    #[tokio::test]
    async fn streamed_openai_responses_record_application_failure() {
        const STREAM: &str = concat!(
            "event: error\n",
            "data: {\"type\":\"error\",\"code\":\"server_error\",\"message\":\"temporary upstream failure\",\"sequence_number\":3}\n\n",
            "event: response.failed\n",
            "data: {\"type\":\"response.failed\",\"response\":{\"status\":\"failed\",\"error\":{\"code\":\"server_error\",\"message\":\"temporary upstream failure\"},\"usage\":{\"input_tokens\":13,\"input_tokens_details\":{\"cached_tokens\":10},\"output_tokens\":8,\"total_tokens\":21}},\"sequence_number\":4}\n\n"
        );
        let upstream = Router::new().route(
            "/v1/responses",
            post(|| async {
                Response::builder()
                    .status(StatusCode::OK)
                    .header(header::CONTENT_TYPE, "text/event-stream")
                    .body(Body::from(STREAM))
                    .expect("build upstream response")
            }),
        );
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind upstream");
        let address = listener.local_addr().expect("upstream address");
        let server = tokio::spawn(async move {
            axum::serve(listener, upstream)
                .await
                .expect("serve upstream");
        });

        let database_path = test_database_path();
        let db = Db::open(&database_path).await.expect("open test database");
        let SeededRelayRoute {
            relay_secret,
            route_id,
            ..
        } = seed_relay_route(
            &db,
            RelayRouteSeed {
                base_url: format!("http://{address}"),
                provider: "openai-compatible",
                auth_mode: "bearer",
                provider_secret: "provider-secret".to_string(),
                wire_api: "openai-responses",
                public_model: "public-responses-model",
                upstream_model: "upstream-responses-model",
            },
        )
        .await;
        let state = test_state(db, database_path.clone());

        let response = relay_openai_responses(
            State(state.clone()),
            relay_headers(&relay_secret),
            Uri::from_static("/openai/v1/responses"),
            Bytes::from_static(
                br#"{"model":"public-responses-model","input":"hello","stream":true}"#,
            ),
        )
        .await
        .expect("relay response");
        assert_eq!(response.status(), StatusCode::OK);
        let relay_body = to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("read relay response");
        assert_eq!(relay_body.as_ref(), STREAM.as_bytes());
        server.abort();

        let logs = state
            .db
            .list_request_logs(1)
            .await
            .expect("list request logs");
        assert_eq!(logs.len(), 1);
        assert_eq!(logs[0].status_code, 502);
        assert_eq!(
            (
                logs[0].input_tokens,
                logs[0].cached_input_tokens,
                logs[0].output_tokens,
            ),
            (13, 10, 8)
        );
        assert_eq!(
            logs[0].error.as_deref(),
            Some("upstream stream failed (server_error)")
        );
        let route = state
            .db
            .get_provider_model_route(&route_id)
            .await
            .expect("get provider route")
            .expect("provider route");
        assert_eq!(route.status, "cooling_down");
        assert_eq!(route.last_status_code, Some(502));
        assert!(route.cooldown_until.is_some());

        drop(state);
        remove_test_database(&database_path);
    }

    #[tokio::test]
    async fn observed_application_failure_wins_over_client_disconnect() {
        const STREAM: &str = concat!(
            "event: error\n",
            "data: {\"type\":\"error\",\"code\":\"server_error\",\"message\":\"temporary upstream failure\"}\n\n"
        );
        let upstream = Router::new().route(
            "/v1/responses",
            post(|| async {
                let chunks = stream::once(async {
                    Ok::<_, Infallible>(Bytes::from_static(STREAM.as_bytes()))
                })
                .chain(stream::pending());
                Response::builder()
                    .status(StatusCode::OK)
                    .header(header::CONTENT_TYPE, "text/event-stream")
                    .body(Body::from_stream(chunks))
                    .expect("build upstream response")
            }),
        );
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind upstream");
        let address = listener.local_addr().expect("upstream address");
        let server = tokio::spawn(async move {
            axum::serve(listener, upstream)
                .await
                .expect("serve upstream");
        });

        let database_path = test_database_path();
        let db = Db::open(&database_path).await.expect("open test database");
        let SeededRelayRoute {
            relay_secret,
            route_id,
            ..
        } = seed_relay_route(
            &db,
            RelayRouteSeed {
                base_url: format!("http://{address}"),
                provider: "openai-compatible",
                auth_mode: "bearer",
                provider_secret: "provider-secret".to_string(),
                wire_api: "openai-responses",
                public_model: "public-responses-model",
                upstream_model: "upstream-responses-model",
            },
        )
        .await;
        let state = test_state(db, database_path.clone());

        let response = relay_openai_responses(
            State(state.clone()),
            relay_headers(&relay_secret),
            Uri::from_static("/openai/v1/responses"),
            Bytes::from_static(
                br#"{"model":"public-responses-model","input":"hello","stream":true}"#,
            ),
        )
        .await
        .expect("relay response");
        let mut body = response.into_body().into_data_stream();
        assert_eq!(
            body.next()
                .await
                .expect("first response chunk")
                .expect("valid response chunk")
                .as_ref(),
            STREAM.as_bytes()
        );
        drop(body);

        let mut logs = Vec::new();
        for _ in 0..20 {
            logs = state
                .db
                .list_request_logs(1)
                .await
                .expect("list request logs");
            if !logs.is_empty() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        server.abort();

        assert_eq!(logs.len(), 1);
        assert_eq!(logs[0].status_code, 502);
        assert_eq!(
            logs[0].error.as_deref(),
            Some("upstream stream failed (server_error)")
        );
        let route = state
            .db
            .get_provider_model_route(&route_id)
            .await
            .expect("get provider route")
            .expect("provider route");
        assert_eq!(route.status, "cooling_down");
        assert_eq!(route.last_status_code, Some(502));
        assert!(route.cooldown_until.is_some());

        drop(state);
        remove_test_database(&database_path);
    }

    #[tokio::test]
    async fn disconnected_stream_is_recorded_without_penalizing_the_route() {
        let upstream = Router::new().route(
            "/v1/responses",
            post(|| async {
                let chunks = stream::once(async {
                    Ok::<_, Infallible>(Bytes::from_static(b"data: {}\n\n"))
                })
                .chain(stream::pending());
                Response::builder()
                    .status(StatusCode::OK)
                    .header(header::CONTENT_TYPE, "text/event-stream")
                    .body(Body::from_stream(chunks))
                    .expect("build upstream response")
            }),
        );
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind upstream");
        let address = listener.local_addr().expect("upstream address");
        let server = tokio::spawn(async move {
            axum::serve(listener, upstream)
                .await
                .expect("serve upstream");
        });

        let database_path = test_database_path();
        let db = Db::open(&database_path).await.expect("open test database");
        let SeededRelayRoute {
            relay_secret,
            route_id,
            ..
        } = seed_relay_route(
            &db,
            RelayRouteSeed {
                base_url: format!("http://{address}"),
                provider: "openai-compatible",
                auth_mode: "bearer",
                provider_secret: "provider-secret".to_string(),
                wire_api: "openai-responses",
                public_model: "public-responses-model",
                upstream_model: "upstream-responses-model",
            },
        )
        .await;
        let state = test_state(db, database_path.clone());

        let response = relay_openai_responses(
            State(state.clone()),
            relay_headers(&relay_secret),
            Uri::from_static("/openai/v1/responses"),
            Bytes::from_static(
                br#"{"model":"public-responses-model","input":"hello","stream":true}"#,
            ),
        )
        .await
        .expect("relay response");
        let mut body = response.into_body().into_data_stream();
        body.next()
            .await
            .expect("first response chunk")
            .expect("valid response chunk");
        drop(body);

        let mut logs = Vec::new();
        for _ in 0..20 {
            logs = state
                .db
                .list_request_logs(1)
                .await
                .expect("list request logs");
            if !logs.is_empty() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        server.abort();

        assert_eq!(logs.len(), 1);
        assert_eq!(logs[0].status_code, 499);
        assert_eq!(logs[0].cached_input_tokens, 0);
        assert_eq!(
            logs[0].error.as_deref(),
            Some("client disconnected before the upstream stream completed")
        );
        let route = state
            .db
            .get_provider_model_route(&route_id)
            .await
            .expect("get provider route")
            .expect("provider route");
        assert_eq!(route.status, "healthy");
        assert_eq!(route.last_status_code, None);

        drop(state);
        remove_test_database(&database_path);
    }

    #[tokio::test]
    async fn stalled_stream_ends_without_waiting_for_the_upstream() {
        let upstream = Router::new().route(
            "/v1/responses",
            post(|| async {
                let chunks = stream::pending::<Result<Bytes, Infallible>>();
                Response::builder()
                    .status(StatusCode::OK)
                    .header(header::CONTENT_TYPE, "text/event-stream")
                    .body(Body::from_stream(chunks))
                    .expect("build upstream response")
            }),
        );
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind upstream");
        let address = listener.local_addr().expect("upstream address");
        let server = tokio::spawn(async move {
            axum::serve(listener, upstream)
                .await
                .expect("serve upstream");
        });

        let database_path = test_database_path();
        let db = Db::open(&database_path).await.expect("open test database");
        let SeededRelayRoute {
            relay_secret,
            route_id,
            ..
        } = seed_relay_route(
            &db,
            RelayRouteSeed {
                base_url: format!("http://{address}"),
                provider: "openai-compatible",
                auth_mode: "bearer",
                provider_secret: "provider-secret".to_string(),
                wire_api: "openai-responses",
                public_model: "public-responses-model",
                upstream_model: "upstream-responses-model",
            },
        )
        .await;
        let mut state = test_state(db, database_path.clone());
        state.relay_stream_idle_timeout = Duration::from_millis(20);

        let response = relay_openai_responses(
            State(state.clone()),
            relay_headers(&relay_secret),
            Uri::from_static("/openai/v1/responses"),
            Bytes::from_static(
                br#"{"model":"public-responses-model","input":"hello","stream":true}"#,
            ),
        )
        .await
        .expect("relay response");
        let mut body = response.into_body().into_data_stream();
        let error = tokio::time::timeout(Duration::from_millis(100), body.next())
            .await
            .expect("stalled stream must end promptly")
            .expect("stalled stream must return an error")
            .expect_err("stalled stream must fail");
        assert!(error.to_string().contains("upstream stream"));

        let mut logs = Vec::new();
        for _ in 0..20 {
            logs = state
                .db
                .list_request_logs(1)
                .await
                .expect("list request logs");
            if !logs.is_empty() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        server.abort();

        assert_eq!(logs.len(), 1);
        assert_eq!(logs[0].status_code, 502);
        assert!(
            logs[0]
                .error
                .as_deref()
                .is_some_and(|error| error.contains("upstream stream"))
        );
        let route = state
            .db
            .get_provider_model_route(&route_id)
            .await
            .expect("get provider route")
            .expect("provider route");
        assert_eq!(route.status, "cooling_down");
        assert_eq!(route.last_status_code, None);

        drop(state);
        remove_test_database(&database_path);
    }

    #[tokio::test]
    async fn shutdown_ends_stalled_stream_without_penalizing_the_route() {
        let upstream = Router::new().route(
            "/v1/responses",
            post(|| async {
                let chunks = stream::pending::<Result<Bytes, Infallible>>();
                Response::builder()
                    .status(StatusCode::OK)
                    .header(header::CONTENT_TYPE, "text/event-stream")
                    .body(Body::from_stream(chunks))
                    .expect("build upstream response")
            }),
        );
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind upstream");
        let address = listener.local_addr().expect("upstream address");
        let server = tokio::spawn(async move {
            axum::serve(listener, upstream)
                .await
                .expect("serve upstream");
        });

        let database_path = test_database_path();
        let db = Db::open(&database_path).await.expect("open test database");
        let SeededRelayRoute {
            relay_secret,
            route_id,
            ..
        } = seed_relay_route(
            &db,
            RelayRouteSeed {
                base_url: format!("http://{address}"),
                provider: "openai-compatible",
                auth_mode: "bearer",
                provider_secret: "provider-secret".to_string(),
                wire_api: "openai-responses",
                public_model: "public-responses-model",
                upstream_model: "upstream-responses-model",
            },
        )
        .await;
        let state = test_state(db, database_path.clone());

        let response = relay_openai_responses(
            State(state.clone()),
            relay_headers(&relay_secret),
            Uri::from_static("/openai/v1/responses"),
            Bytes::from_static(
                br#"{"model":"public-responses-model","input":"hello","stream":true}"#,
            ),
        )
        .await
        .expect("relay response");
        let mut body = response.into_body().into_data_stream();
        state.shutdown.cancel();
        assert!(
            tokio::time::timeout(Duration::from_millis(100), body.next())
                .await
                .expect("shutdown must end the stream promptly")
                .is_none()
        );

        let mut logs = Vec::new();
        for _ in 0..20 {
            logs = state
                .db
                .list_request_logs(1)
                .await
                .expect("list request logs");
            if !logs.is_empty() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        server.abort();

        assert_eq!(logs.len(), 1);
        assert_eq!(logs[0].status_code, 499);
        assert_eq!(
            logs[0].error.as_deref(),
            Some("server shut down before the upstream stream completed")
        );
        let route = state
            .db
            .get_provider_model_route(&route_id)
            .await
            .expect("get provider route")
            .expect("provider route");
        assert_eq!(route.status, "healthy");
        assert_eq!(route.last_status_code, None);

        drop(state);
        remove_test_database(&database_path);
    }

    #[tokio::test]
    async fn upstream_rate_limit_marks_the_route_and_records_the_failure() {
        let upstream = Router::new().route(
            "/chat/completions",
            post(|| async {
                (
                    StatusCode::TOO_MANY_REQUESTS,
                    Json(json!({"error": {"message": "quota exhausted"}})),
                )
            }),
        );
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind upstream");
        let address = listener.local_addr().expect("upstream address");
        let server = tokio::spawn(async move {
            axum::serve(listener, upstream)
                .await
                .expect("serve upstream");
        });

        let database_path = test_database_path();
        let db = Db::open(&database_path).await.expect("open test database");
        let SeededRelayRoute {
            relay_secret,
            route_id,
            ..
        } = seed_relay_route(
            &db,
            RelayRouteSeed {
                base_url: format!("http://{address}"),
                provider: "openai-compatible",
                auth_mode: "bearer",
                provider_secret: "provider-secret".to_string(),
                wire_api: "openai-chat",
                public_model: "public-chat-model",
                upstream_model: "upstream-chat-model",
            },
        )
        .await;
        let state = test_state(db, database_path.clone());

        let response = relay_openai_chat(
            State(state.clone()),
            relay_headers(&relay_secret),
            Uri::from_static("/openai/v1/chat/completions"),
            Bytes::from_static(
                br#"{"model":"public-chat-model","messages":[{"role":"user","content":"hello"}]}"#,
            ),
        )
        .await
        .expect("relay response");
        server.abort();

        assert_eq!(response.status(), StatusCode::TOO_MANY_REQUESTS);
        let logs = state
            .db
            .list_request_logs(1)
            .await
            .expect("list request logs");
        assert_eq!(logs.len(), 1);
        assert_eq!(logs[0].status_code, 429);
        assert_eq!(logs[0].cached_input_tokens, 0);
        assert_eq!(logs[0].error.as_deref(), Some("upstream returned 429"));
        let route = state
            .db
            .get_provider_model_route(&route_id)
            .await
            .expect("get provider route")
            .expect("provider route");
        assert_eq!(route.status, "cooling_down");
        assert_eq!(route.last_status_code, Some(429));
        assert!(route.cooldown_until.is_some());
        let second = relay_openai_chat(
            State(state.clone()),
            relay_headers(&relay_secret),
            Uri::from_static("/openai/v1/chat/completions"),
            Bytes::from_static(
                br#"{"model":"public-chat-model","messages":[{"role":"user","content":"hello"}]}"#,
            ),
        )
        .await
        .expect_err("limited account should remain unavailable");
        assert_eq!(second.status(), StatusCode::TOO_MANY_REQUESTS);

        drop(state);
        remove_test_database(&database_path);
    }

    #[tokio::test]
    async fn usage_limit_retries_another_account_and_keeps_the_first_account_limited() {
        let limited_upstream = Router::new().route(
            "/chat/completions",
            post(|| async {
                (
                    StatusCode::TOO_MANY_REQUESTS,
                    Json(json!({
                        "error": {
                            "code": "usage_limit_reached",
                            "message": "usage limit reached"
                        }
                    })),
                )
            }),
        );
        let limited_listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind limited upstream");
        let limited_address = limited_listener.local_addr().expect("limited address");
        let limited_server = tokio::spawn(async move {
            axum::serve(limited_listener, limited_upstream)
                .await
                .expect("serve limited upstream");
        });

        let backup_upstream = Router::new().route(
            "/chat/completions",
            post(|| async {
                (
                    StatusCode::OK,
                    Json(json!({
                        "id": "backup-success",
                        "usage": {"prompt_tokens": 3, "completion_tokens": 5}
                    })),
                )
            }),
        );
        let backup_listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind backup upstream");
        let backup_address = backup_listener.local_addr().expect("backup address");
        let backup_server = tokio::spawn(async move {
            axum::serve(backup_listener, backup_upstream)
                .await
                .expect("serve backup upstream");
        });

        let database_path = test_database_path();
        let db = Db::open(&database_path).await.expect("open test database");
        let SeededRelayRoute {
            relay_secret,
            account_id: limited_account_id,
            ..
        } = seed_relay_route(
            &db,
            RelayRouteSeed {
                base_url: format!("http://{limited_address}"),
                provider: "openai-compatible",
                auth_mode: "bearer",
                provider_secret: "limited-secret".to_string(),
                wire_api: "openai-chat",
                public_model: "public-chat-model",
                upstream_model: "limited-upstream-model",
            },
        )
        .await;
        let backup_account = db
            .create_provider_account(CreateProviderAccountRequest {
                name: "backup provider".to_string(),
                provider: "openai-compatible".to_string(),
                base_url: format!("http://{backup_address}"),
                auth_mode: "bearer".to_string(),
                wire_api: "openai-chat".to_string(),
                api_key: "backup-secret".to_string(),
                is_active: true,
                max_running_requests: 0,
            })
            .await
            .expect("create backup account");
        db.create_provider_model_route(CreateProviderModelRouteRequest {
            public_model_id: "public-chat-model".to_string(),
            provider_account_id: backup_account.id.clone(),
            upstream_model_id: "backup-upstream-model".to_string(),
            wire_api: "openai-chat".to_string(),
            role: "backup".to_string(),
            enabled: true,
            weight: 100,
            strip_params: Vec::new(),
        })
        .await
        .expect("create backup route");
        let state = test_state(db, database_path.clone());

        let response = relay_openai_chat(
            State(state.clone()),
            relay_headers(&relay_secret),
            Uri::from_static("/openai/v1/chat/completions"),
            Bytes::from_static(
                br#"{"model":"public-chat-model","messages":[{"role":"user","content":"hello"}]}"#,
            ),
        )
        .await
        .expect("relay response");
        let response_body = to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("response body");

        limited_server.abort();
        backup_server.abort();
        assert_eq!(
            serde_json::from_slice::<Value>(&response_body)
                .expect("response json")
                .get("id")
                .and_then(Value::as_str),
            Some("backup-success")
        );
        let limited_account = state
            .db
            .get_provider_account(&limited_account_id)
            .await
            .expect("get limited account")
            .expect("limited account");
        assert_eq!(limited_account.status, "partially_limited");
        let candidates = state
            .db
            .list_provider_route_candidates("openai-chat", "public-chat-model")
            .await
            .expect("list candidates");
        assert_eq!(candidates.len(), 1);
        assert_eq!(candidates[0].account.account.id, backup_account.id);
        let logs = state
            .db
            .list_request_logs(10)
            .await
            .expect("list request logs");
        assert_eq!(logs.len(), 2);
        assert!(logs.iter().any(|log| log.status_code == 429));
        assert!(logs.iter().any(|log| log.status_code == 200));

        drop(state);
        remove_test_database(&database_path);
    }

    #[tokio::test]
    async fn streamed_usage_limit_retries_before_forwarding_upstream_events() {
        let limited_upstream = Router::new().route(
            "/v1/responses",
            post(|| async {
                Response::builder()
                    .status(StatusCode::OK)
                    .header(header::CONTENT_TYPE, "text/event-stream")
                    .body(Body::from(
                        "event: response.failed\ndata: {\"type\":\"response.failed\",\"response\":{\"error\":{\"code\":\"usage_limit_reached\"}}}\n\n",
                    ))
                    .expect("limited response")
            }),
        );
        let limited_listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind limited upstream");
        let limited_address = limited_listener.local_addr().expect("limited address");
        let limited_server = tokio::spawn(async move {
            axum::serve(limited_listener, limited_upstream)
                .await
                .expect("serve limited upstream");
        });

        let backup_upstream = Router::new().route(
            "/v1/responses",
            post(|| async {
                Response::builder()
                    .status(StatusCode::OK)
                    .header(header::CONTENT_TYPE, "text/event-stream")
                    .body(Body::from(
                        "event: response.completed\ndata: {\"type\":\"response.completed\",\"response\":{\"id\":\"backup-response\",\"usage\":{\"input_tokens\":3,\"output_tokens\":5}}}\n\n",
                    ))
                    .expect("backup response")
            }),
        );
        let backup_listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind backup upstream");
        let backup_address = backup_listener.local_addr().expect("backup address");
        let backup_server = tokio::spawn(async move {
            axum::serve(backup_listener, backup_upstream)
                .await
                .expect("serve backup upstream");
        });

        let database_path = test_database_path();
        let db = Db::open(&database_path).await.expect("open test database");
        let SeededRelayRoute {
            relay_secret,
            account_id: limited_account_id,
            ..
        } = seed_relay_route(
            &db,
            RelayRouteSeed {
                base_url: format!("http://{limited_address}"),
                provider: "openai-compatible",
                auth_mode: "bearer",
                provider_secret: "limited-secret".to_string(),
                wire_api: "openai-responses",
                public_model: "public-responses-model",
                upstream_model: "limited-upstream-model",
            },
        )
        .await;
        let backup_account = db
            .create_provider_account(CreateProviderAccountRequest {
                name: "backup provider".to_string(),
                provider: "openai-compatible".to_string(),
                base_url: format!("http://{backup_address}"),
                auth_mode: "bearer".to_string(),
                wire_api: "openai-responses".to_string(),
                api_key: "backup-secret".to_string(),
                is_active: true,
                max_running_requests: 0,
            })
            .await
            .expect("create backup account");
        db.create_provider_model_route(CreateProviderModelRouteRequest {
            public_model_id: "public-responses-model".to_string(),
            provider_account_id: backup_account.id,
            upstream_model_id: "backup-upstream-model".to_string(),
            wire_api: "openai-responses".to_string(),
            role: "backup".to_string(),
            enabled: true,
            weight: 100,
            strip_params: Vec::new(),
        })
        .await
        .expect("create backup route");
        let state = test_state(db, database_path.clone());

        let response = relay_openai_responses(
            State(state.clone()),
            relay_headers(&relay_secret),
            Uri::from_static("/openai/v1/responses"),
            Bytes::from_static(
                br#"{"model":"public-responses-model","input":"hello","stream":true}"#,
            ),
        )
        .await
        .expect("relay response");
        let body = to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("response body");

        limited_server.abort();
        backup_server.abort();
        let body = String::from_utf8(body.to_vec()).expect("utf8 response");
        assert!(body.contains("backup-response"));
        assert!(!body.contains("usage_limit_reached"));
        let account = state
            .db
            .get_provider_account(&limited_account_id)
            .await
            .expect("get limited account")
            .expect("limited account");
        assert_eq!(account.status, "partially_limited");

        drop(state);
        remove_test_database(&database_path);
    }

    #[tokio::test]
    async fn gemini_relay_transforms_the_wire_payload_and_records_the_attempt() {
        let seen = Arc::new(Mutex::new(None));
        let handler_seen = Arc::clone(&seen);
        let upstream = Router::new().route(
            "/v1internal:generateContent",
            post(move |headers: HeaderMap, Json(body): Json<Value>| {
                let handler_seen = Arc::clone(&handler_seen);
                async move {
                    *handler_seen.lock().await = Some((headers, body));
                    (
                        StatusCode::OK,
                        Json(json!({
                            "traceId": "gemini-1",
                            "response": {
                                "candidates": [{"content": {"parts": [{"text": "hello"}]}}],
                                "usageMetadata": {
                                    "promptTokenCount": 11,
                                    "cachedContentTokenCount": 6,
                                    "candidatesTokenCount": 7
                                }
                            }
                        })),
                    )
                }
            }),
        );
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind upstream");
        let address = listener.local_addr().expect("upstream address");
        let server = tokio::spawn(async move {
            axum::serve(listener, upstream)
                .await
                .expect("serve upstream");
        });

        let database_path = test_database_path();
        let db = Db::open(&database_path).await.expect("open test database");
        let SeededRelayRoute {
            relay_secret,
            account_id,
            route_id,
        } = seed_relay_route(&db, gemini_relay_route_seed(address)).await;
        let state = test_state(db, database_path.clone());

        let response = relay_gemini_generate_content(
            State(state.clone()),
            relay_headers(&relay_secret),
            Uri::from_static("/gemini/v1beta/models/public-gemini-model:generateContent"),
            axum::extract::Path("public-gemini-model:generateContent".to_string()),
            Bytes::from_static(br#"{"contents":[{"parts":[{"text":"hello"}]}]}"#),
        )
        .await
        .expect("relay response");
        server.abort();

        assert_eq!(response.status(), StatusCode::OK);
        let relay_body: Value = serde_json::from_slice(
            &to_bytes(response.into_body(), usize::MAX)
                .await
                .expect("read relay response"),
        )
        .expect("decode relay response");
        assert_eq!(relay_body["responseId"], "gemini-1");
        assert_eq!(relay_body["usageMetadata"]["promptTokenCount"], 11);

        let (headers, body) = seen.lock().await.take().expect("capture upstream request");
        assert_eq!(body["model"], "upstream-gemini-model");
        assert_eq!(body["project"], "test-project");
        assert_eq!(body["request"]["contents"][0]["role"], "user");
        assert_eq!(
            headers
                .get(header::AUTHORIZATION)
                .and_then(|value| value.to_str().ok()),
            Some("Bearer access-token")
        );

        let logs = state
            .db
            .list_request_logs(1)
            .await
            .expect("list request logs");
        assert_eq!(logs.len(), 1);
        assert_eq!(logs[0].status_code, 200);
        assert_eq!(logs[0].model.as_deref(), Some("public-gemini-model"));
        assert_eq!(
            logs[0].upstream_model.as_deref(),
            Some("upstream-gemini-model")
        );
        assert_eq!((logs[0].input_tokens, logs[0].output_tokens), (11, 7));
        assert_eq!(logs[0].cached_input_tokens, 6);
        assert!(logs[0].error.is_none());
        assert_eq!(
            state
                .db
                .get_provider_account(&account_id)
                .await
                .expect("get provider account")
                .expect("provider account")
                .status,
            "healthy"
        );
        assert_eq!(
            state
                .db
                .get_provider_model_route(&route_id)
                .await
                .expect("get provider route")
                .expect("provider route")
                .last_status_code,
            Some(200)
        );

        drop(state);
        remove_test_database(&database_path);
    }

    #[tokio::test]
    async fn streamed_gemini_relay_records_usage() {
        let upstream = Router::new().route(
            "/v1internal:streamGenerateContent",
            post(|| async {
                (
                    StatusCode::OK,
                    [(header::CONTENT_TYPE, "text/event-stream")],
                    concat!(
                        "data: {\"traceId\":\"gemini-stream-1\",\"response\":{",
                        "\"candidates\":[{\"content\":{\"parts\":[{\"text\":\"hello\"}]}}],",
                        "\"usageMetadata\":{\"promptTokenCount\":11,\"cachedContentTokenCount\":6,\"candidatesTokenCount\":7}}}\n\n"
                    ),
                )
            }),
        );
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind upstream");
        let address = listener.local_addr().expect("upstream address");
        let server = tokio::spawn(async move {
            axum::serve(listener, upstream)
                .await
                .expect("serve upstream");
        });

        let database_path = test_database_path();
        let db = Db::open(&database_path).await.expect("open test database");
        let SeededRelayRoute { relay_secret, .. } =
            seed_relay_route(&db, gemini_relay_route_seed(address)).await;
        let state = test_state(db, database_path.clone());

        let response = relay_gemini_generate_content(
            State(state.clone()),
            relay_headers(&relay_secret),
            Uri::from_static("/gemini/v1beta/models/public-gemini-model:streamGenerateContent"),
            axum::extract::Path("public-gemini-model:streamGenerateContent".to_string()),
            Bytes::from_static(br#"{"contents":[{"parts":[{"text":"hello"}]}]}"#),
        )
        .await
        .expect("relay response");
        assert_eq!(response.status(), StatusCode::OK);
        let relay_body = to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("read relay response");
        server.abort();

        let relay_body = String::from_utf8(relay_body.to_vec()).expect("decode relay stream");
        assert!(relay_body.contains(r#""responseId":"gemini-stream-1""#));
        assert!(relay_body.contains(r#""promptTokenCount":11"#));

        let logs = state
            .db
            .list_request_logs(1)
            .await
            .expect("list request logs");
        assert_eq!(logs.len(), 1);
        assert_eq!((logs[0].input_tokens, logs[0].output_tokens), (11, 7));
        assert_eq!(logs[0].cached_input_tokens, 6);

        drop(state);
        remove_test_database(&database_path);
    }

    #[tokio::test]
    async fn gemini_relay_preserves_upstream_errors_and_cools_the_route() {
        const UPSTREAM_ERROR: &str = r#"{"error":{"message":"provider unavailable"}}"#;

        let upstream = Router::new().route(
            "/v1internal:generateContent",
            post(|| async {
                (
                    StatusCode::SERVICE_UNAVAILABLE,
                    [(header::CONTENT_TYPE, "application/json")],
                    UPSTREAM_ERROR,
                )
            }),
        );
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind upstream");
        let address = listener.local_addr().expect("upstream address");
        let server = tokio::spawn(async move {
            axum::serve(listener, upstream)
                .await
                .expect("serve upstream");
        });

        let database_path = test_database_path();
        let db = Db::open(&database_path).await.expect("open test database");
        let SeededRelayRoute {
            relay_secret,
            route_id,
            ..
        } = seed_relay_route(&db, gemini_relay_route_seed(address)).await;
        let state = test_state(db, database_path.clone());

        let response = relay_gemini_generate_content(
            State(state.clone()),
            relay_headers(&relay_secret),
            Uri::from_static("/gemini/v1beta/models/public-gemini-model:generateContent"),
            axum::extract::Path("public-gemini-model:generateContent".to_string()),
            Bytes::from_static(br#"{"contents":[{"parts":[{"text":"hello"}]}]}"#),
        )
        .await
        .expect("relay response");
        server.abort();

        assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
        assert_eq!(
            to_bytes(response.into_body(), usize::MAX)
                .await
                .expect("read relay response")
                .as_ref(),
            UPSTREAM_ERROR.as_bytes()
        );
        let logs = state
            .db
            .list_request_logs(1)
            .await
            .expect("list request logs");
        assert_eq!(logs.len(), 1);
        assert_eq!(logs[0].status_code, 503);
        assert_eq!(logs[0].error.as_deref(), Some("upstream returned 503"));
        let route = state
            .db
            .get_provider_model_route(&route_id)
            .await
            .expect("get provider route")
            .expect("provider route");
        assert_eq!(route.status, "cooling_down");
        assert_eq!(route.last_status_code, Some(503));
        assert!(route.cooldown_until.is_some());

        drop(state);
        remove_test_database(&database_path);
    }

    #[test]
    fn strip_params_remove_only_top_level_keys_and_summarize_metadata() {
        let mut body = json!({
            "model": "public-model",
            "temperature": 0.7,
            "messages": [
                {
                    "role": "user",
                    "content": "do not persist this",
                    "temperature": 1.0
                }
            ],
            "stream": true
        });

        let stripped =
            strip_top_level_params(&mut body, &["temperature".to_string(), "top_p".to_string()]);
        let summary = build_request_summary(&body, 123, stripped);

        assert!(body.get("temperature").is_none());
        assert_eq!(body["messages"][0]["temperature"], 1.0);
        assert_eq!(summary.stripped_params, vec!["temperature"]);
        assert_eq!(summary.top_level_keys, vec!["messages", "model", "stream"]);
        assert_eq!(summary.body_bytes, 123);
        assert!(summary.stream);
    }

    #[test]
    fn official_openai_chat_streams_request_usage_without_overriding_clients() {
        let mut official = json!({"stream": true});
        enable_stream_usage_if_supported(&mut official, WireApi::OpenAiChat, "openai");
        assert_eq!(official["stream_options"]["include_usage"], true);

        let mut disabled = json!({
            "stream": true,
            "stream_options": {"include_usage": false}
        });
        enable_stream_usage_if_supported(&mut disabled, WireApi::OpenAiChat, "openai");
        assert_eq!(disabled["stream_options"]["include_usage"], false);

        let mut compatible = json!({"stream": true});
        enable_stream_usage_if_supported(&mut compatible, WireApi::OpenAiChat, "openai-compatible");
        assert!(compatible.get("stream_options").is_none());
    }

    #[test]
    fn codex_subscription_responses_strip_unsupported_max_output_tokens() {
        let mut body = json!({
            "model": "gpt-5",
            "max_output_tokens": 32_000,
            "input": "hello"
        });

        let stripped =
            strip_upstream_params(&mut body, &[], WireApi::OpenAiResponses, "codex-oauth");

        assert!(body.get("max_output_tokens").is_none());
        assert_eq!(stripped, vec!["max_output_tokens"]);
    }

    #[test]
    fn codex_subscription_responses_preserve_service_tier() {
        let mut body = json!({
            "model": "gpt-6.1-sol",
            "service_tier": "priority",
            "input": "hello"
        });

        let stripped =
            strip_upstream_params(&mut body, &[], WireApi::OpenAiResponses, "codex-oauth");

        assert_eq!(body["service_tier"], "priority");
        assert!(stripped.is_empty());
    }

    #[test]
    fn api_key_responses_preserve_max_output_tokens() {
        let mut body = json!({
            "model": "gpt-5",
            "max_output_tokens": 32_000,
            "input": "hello"
        });

        let stripped = strip_upstream_params(&mut body, &[], WireApi::OpenAiResponses, "bearer");

        assert_eq!(body["max_output_tokens"], 32_000);
        assert!(stripped.is_empty());
    }

    #[test]
    fn codex_normalization_does_not_duplicate_configured_strip_params() {
        let mut body = json!({
            "model": "gpt-5",
            "max_output_tokens": 32_000,
            "input": "hello"
        });

        let stripped = strip_upstream_params(
            &mut body,
            &["max_output_tokens".to_string()],
            WireApi::OpenAiResponses,
            "codex-oauth",
        );

        assert!(body.get("max_output_tokens").is_none());
        assert_eq!(stripped, vec!["max_output_tokens"]);
    }

    #[test]
    fn sanitize_upstream_url_removes_query() {
        assert_eq!(
            crate::relay_attempt::sanitize_upstream_url(
                "https://api.example.com/v1/messages?token=secret"
            ),
            "https://api.example.com/v1/messages"
        );
    }

    #[test]
    fn gemini_model_operation_parses_generate_methods() {
        let (model, method) =
            parse_gemini_model_operation("gemini-3.5-flash:generateContent").unwrap();
        assert_eq!(model, "gemini-3.5-flash");
        assert_eq!(method, GeminiMethod::GenerateContent);

        let (model, method) =
            parse_gemini_model_operation("gemini-3.5-flash:streamGenerateContent").unwrap();
        assert_eq!(model, "gemini-3.5-flash");
        assert_eq!(method, GeminiMethod::StreamGenerateContent);
        assert!(parse_gemini_model_operation("gemini-3.5-flash:countTokens").is_err());
    }

    #[test]
    fn gemini_code_assist_upstream_url_targets_internal_endpoint() {
        assert_eq!(
            gemini_code_assist_upstream_url(
                "https://cloudcode-pa.googleapis.com",
                GeminiMethod::StreamGenerateContent,
                Some("key=tokentoxication-secret&alt=json&trace=1")
            ),
            "https://cloudcode-pa.googleapis.com/v1internal:streamGenerateContent?alt=sse&trace=1"
        );

        assert_eq!(
            gemini_code_assist_upstream_url(
                "https://cloudcode-pa.googleapis.com/v1internal",
                GeminiMethod::GenerateContent,
                None
            ),
            "https://cloudcode-pa.googleapis.com/v1internal:generateContent"
        );
    }

    #[test]
    fn gemini_code_assist_sse_event_unwraps_response() {
        let event = transform_code_assist_sse_event(
            r#"data: {"traceId":"trace-1","response":{"candidates":[{"content":{"parts":[{"text":"hi"}]}}]}}"#,
        );

        assert!(event.starts_with("data: {"));
        assert!(event.contains(r#""responseId":"trace-1""#));
        assert!(event.contains(r#""text":"hi""#));
        assert!(!event.contains(r#""response":{"#));
    }

    #[test]
    fn gemini_generate_content_requires_contents() {
        assert!(
            validate_gemini_generate_content_request(&json!({
                "contents": [{"parts": [{"text": "hello"}]}]
            }))
            .is_ok()
        );
        assert!(validate_gemini_generate_content_request(&json!({"input": "hello"})).is_err());
    }

    #[test]
    fn parse_usage_reads_gemini_usage_metadata() {
        let body = serde_json::to_vec(&json!({
            "usageMetadata": {
                "promptTokenCount": 11,
                "cachedContentTokenCount": 6,
                "candidatesTokenCount": 7
            }
        }))
        .unwrap();

        assert_eq!(
            parse_usage(&body),
            TokenUsage {
                input_tokens: 11,
                cached_input_tokens: 6,
                output_tokens: 7,
                generation_ms: None,
            }
        );
    }

    #[test]
    fn parse_usage_handles_uncached_and_invalid_payloads() {
        let uncached = serde_json::to_vec(&json!({
            "usage": {"input_tokens": 9, "output_tokens": 4}
        }))
        .unwrap();
        assert_eq!(
            parse_usage(&uncached),
            TokenUsage {
                input_tokens: 9,
                cached_input_tokens: 0,
                output_tokens: 4,
                generation_ms: None,
            }
        );
        assert_eq!(parse_usage(b"not json"), TokenUsage::default());
        assert_eq!(parse_usage(br#"{"id":"no-usage"}"#), TokenUsage::default());
    }

    #[test]
    fn streaming_usage_merges_anthropic_events_across_chunks() {
        let mut usage = StreamingResponseObservation::default();
        for chunk in [
            &b"event: message_start\ndata: {\"type\":\"message_start\",\"message\":{\"usage\":{\"input_tokens\":17,\"cache_read_input_tokens\":100,\"cache_creation_input_tokens\":3,\"output_tokens\":1}}}\n\n"[..],
            &b"event: message_delta\ndata: {\"type\":\"message_delta\",\"usage\":{\"out"[..],
            &b"put_tokens\":9}}\n\n"[..],
        ] {
            usage.observe(chunk, Instant::now());
        }

        assert_eq!(
            usage.tokens(),
            TokenUsage {
                input_tokens: 120,
                cached_input_tokens: 100,
                output_tokens: 9,
                generation_ms: None,
            }
        );
    }

    #[test]
    fn parse_usage_reads_nested_responses_usage() {
        let body = serde_json::to_vec(&json!({
            "type": "response.completed",
            "response": {
                "usage": {
                    "input_tokens": 13,
                    "input_tokens_details": {"cached_tokens": 10},
                    "output_tokens": 8,
                    "total_tokens": 21
                }
            }
        }))
        .unwrap();

        assert_eq!(
            parse_usage(&body),
            TokenUsage {
                input_tokens: 13,
                cached_input_tokens: 10,
                output_tokens: 8,
                generation_ms: None,
            }
        );
    }

    #[test]
    fn streaming_observation_tracks_openai_responses_failure() {
        let mut observation = StreamingResponseObservation::default();
        observation.observe(
            b"event: response.failed\ndata: {\"type\":\"response.failed\",\"response\":{\"status\":\"failed\",\"error\":{\"code\":\"server_error\",\"message\":\"temporary upstream failure\"}}}\n\n",
            Instant::now(),
        );

        let failure = observation
            .application_failure(WireApi::OpenAiResponses)
            .expect("application failure");
        assert_eq!(failure.code.as_deref(), Some("server_error"));
        assert_eq!(failure.log_error(), "upstream stream failed (server_error)");
    }

    #[test]
    fn output_delta_marks_the_generation_boundary() {
        assert!(!is_output_delta(&json!({
            "type": "response.created"
        })));
        assert!(is_output_delta(&json!({
            "type": "response.output_text.delta",
            "delta": "token"
        })));
        assert!(is_output_delta(&json!({
            "type": "content_block_delta",
            "delta": {"text": "token"}
        })));
        assert!(!is_output_delta(&json!({
            "choices": [{"delta": {"role": "assistant", "content": ""}}]
        })));
        assert!(is_output_delta(&json!({
            "choices": [{"delta": {"content": "token"}}]
        })));
    }

    #[test]
    fn streaming_failure_after_response_created_is_replayable_without_output() {
        let mut observation = StreamingResponseObservation::default();
        observation.observe(
            b"event: response.created\ndata: {\"type\":\"response.created\",\"response\":{\"id\":\"resp-1\"}}\n\nevent: response.failed\ndata: {\"type\":\"response.failed\",\"response\":{\"id\":\"resp-1\",\"error\":{\"code\":\"usage_limit_reached\"}}}\n\n",
            Instant::now(),
        );

        assert!(
            observation
                .application_failure(WireApi::OpenAiResponses)
                .is_some()
        );
        assert_eq!(
            observation.replayable_application_failure(WireApi::OpenAiResponses),
            observation.application_failure(WireApi::OpenAiResponses)
        );
        assert!(!observation.output_started);
    }

    #[test]
    fn streaming_failure_after_reasoning_output_is_not_replayable() {
        let mut observation = StreamingResponseObservation::default();
        observation.observe(
            b"event: response.created\ndata: {\"type\":\"response.created\",\"response\":{\"id\":\"resp-1\"}}\n\nevent: response.reasoning_summary_text.delta\ndata: {\"type\":\"response.reasoning_summary_text.delta\",\"delta\":\"thinking\"}\n\nevent: response.failed\ndata: {\"type\":\"response.failed\",\"response\":{\"id\":\"resp-1\",\"error\":{\"code\":\"server_error\"}}}\n\n",
            Instant::now(),
        );

        assert!(observation.output_started);
        assert_eq!(
            observation.replayable_application_failure(WireApi::OpenAiResponses),
            None
        );
    }

    #[test]
    fn streaming_failure_preserves_bounded_retry_after() {
        let failure = openai_responses_stream_failure(&json!({
            "type": "error",
            "error": {
                "code": "server_is_overloaded",
                "headers": {"Retry-After": "7"}
            }
        }))
        .expect("application failure");

        assert_eq!(failure.retry_after, Some(Duration::from_secs(7)));
    }

    #[test]
    fn streaming_failure_logs_exclude_provider_messages_and_unsafe_codes() {
        let failure = openai_responses_stream_failure(&json!({
            "type": "error",
            "code": "unsafe code with spaces",
            "message": "prompt text or credentials must not reach request logs"
        }))
        .expect("application failure");

        assert_eq!(failure.code, None);
        assert_eq!(failure.log_error(), "upstream stream failed");
    }

    #[test]
    fn streaming_application_failures_are_responses_only() {
        let mut observation = StreamingResponseObservation::default();
        observation.observe(
            b"event: error\ndata: {\"type\":\"error\",\"code\":\"server_error\",\"message\":\"temporary upstream failure\"}\n\n",
            Instant::now(),
        );

        assert!(
            observation
                .application_failure(WireApi::OpenAiResponses)
                .is_some()
        );
        for wire_api in [
            WireApi::OpenAiChat,
            WireApi::AnthropicMessages,
            WireApi::GeminiGenerateContent,
        ] {
            assert_eq!(observation.application_failure(wire_api), None);
        }
    }

    #[test]
    fn responses_continuations_are_not_replay_safe() {
        assert!(request_is_replay_safe(
            WireApi::OpenAiResponses,
            &json!({"model": "gpt", "input": "hello"})
        ));
        assert!(request_is_replay_safe(
            WireApi::OpenAiResponses,
            &json!({"model": "gpt", "input": "hello", "previous_response_id": null})
        ));
        assert!(!request_is_replay_safe(
            WireApi::OpenAiResponses,
            &json!({
                "model": "gpt",
                "input": "continue",
                "previous_response_id": "resp_123"
            })
        ));
        assert!(request_is_replay_safe(
            WireApi::OpenAiChat,
            &json!({"model": "gpt", "messages": []})
        ));
    }
}
