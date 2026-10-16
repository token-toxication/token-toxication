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
//!   catalog. TT therefore always sends one, falling back to
//!   [`FALLBACK_CLIENT_VERSION`] when the client sent nothing usable.
//! - The catalog is shaped **by version**: an older or unknown value returns a
//!   smaller set. The version is therefore part of the cache key, and never a
//!   constant that is not a real release.
//! - The Codex client abandons its catalog refresh after 5 seconds. TT's own
//!   budget stays well inside that so a slow upstream cannot make the client
//!   time out and lose its model list.

use std::time::Duration;

use axum::http::header;

use crate::{codex_subscription::CodexSubscriptionAuthorization, error::AppError};

/// Upper bound on an upstream catalog body.
///
/// Larger than the client-side cap on purpose: TT reads the whole document so it
/// can decide what fits, instead of forwarding a truncated body.
pub const MAX_UPSTREAM_CATALOG_BYTES: usize = 4 * 1024 * 1024;

/// TT's upstream budget, comfortably inside the client's 5 second refresh.
const CATALOG_TIMEOUT: Duration = Duration::from_secs(3);

/// Version TT reports upstream when the client sent nothing usable.
///
/// The backend shapes the catalog to the requested version, so a placeholder
/// such as `0.0.0` is not neutral: it yields a **reduced** catalog that is
/// missing models newer clients can use. TT therefore reports the current Codex
/// release, which is what a client that sends no version most likely is.
///
/// Keep this current. Latest verified `2026-10-16` from `~/.codex/version.json`
/// (`latest_version`) and `https://registry.npmjs.org/@openai/codex/latest`.
pub const FALLBACK_CLIENT_VERSION: &str = "0.163.0";

/// Longest client version TT forwards.
const MAX_CLIENT_VERSION_LEN: usize = 32;

/// A catalog response, or a `304` revalidation result.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FetchedCatalog {
    /// The upstream returned a fresh body.
    Ok { body: Vec<u8>, etag: Option<String> },
    /// The cached copy is still current.
    NotModified,
}

/// Reduce an untrusted client version to something safe to place in a query.
///
/// Never rejects: a client with an unexpected version string still needs a
/// catalog, and losing every model is worse than reporting a placeholder.
pub fn upstream_client_version(value: Option<&str>) -> String {
    let sanitized: String = value
        .unwrap_or_default()
        .trim()
        .chars()
        .filter(|character| {
            character.is_ascii_alphanumeric() || matches!(character, '.' | '-' | '_')
        })
        .take(MAX_CLIENT_VERSION_LEN)
        .collect();
    if sanitized.is_empty() {
        FALLBACK_CLIENT_VERSION.to_string()
    } else {
        sanitized
    }
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
    fn the_fallback_version_is_a_real_release() {
        // A placeholder such as `0.0.0` is not neutral: the backend shapes the
        // catalog by version and returns a smaller set for an unknown one, which
        // is how a single versionless request used to downgrade every client.
        let parts: Vec<&str> = FALLBACK_CLIENT_VERSION.split('.').collect();
        assert_eq!(parts.len(), 3, "expected major.minor.patch");
        for part in &parts {
            assert!(
                part.parse::<u32>().is_ok(),
                "fallback version component {part:?} is not numeric"
            );
        }
        assert_ne!(FALLBACK_CLIENT_VERSION, "0.0.0");
        assert!(
            parts.iter().any(|part| *part != "0"),
            "fallback version must name a real release"
        );
    }

    #[test]
    fn a_client_version_is_always_usable() {
        // The backend rejects an absent parameter, so this must never be empty.
        assert_eq!(upstream_client_version(None), FALLBACK_CLIENT_VERSION);
        assert_eq!(FALLBACK_CLIENT_VERSION, "0.163.0");
        assert_eq!(upstream_client_version(Some("")), FALLBACK_CLIENT_VERSION);
        assert_eq!(
            upstream_client_version(Some("   ")),
            FALLBACK_CLIENT_VERSION
        );
        assert_eq!(
            upstream_client_version(Some("0.162.0-alpha.17.2")),
            "0.162.0-alpha.17.2"
        );
        // Unexpected input is sanitized rather than rejected: a client with an
        // odd version string still needs its model list.
        assert_eq!(
            upstream_client_version(Some("1.2.3+build 4")),
            "1.2.3build4"
        );
        assert_eq!(
            upstream_client_version(Some("../../etc/passwd")),
            "....etcpasswd"
        );
        assert_eq!(
            upstream_client_version(Some(&"9".repeat(80))).len(),
            MAX_CLIENT_VERSION_LEN
        );
    }
}
