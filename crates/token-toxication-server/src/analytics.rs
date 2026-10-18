//! Bounded, metadata-only analytics. Readers never acquire the relay's DB mutex.
mod query;

use std::{
    collections::VecDeque,
    path::PathBuf,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

use chrono::{DateTime, Utc};
use rusqlite::{Connection, OpenFlags};
use serde::{Deserialize, Serialize};
use tokio::sync::Semaphore;
use utoipa::{IntoParams, ToSchema};

use crate::{
    AppState,
    error::AppError,
    models::{
        RequestOutcomeMetric, RouteSelectionMetric, SelectionFailureMetric, SessionAffinityMetric,
    },
};

const CACHE_TTL: Duration = Duration::from_secs(10);
const QUERY_TIMEOUT: Duration = Duration::from_secs(5);
const MAX_CACHE_ENTRIES: usize = 16;

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Deserialize, Serialize, ToSchema)]
pub enum AnalyticsRange {
    #[serde(rename = "1h")]
    Hour,
    #[default]
    #[serde(rename = "24h")]
    Day,
    #[serde(rename = "7d")]
    Week,
}

impl AnalyticsRange {
    fn dimensions(self) -> (i64, i64) {
        match self {
            Self::Hour => (60, 60),
            Self::Day => (300, 288),
            Self::Week => (3600, 168),
        }
    }
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Deserialize, Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum AnalyticsRankBy {
    #[default]
    Attempts,
    Tokens,
    Failures,
}

#[derive(Debug, Default, Clone, PartialEq, Eq, Deserialize, Serialize, IntoParams)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[into_params(parameter_in = Query)]
pub struct AnalyticsQuery {
    #[serde(default)]
    pub range: AnalyticsRange,
    pub model: Option<String>,
    /// A stable provider account ID, or `unassigned`.
    pub account_id: Option<String>,
    #[serde(default)]
    pub rank_by: AnalyticsRankBy,
}

impl AnalyticsQuery {
    pub fn validate(&self) -> Result<(), AppError> {
        for value in [&self.model, &self.account_id].into_iter().flatten() {
            if value.is_empty() || value.len() > 200 || value.chars().any(char::is_control) {
                return Err(AppError::BadRequest("invalid analytics filter".into()));
            }
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, Deserialize, Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum AnalyticsDimension {
    Model,
    Account,
}

#[derive(Debug, Clone, Deserialize, IntoParams)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[into_params(parameter_in = Query)]
pub struct AnalyticsOptionsQuery {
    #[serde(default)]
    pub range: AnalyticsRange,
    pub dimension: AnalyticsDimension,
    #[serde(default)]
    pub search: String,
    #[serde(default)]
    pub page: u32,
}

#[derive(Debug, Default, Clone, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct AnalyticsPercentiles {
    pub samples: u64,
    pub p50: Option<f64>,
    pub p95: Option<f64>,
}

#[derive(Debug, Default, Clone, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct AnalyticsStats {
    pub attempts: u64,
    pub success: u64,
    pub failures: u64,
    pub client_disconnect: u64,
    pub server_shutdown: u64,
    pub unknown: u64,
    pub input_tokens: u64,
    pub output_tokens: u64,
    /// Only inputs whose complete cache split is known.
    pub classified_input_tokens: u64,
    pub cached_input_tokens: u64,
    pub cache_samples: u64,
    pub complete_usage_samples: u64,
    pub retry_attempts: u64,
    pub overload_retries: u64,
    pub ttft: AnalyticsPercentiles,
    pub latency: AnalyticsPercentiles,
    pub queue: AnalyticsPercentiles,
    pub otps: AnalyticsPercentiles,
}

#[derive(Debug, Clone, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct AnalyticsBucket {
    pub started_at: DateTime<Utc>,
    pub partial: bool,
    pub stats: AnalyticsStats,
}

#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct AnalyticsCount {
    pub name: String,
    pub count: u64,
}

#[derive(Debug, Clone, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct AnalyticsGroup {
    pub id: Option<String>,
    pub name: Option<String>,
    pub deleted: bool,
    pub other: bool,
    pub attempts: u64,
    pub failures: u64,
    pub tokens: u64,
}

#[derive(Debug, Clone, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct AnalyticsOptions {
    pub data: Vec<AnalyticsGroup>,
    pub has_more: bool,
}

#[derive(Debug, Clone, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct AnalyticsHistory {
    pub range: AnalyticsRange,
    pub model: Option<String>,
    pub account_id: Option<String>,
    pub rank_by: AnalyticsRankBy,
    pub window_started_at: DateTime<Utc>,
    pub window_ended_at: DateTime<Utc>,
    pub generated_at: DateTime<Utc>,
    pub bucket_duration_seconds: u64,
    pub summary: AnalyticsStats,
    pub buckets: Vec<AnalyticsBucket>,
    pub outcomes: Vec<AnalyticsCount>,
    pub transports: Vec<AnalyticsCount>,
    pub statuses: Vec<AnalyticsCount>,
    pub models: Vec<AnalyticsGroup>,
    pub accounts: Vec<AnalyticsGroup>,
}

#[derive(Debug, Clone, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct AnalyticsRuntime {
    pub started_at: DateTime<Utc>,
    pub snapshot_at: DateTime<Utc>,
    pub request_outcomes: Vec<RequestOutcomeMetric>,
    pub route_selections: Vec<RouteSelectionMetric>,
    pub session_affinity: Vec<SessionAffinityMetric>,
    pub selection_failures: Vec<SelectionFailureMetric>,
    pub overload_retries: u64,
}

#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct AnalyticsResponse {
    pub history: AnalyticsHistory,
    pub runtime: AnalyticsRuntime,
}

impl AnalyticsRuntime {
    pub fn snapshot(state: &AppState) -> Self {
        let (session_affinity, route_selections, request_outcomes, selection_failures) =
            state.relay_metrics.snapshot();
        Self {
            started_at: state.started_at,
            snapshot_at: Utc::now(),
            request_outcomes,
            route_selections,
            session_affinity,
            selection_failures,
            overload_retries: state.relay_metrics.overload_retries(),
        }
    }
}

type Cache = VecDeque<(AnalyticsQuery, Instant, AnalyticsHistory)>;

#[cfg(test)]
mod tests;

#[derive(Clone)]
pub(crate) struct AnalyticsReader {
    path: Arc<PathBuf>,
    gate: Arc<Semaphore>,
    cache: Arc<Mutex<Cache>>,
}

impl AnalyticsReader {
    pub fn new(path: PathBuf) -> Self {
        Self {
            path: Arc::new(path),
            gate: Arc::new(Semaphore::new(1)),
            cache: Default::default(),
        }
    }

    fn cached(&self, query: &AnalyticsQuery) -> Option<AnalyticsHistory> {
        self.cache
            .lock()
            .expect("analytics cache poisoned")
            .iter()
            .find(|(key, at, _)| key == query && at.elapsed() < CACHE_TTL)
            .map(|(_, _, history)| history.clone())
    }

    async fn read<T: Send + 'static>(
        &self,
        run: impl FnOnce(&mut Connection) -> Result<T, rusqlite::Error> + Send + 'static,
    ) -> Result<T, AppError> {
        let permit =
            tokio::time::timeout(Duration::from_secs(1), self.gate.clone().acquire_owned())
                .await
                .map_err(|_| {
                    AppError::ServiceUnavailable("analytics is busy; retry shortly".into())
                })?
                .map_err(|_| AppError::ServiceUnavailable("analytics unavailable".into()))?;
        let path = self.path.clone();
        tokio::task::spawn_blocking(move || {
            // Keep the permit inside the blocking task even if its HTTP caller disconnects.
            let _permit = permit;
            let memory = path.to_string_lossy().contains("mode=memory");
            let flags = if memory {
                OpenFlags::SQLITE_OPEN_READ_WRITE
            } else {
                OpenFlags::SQLITE_OPEN_READ_ONLY
            } | OpenFlags::SQLITE_OPEN_URI;
            let mut conn = Connection::open_with_flags(path.as_ref(), flags)?;
            conn.busy_timeout(Duration::from_millis(500))?;
            conn.execute_batch("PRAGMA query_only=ON; PRAGMA cache_size=-8192;")?;
            let started = Instant::now();
            conn.progress_handler(10_000, Some(move || started.elapsed() >= QUERY_TIMEOUT));
            run(&mut conn)
        })
        .await
        .map_err(|_| AppError::Internal("analytics task failed".into()))?
        .map_err(|error| {
            tracing::warn!(sqlite_code = ?error.sqlite_error_code(), "analytics query failed");
            AppError::ServiceUnavailable(
                "analytics query unavailable or exceeded its time limit".into(),
            )
        })
    }

    pub async fn history(&self, query: AnalyticsQuery) -> Result<AnalyticsHistory, AppError> {
        query.validate()?;
        if let Some(history) = self.cached(&query) {
            return Ok(history);
        }
        let reader = self.clone();
        self.read(move |conn| {
            if let Some(history) = reader.cached(&query) {
                return Ok(history);
            }
            let result = query::history(conn, &query, Utc::now())?;
            let mut cache = reader.cache.lock().expect("analytics cache poisoned");
            cache.retain(|(key, at, _)| key != &query && at.elapsed() < CACHE_TTL);
            while cache.len() >= MAX_CACHE_ENTRIES {
                cache.pop_front();
            }
            cache.push_back((query, Instant::now(), result.clone()));
            Ok(result)
        })
        .await
    }

    pub async fn options(
        &self,
        query: AnalyticsOptionsQuery,
    ) -> Result<AnalyticsOptions, AppError> {
        if query.search.len() > 100
            || query.page > 1000
            || query.search.chars().any(char::is_control)
        {
            return Err(AppError::BadRequest(
                "invalid analytics options query".into(),
            ));
        }
        self.read(move |conn| query::options(conn, &query, Utc::now()))
            .await
    }
}
