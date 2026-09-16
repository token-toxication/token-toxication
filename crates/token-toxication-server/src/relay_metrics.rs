use std::{
    collections::BTreeMap,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

use crate::models::{RouteSelectionMetric, SessionAffinityMetric};

const WARNING_INTERVAL: Duration = Duration::from_secs(300);

#[derive(Clone, Default)]
pub struct RelayMetrics {
    inner: Arc<Mutex<RelayMetricsInner>>,
}

#[derive(Default)]
struct RelayMetricsInner {
    affinity: BTreeMap<(String, String, String), u64>,
    selections: BTreeMap<(String, String, String), u64>,
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

    pub fn snapshot(&self) -> (Vec<SessionAffinityMetric>, Vec<RouteSelectionMetric>) {
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
        (affinity, selections)
    }
}
