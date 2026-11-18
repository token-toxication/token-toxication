//! Fetching an account's own Codex-native catalog.
//!
//! Security: a catalog body carries model instruction text. It is never logged,
//! echoed into errors, or written to disk; only status codes, byte lengths, and
//! model counts are observable.
//!
//! Two upstream constraints shape this module, both verified against the live
//! ChatGPT backend:
//!
//! - `/codex/models` **requires** a `client_version` query parameter. An absent
//!   parameter returns `400`, so a request that omitted it would fail the whole
//!   catalog. TT always sends the latest verified release, including when the
//!   client supplied an older version.
//! - The catalog is shaped **by version**: an older or unknown value returns a
//!   smaller set. TT deliberately uses one current release so an older client
//!   cannot downgrade the managed catalog.
//! - The Codex client abandons its catalog refresh after 5 seconds. TT's own
//!   budget stays well inside that so a slow upstream cannot make the client
//!   time out and lose its model list.

use std::time::{Duration, Instant};

use axum::http::header;
use serde::Deserialize;
use tokio::sync::Mutex;

use crate::{codex_subscription::CodexSubscriptionAuthorization, error::AppError};

/// Upper bound on an upstream catalog body.
///
/// Larger than the client-side cap on purpose: TT reads the whole document so it
/// can decide what fits, instead of forwarding a truncated body.
pub const MAX_UPSTREAM_CATALOG_BYTES: usize = 4 * 1024 * 1024;

/// TT's upstream budget, comfortably inside the client's 5 second refresh.
const CATALOG_TIMEOUT: Duration = Duration::from_secs(3);

/// Default Codex release used until the periodic latest-version check succeeds.
///
/// The backend shapes the catalog to the requested version. Sending an older
/// client version would hide models newly enabled for the subscription and
/// could make managed reconciliation delete their routes.
///
/// Keep this current as an emergency fallback. The running service refreshes
/// the value from the official package registry every two hours.
pub const DEFAULT_CLIENT_VERSION: &str = "0.169.1";
/// Official package metadata used to discover the newest Codex client release.
pub const CODEX_LATEST_VERSION_URL: &str = "https://registry.npmjs.org/@openai/codex/latest";
/// How often a running relay checks the package registry for a newer client.
pub const CLIENT_VERSION_REFRESH_INTERVAL: Duration = Duration::from_secs(2 * 60 * 60);
const CLIENT_VERSION_TIMEOUT: Duration = Duration::from_secs(10);
const MAX_CLIENT_VERSION_BODY_BYTES: usize = 64 * 1024;

#[derive(Debug)]
struct ClientVersionState {
    version: String,
    checked_at: Option<Instant>,
}

/// Cached latest Codex client version shared by catalog requests and the
/// managed-catalog monitor.
#[derive(Debug)]
pub struct ClientVersionCache {
    latest_url: String,
    state: Mutex<ClientVersionState>,
}

impl Default for ClientVersionCache {
    fn default() -> Self {
        Self {
            latest_url: CODEX_LATEST_VERSION_URL.to_string(),
            state: Mutex::new(ClientVersionState {
                version: DEFAULT_CLIENT_VERSION.to_string(),
                checked_at: Some(Instant::now()),
            }),
        }
    }
}

impl ClientVersionCache {
    /// Start with the fallback version and force a registry check on first use.
    pub fn startup() -> Self {
        Self {
            latest_url: CODEX_LATEST_VERSION_URL.to_string(),
            state: Mutex::new(ClientVersionState {
                version: DEFAULT_CLIENT_VERSION.to_string(),
                checked_at: None,
            }),
        }
    }

    /// Point the refresh at a test registry while retaining production timing.
    #[cfg(test)]
    fn for_test(latest_url: String, version: String, checked_at: Option<Instant>) -> Self {
        Self {
            latest_url,
            state: Mutex::new(ClientVersionState {
                version,
                checked_at,
            }),
        }
    }

    /// Return the cached latest version, refreshing it at most once per interval.
    ///
    /// A registry failure keeps the last successful version and suppresses
    /// another refresh until the interval elapses.
    pub async fn current(&self, http: &aioduct::TokioClient) -> String {
        let mut state = self.state.lock().await;
        if state
            .checked_at
            .is_some_and(|checked_at| checked_at.elapsed() < CLIENT_VERSION_REFRESH_INTERVAL)
        {
            return state.version.clone();
        }

        match fetch_latest_client_version(http, &self.latest_url).await {
            Ok(version) => {
                tracing::info!(
                    client_version = version.as_str(),
                    "refreshed latest Codex client version"
                );
                state.version = version;
            }
            Err(error) => {
                tracing::warn!(
                    reason = %error,
                    client_version = state.version.as_str(),
                    "latest Codex client version refresh failed; using cached version"
                );
            }
        }
        state.checked_at = Some(Instant::now());
        state.version.clone()
    }
}

#[derive(Debug, Deserialize)]
struct NpmLatestPackage {
    version: String,
}

async fn fetch_latest_client_version(
    http: &aioduct::TokioClient,
    url: &str,
) -> Result<String, String> {
    let request = http
        .get(url)
        .map_err(|error| error.to_string())?
        .header_str(header::ACCEPT.as_str(), "application/json")
        .map_err(|error| error.to_string())?;
    let response = tokio::time::timeout(CLIENT_VERSION_TIMEOUT, request.send())
        .await
        .map_err(|_| "latest version request timed out".to_string())?
        .map_err(|error| error.to_string())?;
    if !response.status().is_success() {
        return Err(format!(
            "latest version request failed with status {}",
            response.status().as_u16()
        ));
    }
    let bytes = response.bytes().await.map_err(|error| error.to_string())?;
    if bytes.len() > MAX_CLIENT_VERSION_BODY_BYTES {
        return Err("latest version response exceeded size limit".to_string());
    }
    let package: NpmLatestPackage = serde_json::from_slice(&bytes)
        .map_err(|error| format!("invalid latest version JSON: {error}"))?;
    validate_client_version(&package.version)
        .ok_or_else(|| "latest version response contained an invalid version".to_string())
}

fn validate_client_version(value: &str) -> Option<String> {
    let value = value.trim();
    let core = value.split(['-', '+']).next().unwrap_or_default();
    let mut parts = core.split('.');
    let major = parts.next()?;
    let minor = parts.next()?;
    let patch = parts.next()?;
    if parts.next().is_some()
        || major.is_empty()
        || minor.is_empty()
        || patch.is_empty()
        || !major.chars().all(|character| character.is_ascii_digit())
        || !minor.chars().all(|character| character.is_ascii_digit())
        || !patch.chars().all(|character| {
            character.is_ascii_alphanumeric() || matches!(character, '-' | '_' | '+')
        })
        || value.len() > 32
    {
        return None;
    }
    Some(value.to_string())
}

/// A catalog response, or a `304` revalidation result.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FetchedCatalog {
    /// The upstream returned a fresh body.
    Ok { body: Vec<u8>, etag: Option<String> },
    /// The cached copy is still current.
    NotModified,
}

/// Fetch the catalog for one subscription account.
///
/// `etag` turns the call into a conditional request; a `304` is reported as
/// [`FetchedCatalog::NotModified`] so the caller can keep the body it has.
pub async fn fetch(
    http: &aioduct::TokioClient,
    endpoint: &str,
    auth: &CodexSubscriptionAuthorization,
    client_version: &str,
    etag: Option<&str>,
) -> Result<FetchedCatalog, AppError> {
    let separator = if endpoint.contains('?') { '&' } else { '?' };
    let url = format!("{endpoint}{separator}client_version={client_version}");

    let mut request = http
        .get(&url)
        .map_err(AppError::Upstream)?
        .bearer_auth(&auth.access_token);
    if let Some(account_id) = auth.account_id.as_deref() {
        request = request
            .header_str("chatgpt-account-id", account_id)
            .map_err(AppError::Upstream)?;
    }
    request = request
        .header_str("originator", auth.originator)
        .map_err(AppError::Upstream)?;
    if let Some(etag) = etag {
        request = request
            .header_str(header::IF_NONE_MATCH.as_str(), etag)
            .map_err(AppError::Upstream)?;
    }
    let request = request
        .header_str(header::ACCEPT.as_str(), "application/json")
        .map_err(AppError::Upstream)?;

    let response = tokio::time::timeout(CATALOG_TIMEOUT, request.send())
        .await
        .map_err(|_| AppError::ServiceUnavailable("codex catalog request timed out".into()))?
        .map_err(|error| AppError::Upstream(error.into()))?;

    let status = response.status();
    if status == axum::http::StatusCode::NOT_MODIFIED {
        return Ok(FetchedCatalog::NotModified);
    }
    let etag = response
        .headers()
        .get(header::ETAG)
        .and_then(|value| value.to_str().ok())
        .map(str::to_owned);
    if !status.is_success() {
        // Never echo the body: it can contain instruction text.
        return Err(AppError::ServiceUnavailable(format!(
            "codex catalog request failed with status {}",
            status.as_u16()
        )));
    }

    let bytes = response.bytes().await.map_err(AppError::Upstream)?;
    if bytes.len() > MAX_UPSTREAM_CATALOG_BYTES {
        return Err(AppError::ServiceUnavailable(format!(
            "codex catalog response exceeded {MAX_UPSTREAM_CATALOG_BYTES} bytes"
        )));
    }

    Ok(FetchedCatalog::Ok {
        body: bytes.to_vec(),
        etag,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{Router, http::HeaderMap, routing::get};
    use std::sync::{Arc, Mutex};
    use tokio::net::TcpListener;

    fn authorization() -> CodexSubscriptionAuthorization {
        CodexSubscriptionAuthorization {
            access_token: "synthetic-access".to_string(),
            account_id: Some("synthetic-account".to_string()),
            endpoint: String::new(),
            originator: "opencode",
        }
    }

    type Seen = Arc<Mutex<Vec<(String, HeaderMap)>>>;

    async fn serve(response: (axum::http::StatusCode, &'static str)) -> (String, Seen) {
        let seen: Seen = Arc::new(Mutex::new(Vec::new()));
        let handler_seen = Arc::clone(&seen);
        let app = Router::new().route(
            "/codex/models",
            get(move |uri: axum::http::Uri, headers: HeaderMap| {
                let handler_seen = Arc::clone(&handler_seen);
                let (status, body) = response;
                async move {
                    handler_seen
                        .lock()
                        .expect("lock seen")
                        .push((uri.to_string(), headers));
                    (status, body)
                }
            }),
        );
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
        let address = listener.local_addr().expect("address");
        tokio::spawn(async move {
            let _ = axum::serve(listener, app).await;
        });
        (format!("http://{address}/codex/models"), seen)
    }

    fn client() -> aioduct::TokioClient {
        aioduct::TokioClient::builder()
            .tls(aioduct::tls::RustlsConnector::with_webpki_roots())
            .user_agent("token-toxication-test")
            .timeout(Duration::from_secs(5))
            .read_timeout(Duration::from_secs(5))
            .build()
            .expect("build test HTTP client")
    }

    #[tokio::test]
    async fn sends_account_identity_and_the_client_version() {
        let (endpoint, seen) = serve((axum::http::StatusCode::OK, r#"{"models":[]}"#)).await;
        let http = client();

        let fetched = fetch(&http, &endpoint, &authorization(), "0.162.0", None)
            .await
            .expect("fetch catalog");
        match fetched {
            FetchedCatalog::Ok { body, etag } => {
                assert_eq!(etag, None);
                assert_eq!(body, br#"{"models":[]}"#);
            }
            other => panic!("expected a body, got {other:?}"),
        }

        let (uri, headers) = seen
            .lock()
            .expect("lock seen")
            .first()
            .cloned()
            .expect("captured request");
        assert_eq!(uri, "/codex/models?client_version=0.162.0");
        assert_eq!(headers["authorization"], "Bearer synthetic-access");
        assert_eq!(headers["chatgpt-account-id"], "synthetic-account");
        assert_eq!(headers["originator"], "opencode");
        assert!(headers.get("if-none-match").is_none());
    }

    #[tokio::test]
    async fn a_conditional_request_carries_the_etag() {
        let (endpoint, seen) = serve((axum::http::StatusCode::NOT_MODIFIED, "")).await;
        let http = client();

        let fetched = fetch(
            &http,
            &endpoint,
            &authorization(),
            "0.162.0",
            Some("W/\"abc\""),
        )
        .await
        .expect("fetch catalog");
        assert_eq!(fetched, FetchedCatalog::NotModified);

        let (_, headers) = seen
            .lock()
            .expect("lock seen")
            .first()
            .cloned()
            .expect("captured request");
        assert_eq!(headers["if-none-match"], "W/\"abc\"");
    }

    #[tokio::test]
    async fn upstream_failure_reports_status_without_echoing_the_body() {
        let (endpoint, _) = serve((
            axum::http::StatusCode::UNAUTHORIZED,
            r#"{"error":"leaked-instruction-text"}"#,
        ))
        .await;
        let http = client();

        let error = fetch(&http, &endpoint, &authorization(), "0.162.0", None)
            .await
            .expect_err("upstream failure");
        let message = error.to_string();
        assert!(message.contains("401"), "unexpected message: {message}");
        assert!(
            !message.contains("leaked-instruction-text"),
            "error must not echo the upstream body"
        );
    }

    #[tokio::test]
    async fn etag_is_surfaced_when_present() {
        let app = Router::new().route(
            "/codex/models",
            get(|| async {
                (
                    axum::http::StatusCode::OK,
                    [("etag", "W/\"abc\"")],
                    r#"{"models":[]}"#,
                )
            }),
        );
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
        let address = listener.local_addr().expect("address");
        tokio::spawn(async move {
            let _ = axum::serve(listener, app).await;
        });

        let http = client();
        let fetched = fetch(
            &http,
            &format!("http://{address}/codex/models"),
            &authorization(),
            "0.162.0",
            None,
        )
        .await
        .expect("fetch catalog");
        match fetched {
            FetchedCatalog::Ok { etag, .. } => assert_eq!(etag.as_deref(), Some("W/\"abc\"")),
            other => panic!("expected a body, got {other:?}"),
        }
    }

    #[test]
    fn the_latest_version_is_a_real_release() {
        let parts: Vec<&str> = DEFAULT_CLIENT_VERSION.split('.').collect();
        assert_eq!(parts.len(), 3, "expected major.minor.patch");
        for part in &parts {
            assert!(
                part.parse::<u32>().is_ok(),
                "version component {part:?} is not numeric"
            );
        }
        assert_ne!(DEFAULT_CLIENT_VERSION, "0.0.0");
        assert!(
            parts.iter().any(|part| *part != "0"),
            "latest version must name a real release"
        );
    }

    #[test]
    fn latest_version_validation_accepts_semver_and_rejects_untrusted_values() {
        assert_eq!(
            validate_client_version("0.169.1"),
            Some("0.169.1".to_string())
        );
        assert_eq!(
            validate_client_version("0.169.1-alpha.1"),
            Some("0.169.1-alpha.1".to_string())
        );
        assert_eq!(validate_client_version("latest"), None);
        assert_eq!(validate_client_version("../../etc/passwd"), None);
    }

    #[tokio::test]
    async fn a_stale_cache_refreshes_from_the_registry_and_is_reused() {
        let calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let counter = calls.clone();
        let app = Router::new().route(
            "/latest",
            get(move || {
                let counter = counter.clone();
                async move {
                    counter.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                    axum::Json(serde_json::json!({ "version": "0.170.0" }))
                }
            }),
        );
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
        let address = listener.local_addr().expect("address");
        tokio::spawn(async move {
            let _ = axum::serve(listener, app).await;
        });

        let cache = ClientVersionCache::for_test(
            format!("http://{address}/latest"),
            DEFAULT_CLIENT_VERSION.to_string(),
            None,
        );
        assert_eq!(cache.current(&client()).await, "0.170.0");
        assert_eq!(cache.current(&client()).await, "0.170.0");
        assert_eq!(
            calls.load(std::sync::atomic::Ordering::Relaxed),
            1,
            "a fresh cached version must not be fetched again"
        );
    }

    #[tokio::test]
    async fn a_failed_registry_refresh_keeps_the_cached_version() {
        let app = Router::new().route(
            "/latest",
            get(|| async { axum::http::StatusCode::SERVICE_UNAVAILABLE }),
        );
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
        let address = listener.local_addr().expect("address");
        tokio::spawn(async move {
            let _ = axum::serve(listener, app).await;
        });

        let cache = ClientVersionCache::for_test(
            format!("http://{address}/latest"),
            "0.169.1".to_string(),
            None,
        );
        assert_eq!(cache.current(&client()).await, "0.169.1");
    }
}
