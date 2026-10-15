//! Fetching an account's own Codex-native model catalog.
//!
//! The relay proxies the account's catalog instead of authoring one, so the
//! instruction templates Codex requires come from the same source the Codex
//! client would use. Nothing here is persisted: the body is validated, filtered,
//! and returned.
//!
//! Security: the response body contains model instruction text. It is never
//! logged, echoed into errors, or stored; only the status code and byte length
//! are observable.

use axum::http::header;
use serde_json::Value;

use crate::{codex_subscription::CodexSubscriptionAuthorization, error::AppError};

/// Upper bound on a fetched catalog.
///
/// The Codex client bounds an explicitly configured catalog at 1 MiB. TT allows
/// headroom for the extra `-fast` entries it derives and rejects anything
/// larger instead of buffering without limit.
pub const MAX_CATALOG_BYTES: usize = 2 * 1024 * 1024;

/// Codex refreshes a catalog inside a 5 second budget.
const CATALOG_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(15);

/// A validated upstream catalog response.
#[derive(Debug)]
pub struct FetchedCatalog {
    pub body: Value,
    pub etag: Option<String>,
}

/// Fetch and decode the Codex-native catalog for one subscription account.
///
/// `client_version` is appended only when the caller supplied one, so TT never
/// invents a version string that would age.
pub async fn fetch(
    http: &aioduct::TokioClient,
    endpoint: &str,
    auth: &CodexSubscriptionAuthorization,
    client_version: Option<&str>,
) -> Result<FetchedCatalog, AppError> {
    let url = match client_version {
        Some(version) => {
            let separator = if endpoint.contains('?') { '&' } else { '?' };
            format!("{endpoint}{separator}client_version={version}")
        }
        None => endpoint.to_string(),
    };

    let mut request = http
        .get(&url)
        .map_err(AppError::Upstream)?
        .bearer_auth(&auth.access_token);
    if let Some(account_id) = auth.account_id.as_deref() {
        request = request
            .header_str("chatgpt-account-id", account_id)
            .map_err(AppError::Upstream)?;
    }
    let request = request
        .header_str("originator", auth.originator)
        .map_err(AppError::Upstream)?
        .header_str(header::ACCEPT.as_str(), "application/json")
        .map_err(AppError::Upstream)?;

    let response = tokio::time::timeout(CATALOG_TIMEOUT, request.send())
        .await
        .map_err(|_| AppError::ServiceUnavailable("codex catalog request timed out".into()))?
        .map_err(|error| AppError::Upstream(error.into()))?;

    let status = response.status();
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
    if bytes.len() > MAX_CATALOG_BYTES {
        return Err(AppError::ServiceUnavailable(format!(
            "codex catalog response exceeded {MAX_CATALOG_BYTES} bytes"
        )));
    }
    let body = serde_json::from_slice(&bytes)
        .map_err(|_| AppError::ServiceUnavailable("codex catalog response was not JSON".into()))?;

    Ok(FetchedCatalog { body, etag })
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

    async fn serve(
        response: (axum::http::StatusCode, &'static str),
    ) -> (String, Arc<Mutex<Vec<(String, HeaderMap)>>>) {
        let seen: Arc<Mutex<Vec<(String, HeaderMap)>>> = Arc::new(Mutex::new(Vec::new()));
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
            .timeout(std::time::Duration::from_secs(5))
            .read_timeout(std::time::Duration::from_secs(5))
            .build()
            .expect("build test HTTP client")
    }

    #[tokio::test]
    async fn sends_account_identity_and_appends_client_version_once() {
        let (endpoint, seen) = serve((axum::http::StatusCode::OK, r#"{"models":[]}"#)).await;
        let http = client();

        let fetched = fetch(&http, &endpoint, &authorization(), Some("0.162.1"))
            .await
            .expect("fetch catalog");
        assert_eq!(fetched.etag, None);
        assert_eq!(fetched.body["models"], serde_json::json!([]));

        let (uri, headers) = seen
            .lock()
            .expect("lock seen")
            .first()
            .cloned()
            .expect("captured request");
        assert_eq!(uri, "/codex/models?client_version=0.162.1");
        assert_eq!(headers["authorization"], "Bearer synthetic-access");
        assert_eq!(headers["chatgpt-account-id"], "synthetic-account");
        assert_eq!(headers["originator"], "opencode");
    }

    #[tokio::test]
    async fn omits_client_version_when_the_caller_supplies_none() {
        let (endpoint, seen) = serve((axum::http::StatusCode::OK, r#"{"models":[]}"#)).await;
        let http = client();

        fetch(&http, &endpoint, &authorization(), None)
            .await
            .expect("fetch catalog");

        let (uri, _) = seen
            .lock()
            .expect("lock seen")
            .first()
            .cloned()
            .expect("captured request");
        assert_eq!(uri, "/codex/models");
    }

    #[tokio::test]
    async fn upstream_failure_reports_status_without_echoing_the_body() {
        let (endpoint, _) = serve((
            axum::http::StatusCode::UNAUTHORIZED,
            r#"{"error":"leaked-instruction-text"}"#,
        ))
        .await;
        let http = client();

        let error = fetch(&http, &endpoint, &authorization(), None)
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
    async fn malformed_json_is_reported_as_unavailable() {
        let (endpoint, _) = serve((axum::http::StatusCode::OK, "not json")).await;
        let http = client();

        let error = fetch(&http, &endpoint, &authorization(), None)
            .await
            .expect_err("malformed body");
        assert!(error.to_string().contains("was not JSON"));
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
            None,
        )
        .await
        .expect("fetch catalog");
        assert_eq!(fetched.etag.as_deref(), Some("W/\"abc\""));
    }
}
