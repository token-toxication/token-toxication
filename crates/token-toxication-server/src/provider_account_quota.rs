//! Client-facing view of upstream provider account quota.
//!
//! `GET /provider-accounts/quota` lets any valid relay API key read the upstream
//! quota of every active provider account. Only Codex subscription and
//! Antigravity accounts support upstream quota queries; other accounts are
//! listed with `quotaSupported = false`.
//!
//! Queries are read-only with respect to routing: they never write
//! `provider_limits` or change account health. Results are cached in memory
//! per account for `CACHE_TTL` so polling clients do not hammer upstream usage
//! endpoints. The cache also receives every successful fetch made elsewhere
//! (capacity sampling, quota rechecks, admin queries) through `remember_codex`
//! and `remember_gemini`.

use std::{
    collections::HashMap,
    sync::{Mutex, OnceLock},
    time::Duration,
};

use axum::{
    Json,
    extract::State,
    http::{HeaderMap, Uri},
};
use chrono::{DateTime, Utc};
use futures_util::future::join_all;
use serde::{Deserialize, Serialize};
use tokio::time::{Instant, timeout};

use crate::{
    AppState,
    antigravity_oauth::gemini_account_quota,
    codex_subscription::{fetch_codex_account_quota, is_codex_subscription_auth},
    error::AppError,
    gemini_code_assist::is_antigravity_oauth_auth,
    models::{CodexAccountQuotaResponse, GeminiAccountQuotaResponse, ProviderAccount},
    relay_attempt::authenticate_relay_api_key,
};

const CACHE_TTL: Duration = Duration::from_secs(60);
const UPSTREAM_TIMEOUT: Duration = Duration::from_secs(20);

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderAccountQuotaListResponse {
    pub data: Vec<ProviderAccountQuota>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderAccountQuota {
    pub id: String,
    pub name: String,
    pub provider: String,
    pub auth_mode: String,
    pub status: String,
    pub quota_supported: bool,
    /// When the quota was fetched from upstream; `None` when not queried.
    pub observed_at: Option<DateTime<Utc>>,
    /// Same body as `GET /admin/api/provider-accounts/{id}/codex/quota`.
    pub codex_quota: Option<CodexAccountQuotaResponse>,
    /// Same body as `GET /admin/api/provider-accounts/{id}/gemini/quota`.
    pub gemini_quota: Option<GeminiAccountQuotaResponse>,
    pub quota_error: Option<String>,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum QuotaKind {
    Codex,
    Gemini,
}

fn quota_kind(account: &ProviderAccount) -> Option<QuotaKind> {
    if is_codex_subscription_auth(&account.auth_mode) {
        Some(QuotaKind::Codex)
    } else if account.provider == "gemini" && is_antigravity_oauth_auth(&account.auth_mode) {
        Some(QuotaKind::Gemini)
    } else {
        None
    }
}

fn base_view(account: &ProviderAccount) -> ProviderAccountQuota {
    ProviderAccountQuota {
        id: account.id.clone(),
        name: account.name.clone(),
        provider: account.provider.clone(),
        auth_mode: account.auth_mode.clone(),
        status: account.status.clone(),
        quota_supported: quota_kind(account).is_some(),
        observed_at: None,
        codex_quota: None,
        gemini_quota: None,
        quota_error: None,
    }
}

/// Last upstream quota result per account, shared by every caller of
/// `fetch_codex_account_quota` and `gemini_account_quota` (admin queries,
/// capacity sampling, quota rechecks and this endpoint).
struct CacheEntry {
    stored_at: Instant,
    observed_at: DateTime<Utc>,
    codex_quota: Option<CodexAccountQuotaResponse>,
    gemini_quota: Option<GeminiAccountQuotaResponse>,
    quota_error: Option<String>,
}

type Cache = Mutex<HashMap<String, CacheEntry>>;

fn cache() -> &'static Cache {
    static CACHE: OnceLock<Cache> = OnceLock::new();
    CACHE.get_or_init(Default::default)
}

fn store(
    account_id: &str,
    codex_quota: Option<CodexAccountQuotaResponse>,
    gemini_quota: Option<GeminiAccountQuotaResponse>,
    quota_error: Option<String>,
) {
    if let Ok(mut cache) = cache().lock() {
        cache.retain(|_, entry| entry.stored_at.elapsed() < CACHE_TTL);
        cache.insert(
            account_id.to_string(),
            CacheEntry {
                stored_at: Instant::now(),
                observed_at: Utc::now(),
                codex_quota,
                gemini_quota,
                quota_error,
            },
        );
    }
}

/// Record a successful Codex quota fetch and pass it through.
pub(crate) fn remember_codex(quota: CodexAccountQuotaResponse) -> CodexAccountQuotaResponse {
    store(&quota.account_id, Some(quota.clone()), None, None);
    quota
}

/// Record a successful Antigravity quota fetch and pass it through.
pub(crate) fn remember_gemini(quota: GeminiAccountQuotaResponse) -> GeminiAccountQuotaResponse {
    store(&quota.account_id, None, Some(quota.clone()), None);
    quota
}

/// Fill `view` from a fresh cache entry; returns false on a miss.
fn fill_from_cache(view: &mut ProviderAccountQuota) -> bool {
    let Ok(cache) = cache().lock() else {
        return false;
    };
    let Some(entry) = cache
        .get(&view.id)
        .filter(|entry| entry.stored_at.elapsed() < CACHE_TTL)
    else {
        return false;
    };
    view.observed_at = Some(entry.observed_at);
    view.codex_quota = entry.codex_quota.clone();
    view.gemini_quota = entry.gemini_quota.clone();
    view.quota_error = entry.quota_error.clone();
    true
}

async fn account_quota(state: &AppState, account: &ProviderAccount) -> ProviderAccountQuota {
    let mut view = base_view(account);
    let Some(kind) = quota_kind(account) else {
        return view;
    };
    if account.status == "blocked" || fill_from_cache(&mut view) {
        return view;
    }

    // Successful fetches are cached by the fetch functions themselves.
    let result = match kind {
        QuotaKind::Codex => timeout(
            UPSTREAM_TIMEOUT,
            fetch_codex_account_quota(
                &state.db,
                &state.http,
                &state.codex_device_oauth,
                &account.id,
            ),
        )
        .await
        .map(|result| result.map(|quota| view.codex_quota = Some(quota))),
        QuotaKind::Gemini => timeout(
            UPSTREAM_TIMEOUT,
            gemini_account_quota(&state.db, &state.gemini_http, &account.id),
        )
        .await
        .map(|result| result.map(|quota| view.gemini_quota = Some(quota))),
    };
    let error = match result {
        Ok(Ok(())) => None,
        Ok(Err(error)) => Some(error.to_string()),
        Err(_) => Some("upstream quota query timed out".to_string()),
    };
    if let Some(error) = &error {
        store(&account.id, None, None, Some(error.clone()));
    }
    view.quota_error = error;
    view.observed_at = Some(Utc::now());
    // Report the cached timestamp so repeated reads show the same `observedAt`.
    fill_from_cache(&mut view);
    view
}

pub async fn list_provider_account_quota(
    State(state): State<AppState>,
    headers: HeaderMap,
    uri: Uri,
) -> Result<Json<ProviderAccountQuotaListResponse>, AppError> {
    authenticate_relay_api_key(&state, &headers, uri.query()).await?;
    let accounts: Vec<_> = state
        .db
        .list_provider_accounts()
        .await?
        .into_iter()
        .filter(|account| account.is_active)
        .collect();
    let data = join_all(
        accounts
            .iter()
            .map(|account| account_quota(&state, account)),
    )
    .await;
    Ok(Json(ProviderAccountQuotaListResponse { data }))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn account(provider: &str, auth_mode: &str) -> ProviderAccount {
        ProviderAccount {
            id: "account-1".to_string(),
            name: "account".to_string(),
            provider: provider.to_string(),
            base_url: "https://upstream.example".to_string(),
            auth_mode: auth_mode.to_string(),
            wire_api: "openai-responses".to_string(),
            is_active: true,
            allow_credits: false,
            max_running_requests: 0,
            running_requests: 0,
            queued_requests: 0,
            status: "healthy".to_string(),
            last_error: None,
            created_at: Utc::now(),
            last_used_at: None,
        }
    }

    #[test]
    fn only_codex_and_antigravity_accounts_support_quota() {
        assert_eq!(
            quota_kind(&account("openai", "codex-device-oauth")),
            Some(QuotaKind::Codex)
        );
        assert_eq!(
            quota_kind(&account("openai", "codex-manual-refresh")),
            Some(QuotaKind::Codex)
        );
        assert_eq!(
            quota_kind(&account("gemini", "antigravity-oauth")),
            Some(QuotaKind::Gemini)
        );
        assert_eq!(quota_kind(&account("openai", "bearer")), None);
        assert!(!base_view(&account("anthropic", "x-api-key")).quota_supported);
    }

    #[test]
    fn view_does_not_expose_account_credentials() {
        let body = serde_json::to_value(base_view(&account("openai", "codex-device-oauth")))
            .expect("serialize view");
        let object = body.as_object().expect("object");
        for key in ["baseUrl", "apiKey", "lastError"] {
            assert!(!object.contains_key(key), "unexpected field {key}");
        }
        assert_eq!(body["quotaSupported"], true);
    }

    #[test]
    fn fetches_made_elsewhere_fill_the_cache() {
        let mut codex = account("openai", "codex-device-oauth");
        codex.id = uuid::Uuid::new_v4().to_string();
        let mut view = base_view(&codex);
        assert!(!fill_from_cache(&mut view));

        let quota: CodexAccountQuotaResponse = serde_json::from_value(serde_json::json!({
            "accountId": codex.id,
            "authMode": "codex-device-oauth",
            "endpoint": "https://chatgpt.com/backend-api/wham/usage",
            "planType": "pro",
            "limits": [],
        }))
        .expect("quota");
        remember_codex(quota);

        assert!(fill_from_cache(&mut view));
        assert!(view.observed_at.is_some());
        assert_eq!(
            view.codex_quota
                .as_ref()
                .and_then(|quota| quota.plan_type.as_deref()),
            Some("pro")
        );
        assert!(view.quota_error.is_none());
    }
}
