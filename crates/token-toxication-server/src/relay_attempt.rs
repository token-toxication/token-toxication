use std::borrow::Cow;
use std::collections::HashSet;
use std::time::{Duration, Instant};

use aioduct::{
    RequestBuilderSend, Response,
    runtime::{ConnectorSend, RuntimePoll},
};
use axum::http::{HeaderMap, StatusCode, Uri};
use chrono::Utc;
use uuid::Uuid;

use crate::{
    AppState,
    account_concurrency::AccountPermit,
    auth::extract_api_key,
    db::ProviderRouteSelection,
    error::AppError,
    models::{ApiKeyRecord, RequestLog, RequestSummary},
    routing::{
        RouteFailure, RoutingScope, classify_response_failure, classify_transport_failure,
        select_route,
    },
    session_affinity::SessionAffinity,
};

pub(crate) const HTTP_TRANSPORT: &str = "http";
pub(crate) const HTTP_SSE_TRANSPORT: &str = "http_sse";
pub(crate) const WEBSOCKET_TRANSPORT: &str = "websocket";

#[derive(Clone)]
pub(crate) struct RelayAttemptLog {
    pub request_id: String,
    pub path: String,
    pub transport: &'static str,
    pub upstream_url: Option<String>,
    pub request_summary: Option<RequestSummary>,
    pub overload_retry_count: u8,
    pub overload_retry_started: bool,
    pub overload_retry_exhausted: bool,
    pub overload_retry_skip_reason: Option<String>,
    pub client_retry_advice_injected: bool,
    pub attempt_index: u8,
    pub output_started: bool,
    pub retry_after_ms: Option<u64>,
    pub ttft_ms: Option<u64>,
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub(crate) struct TokenUsage {
    pub input_tokens: u64,
    pub cached_input_tokens: u64,
    pub output_tokens: u64,
    /// Time spent generating output after the first output delta was emitted.
    pub generation_ms: Option<u64>,
}

const CLIENT_CLOSED_REQUEST_STATUS: u16 = 499;

pub(crate) struct RelayAttempt {
    state: AppState,
    api_key_id: String,
    selection: ProviderRouteSelection,
    /// The model id the client asked for, which is not always the id of the
    /// route that served it: a Codex `-fast` variant is an alias for its base
    /// model. Request logs keep the client-facing id so Fast traffic stays
    /// distinguishable from ordinary traffic.
    requested_model: String,
    /// When the running-request slot was acquired; latency excludes queueing.
    started: Instant,
    queue_wait: Duration,
    /// Running-request slot on the selected account, released on drop.
    permit: Option<AccountPermit>,
}

pub(crate) struct AuthenticatedRelayAttempt {
    state: AppState,
    api_key_id: String,
}

/// Error returned when the relay shuts down while a request is queued.
pub(crate) fn shutting_down_error() -> AppError {
    AppError::ServiceUnavailable("relay is shutting down".into())
}

impl RelayAttempt {
    pub(crate) async fn authenticate(
        state: &AppState,
        headers: &HeaderMap,
        query: Option<&str>,
    ) -> Result<AuthenticatedRelayAttempt, AppError> {
        let api_key_id = authenticate_relay_api_key(state, headers, query)
            .await?
            .view
            .id;

        Ok(AuthenticatedRelayAttempt {
            state: state.clone(),
            api_key_id,
        })
    }

    pub(crate) fn selection(&self) -> &ProviderRouteSelection {
        &self.selection
    }

    pub(crate) fn elapsed_ms(&self) -> u64 {
        self.started.elapsed().as_millis() as u64
    }

    /// Waits in FIFO order for a running-request slot on the bound account.
    /// Dropping the returned future leaves the queue; shutdown aborts the wait.
    pub(crate) async fn acquire_permit(&self) -> Result<QueuedPermit, AppError> {
        acquire_until_shutdown(&self.state, &self.selection).await
    }

    pub(crate) fn continuation(&self, permit: QueuedPermit) -> Self {
        Self {
            state: self.state.clone(),
            api_key_id: self.api_key_id.clone(),
            selection: self.selection.clone(),
            requested_model: self.requested_model.clone(),
            started: Instant::now(),
            queue_wait: permit.waited,
            permit: Some(permit.permit),
        }
    }

    pub(crate) fn queue_wait(&self) -> Duration {
        self.queue_wait
    }

    /// Hands the slot to a continuation without re-queueing.
    pub(crate) fn take_queued_permit(&mut self) -> Option<QueuedPermit> {
        let waited = self.queue_wait;
        self.permit
            .take()
            .map(|permit| QueuedPermit { permit, waited })
    }

    pub(crate) async fn record_failure(
        &self,
        log: &RelayAttemptLog,
        failure: RouteFailure,
    ) -> Result<(), AppError> {
        self.record_failure_with_usage(log, failure, TokenUsage::default())
            .await
    }

    pub(crate) async fn record_failure_with_usage(
        &self,
        log: &RelayAttemptLog,
        failure: RouteFailure,
        usage: TokenUsage,
    ) -> Result<(), AppError> {
        record_route_failure_state(
            &self.state,
            &self.selection.account.account.id,
            &self.selection.route_id,
            &self.selection.upstream_model_id,
            &failure,
        )
        .await?;
        self.insert_request_log(
            log,
            failure
                .status_code
                .unwrap_or(StatusCode::BAD_GATEWAY.as_u16()),
            usage,
            Some(failure.error),
        )
        .await
    }

    pub(crate) async fn record_application_failure(
        &self,
        log: &RelayAttemptLog,
        status: StatusCode,
        usage: TokenUsage,
        error: String,
    ) -> Result<(), AppError> {
        self.insert_request_log(log, status.as_u16(), usage, Some(error))
            .await
    }

    pub(crate) async fn send<'request, R, C>(
        &self,
        request: RequestBuilderSend<'request, R, C>,
        log: &RelayAttemptLog,
    ) -> Result<Response, AppError>
    where
        R: RuntimePoll,
        C: ConnectorSend,
    {
        match self.send_unrecorded(request).await {
            Ok(response) => Ok(response),
            Err(error) => {
                self.record_failure(
                    log,
                    classify_transport_failure(error.to_string(), Utc::now()),
                )
                .await?;
                Err(AppError::Upstream(error))
            }
        }
    }

    pub(crate) async fn send_unrecorded<'request, R, C>(
        &self,
        request: RequestBuilderSend<'request, R, C>,
    ) -> Result<Response, aioduct::Error>
    where
        R: RuntimePoll,
        C: ConnectorSend,
    {
        request.send().await.map_err(Into::into)
    }

    pub(crate) async fn record_response(
        &self,
        log: &RelayAttemptLog,
        status: StatusCode,
        headers: &HeaderMap,
        body: &[u8],
        usage: TokenUsage,
    ) -> Result<(), AppError> {
        let error =
            record_upstream_response_result(&self.state, &self.selection, status, headers, body)
                .await?;
        self.insert_request_log(log, status.as_u16(), usage, error)
            .await?;
        Ok(())
    }

    pub(crate) async fn record_client_disconnect(
        &self,
        log: &RelayAttemptLog,
        usage: TokenUsage,
    ) -> Result<(), AppError> {
        self.insert_request_log(
            log,
            CLIENT_CLOSED_REQUEST_STATUS,
            usage,
            Some("client disconnected before the upstream stream completed".to_string()),
        )
        .await
    }

    pub(crate) async fn record_server_shutdown(
        &self,
        log: &RelayAttemptLog,
        usage: TokenUsage,
    ) -> Result<(), AppError> {
        self.insert_request_log(
            log,
            CLIENT_CLOSED_REQUEST_STATUS,
            usage,
            Some("server shut down before the upstream stream completed".to_string()),
        )
        .await
    }

    async fn insert_request_log(
        &self,
        log: &RelayAttemptLog,
        status_code: u16,
        usage: TokenUsage,
        error: Option<String>,
    ) -> Result<(), AppError> {
        let outcome = crate::relay_metrics::request_outcome(status_code, error.as_deref());
        self.state
            .relay_metrics
            .record_request(log.transport, status_code, error.as_deref());
        if status_code >= 400 {
            tracing::warn!(
                request_id = %log.request_id,
                transport = log.transport,
                status_code,
                outcome,
                error_present = error.is_some(),
                overload_retry_count = log.overload_retry_count,
                "relay request completed with non-success status"
            );
        }
        self.state
            .db
            .insert_request_log(RequestLog {
                id: Uuid::new_v4().to_string(),
                api_key_id: self.api_key_id.clone(),
                provider_account_id: Some(self.selection.account.account.id.clone()),
                method: "POST".to_string(),
                path: log.path.clone(),
                model: Some(self.requested_model.clone()),
                upstream_model: Some(self.selection.upstream_model_id.clone()),
                upstream_url: log.upstream_url.as_deref().map(sanitize_upstream_url),
                request_summary: log.request_summary.clone(),
                status_code,
                latency_ms: self.started.elapsed().as_millis() as u64,
                ttft_ms: log.ttft_ms,
                queue_wait_ms: self.queue_wait.as_millis() as u64,
                overload_retry_count: log.overload_retry_count,
                overload_retry_started: log.overload_retry_started,
                overload_retry_exhausted: log.overload_retry_exhausted,
                overload_retry_skip_reason: log.overload_retry_skip_reason.clone(),
                client_retry_advice_injected: log.client_retry_advice_injected,
                input_tokens: usage.input_tokens,
                cached_input_tokens: usage.cached_input_tokens,
                output_tokens: usage.output_tokens,
                otps: output_tokens_per_second(usage.output_tokens, usage.generation_ms),
                cost_usd: 0.0,
                created_at: Utc::now(),
                error,
                attempt_index: log.attempt_index,
                output_started: log.output_started,
                retry_after_ms: log.retry_after_ms,
                logical_request_id: log.request_id.clone(),
            })
            .await?;
        Ok(())
    }
}

fn output_tokens_per_second(output_tokens: u64, generation_ms: Option<u64>) -> Option<f64> {
    let elapsed_seconds = generation_ms? as f64 / 1000.0;
    (output_tokens > 0 && elapsed_seconds > 0.0).then(|| output_tokens as f64 / elapsed_seconds)
}

impl AuthenticatedRelayAttempt {
    pub(crate) async fn select(
        &self,
        wire_api: &str,
        model: &str,
        affinity: Option<&SessionAffinity>,
    ) -> Result<RelayAttempt, AppError> {
        self.select_excluding(wire_api, model, affinity, &HashSet::new())
            .await
    }

    pub(crate) async fn select_excluding(
        &self,
        wire_api: &str,
        model: &str,
        affinity: Option<&SessionAffinity>,
        excluded_account_ids: &HashSet<String>,
    ) -> Result<RelayAttempt, AppError> {
        // A Codex `-fast` variant is a client-facing alias for its base model:
        // TT derives the variants into the catalog, and OpenCode resolves the
        // same aliases before sending. No upstream accepts the suffix, so route
        // selection uses the base id while logs keep the requested id.
        let requested_model = model.to_string();
        let model = self.routing_model(wire_api, model).await?;
        let model = model.as_ref();
        let mut candidates = self
            .state
            .db
            .list_provider_route_candidates(wire_api, model)
            .await?;
        candidates
            .retain(|candidate| !excluded_account_ids.contains(&candidate.account.account.id));
        let scope = RoutingScope {
            api_key_id: &self.api_key_id,
            public_model_id: model,
            wire_api,
        };
        if candidates.is_empty()
            && self
                .state
                .db
                .has_limited_provider_route(wire_api, model)
                .await?
        {
            self.state.relay_metrics.record_selection_failure(
                wire_api,
                model,
                "all_routes_rate_limited",
            );
            tracing::warn!(
                wire_api,
                model,
                excluded_accounts = excluded_account_ids.len(),
                "relay request rejected because all matching provider routes are rate limited"
            );
            return Err(AppError::TooManyRequestsWithRetryAfter(
                "all matching provider accounts are rate limited".into(),
                60,
            ));
        }
        let (selection, permit) = {
            let mut rng = rand::rng();
            let preferred = select_route(&candidates, affinity, &scope, &mut rng)
                .cloned()
                .ok_or_else(|| {
                    AppError::Forbidden("no active provider account is available".into())
                })?;
            match self.try_acquire(&preferred) {
                Some(permit) => (preferred, Some(permit)),
                None => match self.overflow(&candidates, &preferred, affinity, &scope, &mut rng) {
                    Some((selection, permit)) => (selection, Some(permit)),
                    None => (preferred, None),
                },
            }
        };
        let permit = match permit {
            Some(permit) => QueuedPermit {
                permit,
                waited: Duration::ZERO,
            },
            // Every same-tier account is saturated: queue on the preferred one.
            // There is no timeout; client cancellation drops this future and
            // removes the waiter from the queue, and shutdown aborts it.
            None => acquire_until_shutdown(&self.state, &selection).await?,
        };
        self.state.relay_metrics.record_selection(
            wire_api,
            &selection.role,
            if affinity.is_some() {
                "weighted_rendezvous"
            } else {
                "weighted_random"
            },
        );

        Ok(RelayAttempt {
            state: self.state.clone(),
            api_key_id: self.api_key_id.clone(),
            selection,
            requested_model,
            started: Instant::now(),
            queue_wait: permit.waited,
            permit: Some(permit.permit),
        })
    }

    /// Resolve a client-facing model id to the id used for route selection.
    ///
    /// Only `-fast` aliases are resolved, and an exact catalog entry always
    /// wins, so a route an operator declares for a `-fast` id keeps serving it.
    async fn routing_model<'a>(
        &self,
        wire_api: &str,
        model: &'a str,
    ) -> Result<Cow<'a, str>, AppError> {
        let base = crate::codex_catalog::policy::canonical_model_id(model);
        if base == model {
            return Ok(Cow::Borrowed(model));
        }
        let exact = self
            .state
            .db
            .list_provider_route_candidates(wire_api, model)
            .await?;
        Ok(if exact.is_empty() {
            Cow::Borrowed(base)
        } else {
            Cow::Borrowed(model)
        })
    }

    pub(crate) async fn select_account(
        &self,
        wire_api: &str,
        model: &str,
        affinity: Option<&SessionAffinity>,
        account_id: &str,
    ) -> Result<RelayAttempt, AppError> {
        let candidates = self
            .state
            .db
            .list_provider_route_candidates(wire_api, model)
            .await?;
        if !candidates
            .iter()
            .any(|candidate| candidate.account.account.id == account_id)
        {
            return Err(AppError::ServiceUnavailable(
                "retry account is no longer available".into(),
            ));
        }
        let excluded = candidates
            .into_iter()
            .filter(|candidate| candidate.account.account.id != account_id)
            .map(|candidate| candidate.account.account.id)
            .collect();
        self.select_excluding(wire_api, model, affinity, &excluded)
            .await
    }

    fn try_acquire(&self, selection: &ProviderRouteSelection) -> Option<AccountPermit> {
        self.state.account_concurrency.try_acquire(
            &selection.account.account.id,
            selection.account.account.max_running_requests,
        )
    }

    /// Picks another account in the preferred account's tier that has a free
    /// slot, keeping the usual weighted/affinity order among them.
    fn overflow(
        &self,
        candidates: &[ProviderRouteSelection],
        preferred: &ProviderRouteSelection,
        affinity: Option<&SessionAffinity>,
        scope: &RoutingScope<'_>,
        rng: &mut impl rand::Rng,
    ) -> Option<(ProviderRouteSelection, AccountPermit)> {
        let mut remaining: Vec<ProviderRouteSelection> = candidates
            .iter()
            .filter(|candidate| {
                candidate.role == preferred.role
                    && candidate.account.account.id != preferred.account.account.id
            })
            .cloned()
            .collect();
        while let Some(next) = select_route(&remaining, affinity, scope, rng).cloned() {
            if let Some(permit) = self.try_acquire(&next) {
                return Some((next, permit));
            }
            remaining.retain(|candidate| candidate.account.account.id != next.account.account.id);
        }
        None
    }
}

/// A running-request slot together with how long the request queued for it.
pub(crate) struct QueuedPermit {
    permit: AccountPermit,
    waited: Duration,
}

async fn acquire_until_shutdown(
    state: &AppState,
    selection: &ProviderRouteSelection,
) -> Result<QueuedPermit, AppError> {
    let mut shutdown = state.shutdown.subscribe();
    if *shutdown.borrow_and_update() {
        return Err(shutting_down_error());
    }
    let account_id = &selection.account.account.id;
    let limit = selection.account.account.max_running_requests;
    let load = state.account_concurrency.load(account_id);
    let may_wait = limit > 0 && (load.running >= limit || load.queued > 0);
    if may_wait {
        tracing::info!(
            account_id,
            wire_api = selection.account.account.wire_api.as_str(),
            model = selection.public_model_id.as_str(),
            max_running_requests = limit,
            running_requests = load.running,
            queued_requests = load.queued,
            "relay request waiting for running-request slot"
        );
    }
    let queued_at = Instant::now();
    let acquire = state.account_concurrency.acquire(account_id, limit);
    tokio::pin!(acquire);
    loop {
        tokio::select! {
            biased;
            changed = shutdown.changed() => match changed {
                Ok(()) if !*shutdown.borrow_and_update() => continue,
                _ => return Err(shutting_down_error()),
            },
            permit = &mut acquire => {
                let waited = queued_at.elapsed();
                if waited > Duration::ZERO {
                    tracing::info!(
                        account_id,
                        wire_api = selection.account.account.wire_api.as_str(),
                        model = selection.public_model_id.as_str(),
                        max_running_requests = limit,
                        queue_wait_ms = waited.as_millis() as u64,
                        "relay request acquired running-request slot"
                    );
                }
                return Ok(QueuedPermit { permit, waited });
            }
        }
    }
}

pub(crate) async fn authenticate_relay_api_key(
    state: &AppState,
    headers: &HeaderMap,
    query: Option<&str>,
) -> Result<ApiKeyRecord, AppError> {
    let api_key = extract_api_key(headers, query)
        .ok_or_else(|| AppError::Unauthorized("missing API key".into()))?;
    if !api_key.starts_with(&state.config.api_key_prefix) {
        return Err(AppError::Unauthorized("invalid API key prefix".into()));
    }
    state
        .db
        .validate_api_key(&api_key)
        .await?
        .ok_or_else(|| AppError::Unauthorized("invalid or inactive API key".into()))
}

pub(crate) fn sanitize_upstream_url(url: &str) -> String {
    match url.parse::<Uri>() {
        Ok(uri) => match (uri.scheme_str(), uri.authority()) {
            (Some(scheme), Some(authority)) => format!("{scheme}://{}{}", authority, uri.path()),
            _ => url.split('?').next().unwrap_or(url).to_string(),
        },
        Err(_) => url.split('?').next().unwrap_or(url).to_string(),
    }
}

async fn record_upstream_response_result(
    state: &AppState,
    selection: &ProviderRouteSelection,
    status: StatusCode,
    headers: &HeaderMap,
    body: &[u8],
) -> Result<Option<String>, AppError> {
    if status.is_success() {
        state
            .db
            .clear_due_provider_limits(
                &selection.account.account.id,
                &selection.upstream_model_id,
                Utc::now(),
            )
            .await?;
        state
            .db
            .mark_provider_result(&selection.account.account.id, "healthy", None)
            .await?;
        state
            .db
            .mark_route_success(&selection.route_id, status.as_u16())
            .await?;
        return Ok(None);
    }

    let failure = classify_response_failure(
        &selection.account.account.provider,
        status,
        headers,
        body,
        Utc::now(),
    );
    let error = failure.error.clone();
    record_route_failure_state(
        state,
        &selection.account.account.id,
        &selection.route_id,
        &selection.upstream_model_id,
        &failure,
    )
    .await?;
    Ok(Some(error))
}

async fn record_route_failure_state(
    state: &AppState,
    account_id: &str,
    route_id: &str,
    upstream_model_id: &str,
    failure: &RouteFailure,
) -> Result<(), AppError> {
    if let Some(provider_status) = failure.provider_status {
        state
            .db
            .mark_provider_result(account_id, provider_status, Some(&failure.error))
            .await?;
    }
    if let Some(kind) = failure.limit_kind {
        state
            .db
            .record_provider_limit(
                account_id,
                (!upstream_model_id.is_empty()).then_some(upstream_model_id),
                kind.as_str(),
                &failure.error,
                (kind == crate::routing::LimitKind::RateLimited)
                    .then_some(failure.cooldown_until)
                    .flatten(),
            )
            .await?;
    }
    state
        .db
        .mark_route_failure(
            route_id,
            failure.route_status,
            failure.status_code,
            &failure.error,
            failure.cooldown_until,
        )
        .await?;
    Ok(())
}
