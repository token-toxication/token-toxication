use std::{
    collections::BTreeMap,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

use serde_json::Value;

use crate::models::{
    RequestOutcomeMetric, RouteSelectionMetric, SelectionFailureMetric, SessionAffinityMetric,
};

const WARNING_INTERVAL: Duration = Duration::from_secs(300);

#[derive(Clone, Default)]
pub struct RelayMetrics {
    inner: Arc<Mutex<RelayMetricsInner>>,
}

#[derive(Default)]
struct RelayMetricsInner {
    overload_retries: u64,
    affinity: BTreeMap<(String, String, String), u64>,
    selections: BTreeMap<(String, String, String), u64>,
    requests: BTreeMap<(String, u16, String, u8), u64>,
    selection_failures: BTreeMap<(String, String, u16, String), u64>,
    last_missing_warning: BTreeMap<(String, String), Instant>,
}

impl RelayMetrics {
    pub(crate) fn record_overload_retry(&self) {
        self.inner
            .lock()
            .expect("relay metrics lock poisoned")
            .overload_retries += 1;
    }

    pub(crate) fn overload_retries(&self) -> u64 {
        self.inner
            .lock()
            .expect("relay metrics lock poisoned")
            .overload_retries
    }
    pub fn record_affinity(&self, client: &str, wire_api: &str, status: &str) {
        let mut inner = self.inner.lock().expect("relay metrics lock poisoned");
        *inner
            .affinity
            .entry((client.into(), wire_api.into(), status.into()))
            .or_default() += 1;

        if status == "missing" && client != "unknown" {
            let key = (client.to_string(), wire_api.to_string());
            let now = Instant::now();
            let should_warn = inner
                .last_missing_warning
                .get(&key)
                .is_none_or(|last| now.duration_since(*last) >= WARNING_INTERVAL);
            if should_warn {
                inner.last_missing_warning.insert(key, now);
                tracing::warn!(
                    client,
                    wire_api,
                    "known client request is missing a supported session affinity identifier"
                );
            }
        }
    }

    pub fn record_selection(&self, wire_api: &str, role: &str, affinity: &str) {
        let mut inner = self.inner.lock().expect("relay metrics lock poisoned");
        *inner
            .selections
            .entry((wire_api.into(), role.into(), affinity.into()))
            .or_default() += 1;
    }

    pub fn record_request(
        &self,
        transport: &str,
        status_code: u16,
        error: Option<&str>,
        retry_count: u8,
    ) {
        let outcome = request_outcome(status_code, error);
        let mut inner = self.inner.lock().expect("relay metrics lock poisoned");
        *inner
            .requests
            .entry((
                transport.to_string(),
                status_code,
                outcome.to_string(),
                retry_count,
            ))
            .or_default() += 1;
    }

    pub fn classify_upstream_error(value: &Value) -> Option<&'static str> {
        let error = value.get("error").unwrap_or(value);
        let message = error
            .get("message")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_ascii_lowercase();
        if message.contains("items are not persisted")
            || (message.contains("item with id") && message.contains("not found"))
        {
            return Some("replay_missing_item");
        }
        if message.contains("turn state")
            || message.contains("turn_state")
            || message.contains("x-codex-turn-state")
        {
            return Some("turn_state");
        }
        let code = error
            .get("code")
            .or_else(|| error.get("type"))
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_ascii_lowercase();
        if code.contains("previous_response_not_found") {
            return Some("replay_missing_item");
        }
        if code.contains("invalid_request") {
            return Some("invalid_request");
        }
        None
    }

    pub fn classify_upstream_error_body(body: &[u8]) -> Option<&'static str> {
        let value = serde_json::from_slice::<Value>(body).ok()?;
        Self::classify_upstream_error(&value)
    }

    pub fn record_selection_failure(&self, wire_api: &str, model: &str, reason: &str) {
        let mut inner = self.inner.lock().expect("relay metrics lock poisoned");
        *inner
            .selection_failures
            .entry((wire_api.into(), model.into(), 429, reason.into()))
            .or_default() += 1;
    }

    pub fn snapshot(
        &self,
    ) -> (
        Vec<SessionAffinityMetric>,
        Vec<RouteSelectionMetric>,
        Vec<RequestOutcomeMetric>,
        Vec<SelectionFailureMetric>,
    ) {
        let inner = self.inner.lock().expect("relay metrics lock poisoned");
        let affinity = inner
            .affinity
            .iter()
            .map(
                |((client, wire_api, status), count)| SessionAffinityMetric {
                    client: client.clone(),
                    wire_api: wire_api.clone(),
                    status: status.clone(),
                    count: *count,
                },
            )
            .collect();
        let selections = inner
            .selections
            .iter()
            .map(|((wire_api, role, affinity), count)| RouteSelectionMetric {
                wire_api: wire_api.clone(),
                role: role.clone(),
                affinity: affinity.clone(),
                count: *count,
            })
            .collect();
        let requests = inner
            .requests
            .iter()
            .map(
                |((transport, status_code, outcome, retry_count), count)| RequestOutcomeMetric {
                    transport: transport.clone(),
                    status_code: *status_code,
                    outcome: outcome.clone(),
                    retry_count: *retry_count,
                    count: *count,
                },
            )
            .collect();
        let selection_failures = inner
            .selection_failures
            .iter()
            .map(
                |((wire_api, model, status_code, reason), count)| SelectionFailureMetric {
                    wire_api: wire_api.clone(),
                    model: model.clone(),
                    status_code: *status_code,
                    reason: reason.clone(),
                    count: *count,
                },
            )
            .collect();
        (affinity, selections, requests, selection_failures)
    }
}

pub fn request_outcome(status_code: u16, error: Option<&str>) -> &'static str {
    if (200..300).contains(&status_code) {
        return "success";
    }
    if error.is_some_and(|value| value.contains("server shut down")) {
        return "server_shutdown";
    }
    if status_code == 499 || error.is_some_and(|value| value.contains("client disconnected")) {
        return "client_disconnect";
    }
    let error = error.unwrap_or_default().to_ascii_lowercase();
    if error.contains("replay_missing_item") {
        return "replay_missing_item";
    }
    if error.contains("turn_state") {
        return "turn_state";
    }
    if error.contains("invalid_request") {
        return "invalid_request";
    }
    if status_code == 429
        || error.contains("rate limit")
        || error.contains("quota")
        || error.contains("429")
    {
        return "rate_limited";
    }
    if error.contains("server_is_overloaded") || error.contains("overload") {
        return "overload";
    }
    if error.contains("incomplete") {
        return "incomplete";
    }
    if matches!(status_code, 401 | 403)
        || error.contains("unauthorized")
        || error.contains("forbidden")
        || error.contains("authorization")
    {
        return "authorization";
    }
    if error.contains("timeout") || error.contains("transport") || error.contains("connect") {
        return "transport";
    }
    if status_code >= 500 {
        return "upstream_error";
    }
    if status_code >= 400 {
        return "client_error";
    }
    "other"
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn records_transport_and_outcome_dimensions() {
        let metrics = RelayMetrics::default();
        metrics.record_request("http_sse", 200, None, 0);
        metrics.record_request("websocket", 502, Some("upstream stream failed"), 1);
        metrics.record_request("websocket", 502, Some("upstream stream failed"), 1);
        let (_, _, requests, _) = metrics.snapshot();
        assert_eq!(
            requests,
            vec![
                RequestOutcomeMetric {
                    transport: "http_sse".into(),
                    status_code: 200,
                    outcome: "success".into(),
                    retry_count: 0,
                    count: 1,
                },
                RequestOutcomeMetric {
                    transport: "websocket".into(),
                    status_code: 502,
                    outcome: "upstream_error".into(),
                    retry_count: 1,
                    count: 2,
                },
            ]
        );
    }

    #[test]
    fn records_generic_retry_count_for_failover_attempts() {
        let metrics = RelayMetrics::default();
        metrics.record_request("http", 429, Some("upstream returned 429"), 2);

        let (_, _, requests, _) = metrics.snapshot();
        assert_eq!(requests[0].retry_count, 2);
        assert_eq!(requests[0].outcome, "rate_limited");
    }

    #[test]
    fn classifies_known_failure_boundaries() {
        assert_eq!(request_outcome(499, None), "client_disconnect");
        assert_eq!(
            request_outcome(
                499,
                Some("server shut down before the upstream stream completed")
            ),
            "server_shutdown"
        );
        assert_eq!(
            request_outcome(429, Some("upstream returned 429")),
            "rate_limited"
        );
        assert_eq!(
            request_outcome(502, Some("server_is_overloaded")),
            "overload"
        );
        assert_eq!(request_outcome(401, None), "authorization");
        assert_eq!(
            request_outcome(400, Some("upstream replay_missing_item")),
            "replay_missing_item"
        );
        assert_eq!(
            request_outcome(400, Some("upstream turn_state")),
            "turn_state"
        );
    }

    #[test]
    fn classifies_only_bounded_upstream_metadata() {
        assert_eq!(
            RelayMetrics::classify_upstream_error(&serde_json::json!({
                "message": "Item with id 'rs_secret' not found. Items are not persisted when store is set to false."
            })),
            Some("replay_missing_item")
        );
        assert_eq!(
            RelayMetrics::classify_upstream_error(&serde_json::json!({
                "message": "invalid x-codex-turn-state continuation"
            })),
            Some("turn_state")
        );
        assert_eq!(
            RelayMetrics::classify_upstream_error(&serde_json::json!({
                "message": "prompt and credentials must not be logged"
            })),
            None
        );
        assert_eq!(
            RelayMetrics::classify_upstream_error(&serde_json::json!({
                "code": "previous_response_not_found"
            })),
            Some("replay_missing_item")
        );
    }

    #[test]
    fn records_route_selection_failures_separately() {
        let metrics = RelayMetrics::default();
        metrics.record_selection_failure(
            "openai-responses",
            "gpt-6-astra",
            "all_routes_rate_limited",
        );
        let (_, _, _, failures) = metrics.snapshot();
        assert_eq!(failures[0].status_code, 429);
        assert_eq!(failures[0].count, 1);
        assert_eq!(failures[0].reason, "all_routes_rate_limited");
    }
}
