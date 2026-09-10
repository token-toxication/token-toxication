use super::*;
use crate::websocket_transport::{self, ConnectError};
use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use futures_util::{SinkExt, StreamExt};
use std::collections::HashSet;
use tokio::time::{Instant, timeout};
use tokio_tungstenite::tungstenite;

const LITE_METADATA: &str = "ws_request_header_x_openai_internal_codex_responses_lite";
const MAX_RESPONSES: usize = 1024;

pub async fn relay_responses_websocket(
    State(state): State<AppState>,
    headers: HeaderMap,
    uri: Uri,
    upgrade: WebSocketUpgrade,
) -> Result<Response, AppError> {
    // Authenticate before accepting the upgrade. Query-string credentials are
    // deliberately not accepted for a long-lived connection.
    authenticate_relay_api_key(&state, &headers, None).await?;
    validate_responses_protocol(&headers, &json!({}))?;
    if uri.query().is_some() {
        return Err(AppError::BadRequest(
            "WebSocket query parameters are not supported".into(),
        ));
    }
    Ok(upgrade
        .max_message_size(crate::RELAY_BODY_LIMIT_BYTES)
        .max_frame_size(crate::RELAY_BODY_LIMIT_BYTES)
        .on_upgrade(move |socket| serve(socket, state, headers)))
}

struct ActiveResponse {
    attempt: RelayAttempt,
    log: RelayAttemptLog,
    usage: UsageUpdate,
    response_id: Option<String>,
}

#[derive(Clone, Copy)]
enum Stop {
    Client,
    Shutdown,
    Timeout,
    Upstream,
    Protocol,
    Http(StatusCode),
    Authorization(StatusCode),
    Forwarded,
}

impl Stop {
    fn message(self) -> &'static str {
        match self {
            Self::Client => "client disconnected",
            Self::Shutdown => "server shutting down",
            Self::Timeout => "WebSocket relay timeout",
            Self::Upstream => "upstream WebSocket failed",
            Self::Protocol => "invalid WebSocket continuation or event",
            Self::Http(_) => "upstream WebSocket handshake rejected",
            Self::Authorization(_) => "upstream authorization failed",
            Self::Forwarded => "upstream response failed",
        }
    }

    fn status(self) -> StatusCode {
        match self {
            Self::Http(status) | Self::Authorization(status) => status,
            Self::Protocol => StatusCode::BAD_REQUEST,
            Self::Shutdown => StatusCode::SERVICE_UNAVAILABLE,
            Self::Timeout => StatusCode::GATEWAY_TIMEOUT,
            _ => StatusCode::BAD_GATEWAY,
        }
    }

    fn code(self) -> &'static str {
        match self {
            Self::Http(_) => "upstream_handshake_failed",
            Self::Authorization(_) => "upstream_authorization_failed",
            Self::Protocol => "invalid_websocket_request",
            Self::Shutdown => "relay_shutting_down",
            Self::Timeout => "relay_timeout",
            _ => "upstream_websocket_failed",
        }
    }
}

async fn serve(mut client: WebSocket, state: AppState, headers: HeaderMap) {
    let mut active = None;
    let mut shutdown = state.shutdown.subscribe();
    let stop = if *shutdown.borrow() {
        Stop::Shutdown
    } else {
        tokio::select! {
            _ = shutdown.changed() => Stop::Shutdown,
            result = timeout(state.relay_stream_max_duration,
                relay_connection(&mut client, &state, &headers, &mut active)) => {
                result.unwrap_or(Stop::Timeout)
            }
        }
    };
    if let Some(active) = active {
        let usage = active.usage.tokens();
        let result = match stop {
            Stop::Client => {
                active
                    .attempt
                    .record_client_disconnect(&active.log, usage)
                    .await
            }
            Stop::Shutdown => {
                active
                    .attempt
                    .record_server_shutdown(&active.log, usage)
                    .await
            }
            Stop::Protocol => {
                active
                    .attempt
                    .record_application_failure(
                        &active.log,
                        StatusCode::BAD_REQUEST,
                        usage,
                        stop.message().into(),
                    )
                    .await
            }
            Stop::Http(status) | Stop::Authorization(status) => {
                active
                    .attempt
                    .record_response(&active.log, status, &HeaderMap::new(), b"{}", usage)
                    .await
            }
            Stop::Timeout | Stop::Upstream | Stop::Forwarded => {
                active
                    .attempt
                    .record_failure_with_usage(
                        &active.log,
                        classify_transport_failure(stop.message().into(), Utc::now()),
                        usage,
                    )
                    .await
            }
        };
        if result.is_err() {
            tracing::warn!("failed to record WebSocket relay termination");
        }
    }
    if !matches!(stop, Stop::Client | Stop::Forwarded) {
        let error = json!({"type":"error", "status":stop.status().as_u16(), "error":{"type":"relay_error", "code":stop.code(), "message":stop.message()}});
        let _ = timeout(
            Duration::from_secs(1),
            client.send(Message::Text(error.to_string().into())),
        )
        .await;
    }
    let _ = timeout(Duration::from_secs(1), client.close()).await;
}

fn prepare_event(
    value: &mut Value,
    headers: &HeaderMap,
    model: &str,
    previous: &HashSet<String>,
) -> Result<(), Stop> {
    if value.get("type").and_then(Value::as_str) != Some("response.create")
        || value.get("model").and_then(Value::as_str) != Some(model)
        || validate_responses_request(value).is_err()
    {
        return Err(Stop::Protocol);
    }
    if let Some(id) = value.get("previous_response_id").filter(|id| !id.is_null())
        && !id.as_str().is_some_and(|id| previous.contains(id))
    {
        return Err(Stop::Protocol);
    }
    let mut protocol_headers = HeaderMap::new();
    if let Some(value) = headers.get(RESPONSES_LITE_HEADER) {
        protocol_headers.insert(RESPONSES_LITE_HEADER, value.clone());
    }
    if let Some(metadata) = value
        .get_mut("client_metadata")
        .and_then(Value::as_object_mut)
    {
        if let Some(lite) = metadata.get(LITE_METADATA) {
            if lite.as_str() != Some("true") {
                return Err(Stop::Protocol);
            }
            protocol_headers.insert(RESPONSES_LITE_HEADER, HeaderValue::from_static("true"));
        }
        // Request-scoped protocol headers are encoded in WS metadata. Do not
        // allow the client to smuggle authentication or arbitrary internal headers.
        metadata.retain(|key, _| !key.starts_with("ws_request_header_") || key == LITE_METADATA);
    }
    validate_responses_protocol(&protocol_headers, value).map_err(|_| Stop::Protocol)
}

async fn relay_connection(
    client: &mut WebSocket,
    state: &AppState,
    headers: &HeaderMap,
    active: &mut Option<ActiveResponse>,
) -> Stop {
    let idle = state.relay_stream_idle_timeout;
    let first = match timeout(idle, client.next()).await {
        Ok(Some(Ok(Message::Text(text)))) => text,
        Err(_) => return Stop::Timeout,
        _ => return Stop::Client,
    };
    let mut first: Value = match serde_json::from_str(&first) {
        Ok(value) => value,
        Err(_) => return Stop::Protocol,
    };
    let Some(model) = first
        .get("model")
        .and_then(Value::as_str)
        .map(str::to_owned)
    else {
        return Stop::Protocol;
    };
    let mut previous = HashSet::new();
    if prepare_event(&mut first, headers, &model, &previous).is_err() {
        return Stop::Protocol;
    }
    let authenticated = match RelayAttempt::authenticate(state, headers, None).await {
        Ok(value) => value,
        Err(_) => return Stop::Client,
    };
    let binding = match authenticated.select("openai-responses", &model).await {
        Ok(value) => value,
        Err(_) => return Stop::Protocol,
    };
    let account = &binding.selection().account;
    let base_endpoint = upstream_url(&account.account.base_url, "/v1/responses");
    *active = Some(start_response(&binding, &mut first, &base_endpoint));
    let auth = if is_codex_subscription_auth(&account.account.auth_mode) {
        match codex_subscription_authorization(&state.db, &state.http, account).await {
            Ok(auth) => Some(auth),
            Err(error) => return Stop::Authorization(error.status()),
        }
    } else {
        None
    };
    let endpoint = auth
        .as_ref()
        .map(|auth| auth.endpoint.clone())
        .unwrap_or_else(|| upstream_url(&account.account.base_url, "/v1/responses"));
    let url = match url::Url::parse(&endpoint) {
        Ok(url) => url,
        Err(_) => return Stop::Upstream,
    };
    if !url.username().is_empty() || url.password().is_some() {
        return Stop::Upstream;
    }
    if !matches!(url.scheme(), "http" | "https") {
        return Stop::Upstream;
    }
    let mut upstream_headers = HeaderMap::new();
    let secret = auth
        .as_ref()
        .map(|auth| auth.access_token.as_str())
        .unwrap_or(&account.api_key);
    let auth_header = if auth.is_some() || account.account.auth_mode == "bearer" {
        (header::AUTHORIZATION, format!("Bearer {secret}"))
    } else {
        (
            HeaderName::from_static(if account.account.auth_mode == "x-goog-api-key" {
                "x-goog-api-key"
            } else {
                "x-api-key"
            }),
            secret.to_owned(),
        )
    };
    let Ok(value) = HeaderValue::from_str(&auth_header.1) else {
        return Stop::Upstream;
    };
    upstream_headers.insert(auth_header.0, value);
    if let Some(auth) = &auth {
        if let Some(account_id) = &auth.account_id {
            let Ok(value) = HeaderValue::from_str(account_id) else {
                return Stop::Upstream;
            };
            upstream_headers.insert("chatgpt-account-id", value);
        }
        upstream_headers.insert("originator", HeaderValue::from_static("opencode"));
    }
    upstream_headers.insert(
        "openai-beta",
        HeaderValue::from_static("responses_websockets=2026-02-06"),
    );
    if let Some(lite) = headers.get(RESPONSES_LITE_HEADER) {
        upstream_headers.insert(RESPONSES_LITE_HEADER, lite.clone());
    }
    // The model header must agree with routing, never the client's public alias.
    if let Ok(model) = HeaderValue::from_str(&binding.selection().upstream_model_id) {
        upstream_headers.insert("openai-model", model);
    }
    if let Some(current) = active.as_mut() {
        current.log.upstream_url = Some(endpoint.clone());
    }
    let mut upstream = match timeout(
        idle,
        websocket_transport::connect(&state.websocket_http, &endpoint, upstream_headers),
    )
    .await
    {
        Ok(Ok(socket)) => socket,
        Err(_) => return Stop::Timeout,
        Ok(Err(ConnectError::Http { status, headers })) => {
            if let Some(current) = active.take() {
                let _ = current
                    .attempt
                    .record_response(&current.log, status, &headers, b"{}", TokenUsage::default())
                    .await;
            }
            return Stop::Http(status);
        }
        _ => return Stop::Upstream,
    };
    if !matches!(
        timeout(
            idle,
            upstream.send(tungstenite::Message::Text(first.to_string().into()))
        )
        .await,
        Ok(Ok(()))
    ) {
        return Stop::Upstream;
    }
    let mut idle_deadline = Instant::now() + idle;
    loop {
        tokio::select! {
            _ = tokio::time::sleep_until(idle_deadline) => return Stop::Timeout,
            message = client.next() => {
                match message {
                    Some(Ok(Message::Text(text))) => {
                        if active.is_some() || previous.len() >= MAX_RESPONSES { return Stop::Protocol; }
                        if authenticate_relay_api_key(state, headers, None).await.is_err() { return Stop::Client; }
                        let mut value: Value = match serde_json::from_str(&text) { Ok(value) => value, Err(_) => return Stop::Protocol };
                        if prepare_event(&mut value, headers, &model, &previous).is_err() { return Stop::Protocol; }
                        *active = Some(start_response(&binding, &mut value, &endpoint));
                        if !matches!(timeout(idle, upstream.send(tungstenite::Message::Text(value.to_string().into()))).await, Ok(Ok(()))) { return Stop::Upstream; }
                    }
                    Some(Ok(Message::Ping(data))) => {
                        if !matches!(timeout(idle, client.send(Message::Pong(data))).await, Ok(Ok(()))) { return Stop::Client; }
                    }
                    Some(Ok(Message::Pong(_))) => {}
                    Some(Ok(Message::Binary(_))) => return Stop::Protocol,
                    _ => return Stop::Client,
                }
                idle_deadline = Instant::now() + idle;
            }
            message = upstream.next() => {
                match message {
                    Some(Ok(tungstenite::Message::Text(text))) => {
                        let value: Value = match serde_json::from_str(&text) { Ok(value) => value, Err(_) => return Stop::Upstream };
                        let kind = value.get("type").and_then(Value::as_str).unwrap_or("");
                        let duplicate = value.pointer("/response/id").and_then(Value::as_str).is_some_and(|id| previous.contains(id));
                        if duplicate { continue; }
                        if let Some(current) = active.as_mut() {
                            current.usage.merge(usage_update(&value));
                            if let Some(id) = value.pointer("/response/id").and_then(Value::as_str) {
                                if id.is_empty() || id.len() > 512 { return Stop::Upstream; }
                                if current.response_id.as_deref().is_some_and(|expected| expected != id) { return Stop::Upstream; }
                                current.response_id = Some(id.to_owned());
                            }
                        }
                        if matches!(kind, "response.completed" | "response.failed" | "response.incomplete" | "error") && !duplicate
                            && let Some(current) = active.take() {
                                let wrapped_status = (kind == "error").then(|| value.get("status").or_else(|| value.get("status_code")))
                                    .flatten().and_then(Value::as_u64).and_then(|status| u16::try_from(status).ok())
                                    .and_then(|status| StatusCode::from_u16(status).ok())
                                    .filter(|status| status.is_client_error() || status.is_server_error());
                                let result = if let Some(status) = wrapped_status {
                                    current.attempt.record_response(&current.log, status, &HeaderMap::new(), b"{}", current.usage.tokens()).await
                                } else if let Some(failure) = openai_responses_stream_failure(&value) {
                                    let error = failure.log_error();
                                    if let Some(failure) = classify_upstream_application_failure(failure.code.as_deref(), error.clone(), Utc::now()) {
                                        current.attempt.record_failure_with_usage(&current.log, failure, current.usage.tokens()).await
                                    } else {
                                        current.attempt.record_application_failure(&current.log, StatusCode::BAD_GATEWAY, current.usage.tokens(), error).await
                                    }
                                } else if kind == "response.incomplete" {
                                    current.attempt.record_application_failure(&current.log, StatusCode::BAD_GATEWAY, current.usage.tokens(), "upstream response incomplete".into()).await
                                } else {
                                    current.attempt.record_response(&current.log, StatusCode::OK, &HeaderMap::new(), b"{}", current.usage.tokens()).await
                                };
                                if result.is_err() { return Stop::Upstream; }
                                if let Some(id) = value.pointer("/response/id").and_then(Value::as_str) { previous.insert(id.to_owned()); }
                        }
                        if !matches!(timeout(idle, client.send(Message::Text(text.to_string().into()))).await, Ok(Ok(()))) { return Stop::Client; }
                        if matches!(kind, "error" | "response.failed" | "response.incomplete") { return Stop::Forwarded; }
                    }
                    Some(Ok(tungstenite::Message::Ping(data))) => {
                        if !matches!(timeout(idle, upstream.send(tungstenite::Message::Pong(data))).await, Ok(Ok(()))) { return Stop::Upstream; }
                    }
                    Some(Ok(tungstenite::Message::Pong(_))) => {}
                    _ => return Stop::Upstream,
                }
                idle_deadline = Instant::now() + idle;
            }
        }
    }
}

fn start_response(binding: &RelayAttempt, value: &mut Value, endpoint: &str) -> ActiveResponse {
    value["model"] = json!(binding.selection().upstream_model_id);
    let stripped = strip_upstream_params(
        value,
        &binding.selection().strip_params,
        WireApi::OpenAiResponses,
        &binding.selection().account.account.auth_mode,
    );
    ActiveResponse {
        attempt: binding.continuation(),
        log: RelayAttemptLog {
            path: "/openai/v1/responses".into(),
            upstream_url: Some(endpoint.into()),
            request_summary: Some(build_request_summary(
                value,
                value.to_string().len() as u64,
                stripped,
            )),
        },
        usage: UsageUpdate::default(),
        response_id: None,
    }
}
