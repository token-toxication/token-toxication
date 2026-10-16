use std::{
    collections::BTreeMap,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

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
    affinity: BTreeMap<(String, String, String), u64>,
    selections: BTreeMap<(String, String, String), u64>,
    requests: BTreeMap<(String, u16, String), u64>,
    selection_failures: BTreeMap<(String, String, u16, String), u64>,
    last_missing_warning: BTreeMap<(String, String), Instant>,
}

impl RelayMetrics {
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

    pub fn record_request(&self, transport: &str, status_code: u16, error: Option<&str>) {
        let outcome = request_outcome(status_code, error);
        let mut inner = self.inner.lock().expect("relay metrics lock poisoned");
        *inner
            .requests
            .entry((transport.to_string(), status_code, outcome.to_string()))
            .or_default() += 1;
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
                |((transport, status_code, outcome), count)| RequestOutcomeMetric {
                    transport: transport.clone(),
                    status_code: *status_code,
                    outcome: outcome.clone(),
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
        metrics.record_request("http_sse", 200, None);
        metrics.record_request("websocket", 502, Some("upstream stream failed"));
        metrics.record_request("websocket", 502, Some("upstream stream failed"));
        let (_, _, requests, _) = metrics.snapshot();
        assert_eq!(
            requests,
            vec![
                RequestOutcomeMetric {
                    transport: "http_sse".into(),
                    status_code: 200,
                    outcome: "success".into(),
                    count: 1,
                },
                RequestOutcomeMetric {
                    transport: "websocket".into(),
                    status_code: 502,
                    outcome: "upstream_error".into(),
                    count: 2,
                },
            ]
        );
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
