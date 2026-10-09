use std::{collections::HashMap, sync::Arc};

use aioduct::TokioClient;
use axum::http::{HeaderValue, StatusCode, header};
use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use chrono::{DateTime, Duration, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tokio::sync::Mutex;
use uuid::Uuid;

use crate::{
    auth::hash_secret,
    codex_subscription::{CODEX_DEVICE_OAUTH_AUTH_MODE, CodexSubscriptionAuthorization},
    db::Db,
    error::AppError,
    models::{
        CodexDeviceOAuthStartResponse, CodexDeviceOAuthStatusResponse, CreateProviderAccountRequest,
    },
};

const CODEX_ISSUER: &str = "https://auth.openai.com";
const CODEX_API_ACCOUNTS: &str = "https://auth.openai.com/api/accounts";
const CODEX_CLIENT_ID: &str = "app_EMoamEEZ73f0CkXaXp7hrann";
const CODEX_DEVICE_URL: &str = "https://chatgpt.com/codex/device";
const CODEX_REDIRECT_URI: &str = "https://auth.openai.com/deviceauth/callback";
const FLOW_TTL_MINUTES: i64 = 15;
const DEFAULT_POLL_INTERVAL_SECONDS: u64 = 5;
const REFRESH_SAFETY_MARGIN_SECONDS: i64 = 300;
const POLLING_CLEANUP_GRACE_MINUTES: i64 = 5;
const STORED_CREDENTIAL_TYPE: &str = "token-toxication-codex-device-oauth-v1";

#[derive(Clone, Default)]
pub struct CodexDeviceOAuthStore {
    flows: Arc<Mutex<HashMap<String, DeviceFlow>>>,
    refresh_locks: Arc<Mutex<HashMap<String, Arc<Mutex<()>>>>>,
}

#[derive(Debug, Clone)]
struct DeviceFlow {
    owner_hash: String,
    name: String,
    device_auth_id: String,
    user_code: String,
    poll_interval_seconds: u64,
    expires_at: DateTime<Utc>,
    next_poll_at: DateTime<Utc>,
    polling: bool,
    status: FlowStatus,
    account_id: Option<String>,
    error: Option<String>,
}

#[derive(Debug, Clone)]
struct FlowSnapshot {
    flow_id: String,
    expires_at: DateTime<Utc>,
    next_poll_at: Option<DateTime<Utc>>,
    status: FlowStatus,
    account_id: Option<String>,
    error: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum FlowStatus {
    Pending,
    Succeeded,
    Failed,
    Expired,
}

impl FlowStatus {
    fn as_str(self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Succeeded => "succeeded",
            Self::Failed => "failed",
            Self::Expired => "expired",
        }
    }
}

#[derive(Debug, Deserialize)]
struct UserCodeResponse {
    device_auth_id: String,
    #[serde(alias = "usercode")]
    user_code: String,
    #[serde(default, deserialize_with = "deserialize_interval")]
    interval: u64,
}

#[derive(Debug, Serialize)]
struct UserCodeRequest<'a> {
    client_id: &'a str,
}

#[derive(Debug, Serialize)]
struct TokenPollRequest<'a> {
    device_auth_id: &'a str,
    user_code: &'a str,
}

#[derive(Debug, Deserialize)]
struct AuthorizationCodeResponse {
    authorization_code: String,
    code_verifier: String,
}

#[derive(Debug, Deserialize)]
struct TokenResponse {
    #[serde(default)]
    id_token: Option<String>,
    #[serde(default)]
    access_token: Option<String>,
    #[serde(default)]
    refresh_token: Option<String>,
    #[serde(default)]
    expires_in: Option<i64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct StoredCredential {
    r#type: String,
    refresh: String,
    #[serde(default)]
    access: Option<String>,
    #[serde(default, rename = "expiresAt")]
    expires_at: Option<i64>,
    #[serde(default, rename = "accountId")]
    account_id: Option<String>,
    #[serde(default)]
    issuer: Option<String>,
}

#[derive(Debug, Deserialize)]
struct JwtClaims {
    exp: Option<i64>,
    chatgpt_account_id: Option<String>,
    #[serde(rename = "https://api.openai.com/auth")]
    namespaced_auth: Option<JwtAuthClaims>,
    auth: Option<JwtAuthClaims>,
}

#[derive(Debug, Deserialize)]
struct JwtAuthClaims {
    chatgpt_account_id: Option<String>,
}

pub async fn begin(
    store: &CodexDeviceOAuthStore,
    http: &TokioClient,
    owner_token: &str,
    name: String,
) -> Result<CodexDeviceOAuthStartResponse, AppError> {
    let usercode_url = format!("{CODEX_API_ACCOUNTS}/deviceauth/usercode");
    let usercode_body = serde_json::to_vec(&UserCodeRequest {
        client_id: CODEX_CLIENT_ID,
    })
    .map_err(|error| AppError::Internal(format!("serialize Codex device request: {error}")))?;
    let response = http
        .post(&usercode_url)?
        .header(
            header::CONTENT_TYPE,
            HeaderValue::from_static("application/json"),
        )
        .body(usercode_body)
        .send()
        .await
        .map_err(|error| AppError::Upstream(error.into()))?;
    let status = response.status();
    if !status.is_success() {
        return Err(device_upstream_error("Codex device login", status));
    }
    let bytes = response.bytes().await.map_err(AppError::Upstream)?;
    let response: UserCodeResponse = serde_json::from_slice(&bytes).map_err(|error| {
        AppError::Internal(format!("invalid Codex device login response: {error}"))
    })?;
    let now = Utc::now();
    let expires_at = now + Duration::minutes(FLOW_TTL_MINUTES);
    let poll_interval_seconds = response.interval.clamp(1, 60);
    let flow_id = Uuid::new_v4().to_string();
    let flow = DeviceFlow {
        owner_hash: hash_secret(owner_token),
        name: name.trim().to_string(),
        device_auth_id: response.device_auth_id,
        user_code: response.user_code.clone(),
        poll_interval_seconds,
        expires_at,
        next_poll_at: now,
        polling: false,
        status: FlowStatus::Pending,
        account_id: None,
        error: None,
    };
    let mut flows = store.flows.lock().await;
    flows.retain(|_, flow| retain_flow(flow, now));
    flows.insert(flow_id.clone(), flow);
    Ok(CodexDeviceOAuthStartResponse {
        flow_id,
        verification_url: CODEX_DEVICE_URL.to_string(),
        user_code: response.user_code,
        expires_at,
        poll_interval_seconds,
        status: FlowStatus::Pending.as_str().to_string(),
    })
}

pub async fn status(
    store: &CodexDeviceOAuthStore,
    db: &Db,
    http: &TokioClient,
    owner_token: &str,
    flow_id: &str,
) -> Result<CodexDeviceOAuthStatusResponse, AppError> {
    let owner_hash = hash_secret(owner_token);
    let (device_auth_id, user_code, flow_expires_at) = {
        let mut flows = store.flows.lock().await;
        let flow = flows
            .get_mut(flow_id)
            .ok_or_else(|| AppError::NotFound("Codex device flow not found".into()))?;
        if flow.owner_hash != owner_hash {
            return Err(AppError::NotFound("Codex device flow not found".into()));
        }
        if flow.status == FlowStatus::Pending && flow.expires_at <= Utc::now() {
            flow.status = FlowStatus::Expired;
            flow.error = Some("Codex device login expired; start again".into());
        }
        if flow.status != FlowStatus::Pending || flow.polling || flow.next_poll_at > Utc::now() {
            let snapshot = flow_snapshot(flow_id, flow);
            drop(flows);
            return flow_response(db, snapshot).await;
        }
        flow.polling = true;
        flow.next_poll_at = Utc::now() + Duration::seconds(flow.poll_interval_seconds as i64);
        (
            flow.device_auth_id.clone(),
            flow.user_code.clone(),
            flow.expires_at,
        )
    };

    let poll = poll_device_code(http, &device_auth_id, &user_code).await;
    match poll {
        Ok(None) => {
            let mut flows = store.flows.lock().await;
            let Some(flow) = flows.get_mut(flow_id) else {
                return Err(AppError::NotFound(
                    "Codex device flow no longer exists".into(),
                ));
            };
            flow.polling = false;
            let now = Utc::now();
            if now >= flow.expires_at {
                flow.status = FlowStatus::Expired;
                flow.error = Some("Codex device login expired; start again".into());
            } else {
                flow.next_poll_at = now + Duration::seconds(flow.poll_interval_seconds as i64);
            }
            let snapshot = flow_snapshot(flow_id, flow);
            drop(flows);
            flow_response(db, snapshot).await
        }
        Ok(Some(code)) => match exchange_code(http, &code).await {
            Ok(credential) => {
                let flow_name = flow_name(store, flow_id).await;
                let account = match db
                    .create_provider_account(CreateProviderAccountRequest {
                        name: if flow_name.is_empty() {
                            "Codex Account".into()
                        } else {
                            flow_name
                        },
                        provider: "codex-subscription".into(),
                        base_url: "https://chatgpt.com/backend-api".into(),
                        auth_mode: CODEX_DEVICE_OAUTH_AUTH_MODE.into(),
                        wire_api: "openai-responses".into(),
                        api_key: credential.secret,
                        is_active: true,
                        max_running_requests: 0,
                    })
                    .await
                {
                    Ok(account) => account,
                    Err(error) => {
                        tracing::error!(
                            flow_id,
                            error = %error,
                            "failed to create Codex provider account after device OAuth"
                        );
                        return finish_failed(
                            store,
                            db,
                            flow_id,
                            AppError::Internal("failed to create Codex provider account".into()),
                        )
                        .await;
                    }
                };
                let mut flows = store.flows.lock().await;
                let Some(flow) = flows.get_mut(flow_id) else {
                    return Ok(CodexDeviceOAuthStatusResponse {
                        flow_id: flow_id.to_string(),
                        status: FlowStatus::Succeeded.as_str().to_string(),
                        expires_at: flow_expires_at,
                        next_poll_at: None,
                        account: Some(account),
                        error: None,
                    });
                };
                flow.polling = false;
                flow.status = FlowStatus::Succeeded;
                flow.account_id = Some(account.id.clone());
                flow.error = None;
                let snapshot = flow_snapshot(flow_id, flow);
                drop(flows);
                flow_response(db, snapshot).await
            }
            Err(error) => finish_failed(store, db, flow_id, error).await,
        },
        Err(error) => finish_failed(store, db, flow_id, error).await,
    }
}

async fn finish_failed(
    store: &CodexDeviceOAuthStore,
    db: &Db,
    flow_id: &str,
    error: AppError,
) -> Result<CodexDeviceOAuthStatusResponse, AppError> {
    let mut flows = store.flows.lock().await;
    let Some(flow) = flows.get_mut(flow_id) else {
        return Err(AppError::NotFound(
            "Codex device flow no longer exists".into(),
        ));
    };
    flow.polling = false;
    flow.status = FlowStatus::Failed;
    flow.error = Some(error.to_string());
    let snapshot = flow_snapshot(flow_id, flow);
    drop(flows);
    flow_response(db, snapshot).await
}

async fn flow_response(
    db: &Db,
    snapshot: FlowSnapshot,
) -> Result<CodexDeviceOAuthStatusResponse, AppError> {
    let account = match snapshot.account_id.as_deref() {
        Some(id) => db.get_provider_account(id).await?,
        None => None,
    };
    Ok(CodexDeviceOAuthStatusResponse {
        flow_id: snapshot.flow_id,
        status: snapshot.status.as_str().to_string(),
        expires_at: snapshot.expires_at,
        next_poll_at: snapshot.next_poll_at,
        account,
        error: snapshot.error,
    })
}

fn flow_snapshot(flow_id: &str, flow: &DeviceFlow) -> FlowSnapshot {
    FlowSnapshot {
        flow_id: flow_id.to_string(),
        expires_at: flow.expires_at,
        next_poll_at: (flow.status == FlowStatus::Pending).then_some(flow.next_poll_at),
        status: flow.status,
        account_id: flow.account_id.clone(),
        error: flow.error.clone(),
    }
}

fn retain_flow(flow: &DeviceFlow, now: DateTime<Utc>) -> bool {
    if flow.polling {
        return flow.expires_at + Duration::minutes(POLLING_CLEANUP_GRACE_MINUTES) > now;
    }
    flow.expires_at > now && flow.status == FlowStatus::Pending
}

async fn flow_name(store: &CodexDeviceOAuthStore, flow_id: &str) -> String {
    store
        .flows
        .lock()
        .await
        .get(flow_id)
        .map(|flow| flow.name.clone())
        .unwrap_or_default()
}

async fn poll_device_code(
    http: &TokioClient,
    device_auth_id: &str,
    user_code: &str,
) -> Result<Option<AuthorizationCodeResponse>, AppError> {
    let poll_url = format!("{CODEX_API_ACCOUNTS}/deviceauth/token");
    let poll_body = serde_json::to_vec(&TokenPollRequest {
        device_auth_id,
        user_code,
    })
    .map_err(|error| AppError::Internal(format!("serialize Codex device poll: {error}")))?;
    let response = http
        .post(&poll_url)?
        .header(
            header::CONTENT_TYPE,
            HeaderValue::from_static("application/json"),
        )
        .body(poll_body)
        .send()
        .await
        .map_err(|error| AppError::Upstream(error.into()))?;
    let status = response.status();
    if status == StatusCode::FORBIDDEN || status == StatusCode::NOT_FOUND {
        return Ok(None);
    }
    if !status.is_success() {
        return Err(device_upstream_error("Codex device token poll", status));
    }
    let bytes = response.bytes().await.map_err(AppError::Upstream)?;
    serde_json::from_slice(&bytes).map(Some).map_err(|error| {
        AppError::Internal(format!("invalid Codex device token response: {error}"))
    })
}

#[derive(Debug)]
struct NewCredential {
    secret: String,
}

async fn exchange_code(
    http: &TokioClient,
    code: &AuthorizationCodeResponse,
) -> Result<NewCredential, AppError> {
    let token_url = format!("{CODEX_ISSUER}/oauth/token");
    let response = http
        .post(&token_url)?
        .form(&[
            ("grant_type", "authorization_code"),
            ("client_id", CODEX_CLIENT_ID),
            ("code", code.authorization_code.as_str()),
            ("redirect_uri", CODEX_REDIRECT_URI),
            ("code_verifier", code.code_verifier.as_str()),
        ])
        .send()
        .await
        .map_err(|error| AppError::Upstream(error.into()))?;
    let status = response.status();
    if !status.is_success() {
        return Err(device_upstream_error("Codex OAuth token exchange", status));
    }
    let bytes = response.bytes().await.map_err(AppError::Upstream)?;
    let tokens: TokenResponse = serde_json::from_slice(&bytes).map_err(|error| {
        AppError::Internal(format!("invalid Codex OAuth token response: {error}"))
    })?;
    let access = tokens.access_token.ok_or_else(|| {
        AppError::Unauthorized("Codex OAuth response is missing an access token".into())
    })?;
    let refresh = tokens.refresh_token.ok_or_else(|| {
        AppError::Unauthorized("Codex OAuth response is missing a refresh token".into())
    })?;
    let account_id = tokens
        .id_token
        .as_deref()
        .and_then(account_id_from_jwt)
        .or_else(|| account_id_from_jwt(&access))
        .ok_or_else(|| {
            AppError::Unauthorized("Codex OAuth response is missing a ChatGPT account id".into())
        })?;
    let expires_at = tokens
        .expires_in
        .map(|seconds| Utc::now().timestamp() + seconds)
        .or_else(|| tokens.id_token.as_deref().and_then(exp_from_jwt))
        .or_else(|| exp_from_jwt(&access));
    let secret = serde_json::to_string(&StoredCredential {
        r#type: STORED_CREDENTIAL_TYPE.into(),
        refresh,
        access: Some(access),
        expires_at,
        account_id: Some(account_id),
        issuer: None,
    })
    .map_err(|error| AppError::Internal(format!("serialize Codex credential: {error}")))?;
    Ok(NewCredential { secret })
}

pub async fn authorization(
    store: &CodexDeviceOAuthStore,
    db: &Db,
    http: &TokioClient,
    account: &crate::models::ProviderAccountRecord,
) -> Result<CodexSubscriptionAuthorization, AppError> {
    let lock = {
        let mut locks = store.refresh_locks.lock().await;
        locks
            .entry(account.account.id.clone())
            .or_insert_with(|| Arc::new(Mutex::new(())))
            .clone()
    };
    let _guard = lock.lock().await;
    let current = db
        .get_provider_account_record(&account.account.id)
        .await?
        .ok_or_else(|| AppError::NotFound("provider account not found".into()))?;
    let mut credential: StoredCredential = serde_json::from_str(&current.api_key)
        .map_err(|_| AppError::BadRequest("Codex device OAuth credential is invalid".into()))?;
    if credential.r#type != STORED_CREDENTIAL_TYPE || credential.refresh.trim().is_empty() {
        return Err(AppError::BadRequest(
            "Codex device OAuth credential is invalid".into(),
        ));
    }
    let endpoint =
        crate::codex_subscription::codex_subscription_endpoint(&current.account.base_url)?;
    if credential
        .access
        .as_deref()
        .is_some_and(|access| !access.trim().is_empty())
        && credential
            .expires_at
            .is_some_and(|expires| expires > Utc::now().timestamp() + REFRESH_SAFETY_MARGIN_SECONDS)
    {
        let account_id = credential.account_id.clone().ok_or_else(|| {
            AppError::Unauthorized(
                "Codex device OAuth credential is missing a ChatGPT account id".into(),
            )
        })?;
        return Ok(CodexSubscriptionAuthorization {
            access_token: credential.access.unwrap_or_default(),
            account_id: Some(account_id),
            endpoint,
            originator: crate::codex_subscription::CODEX_DEVICE_ORIGINATOR,
        });
    }
    let token_url = format!(
        "{}/oauth/token",
        credential.issuer.as_deref().unwrap_or(CODEX_ISSUER)
    );
    let token_body = serde_json::to_vec(&serde_json::json!({
        "grant_type": "refresh_token",
        "client_id": CODEX_CLIENT_ID,
        "refresh_token": credential.refresh,
    }))
    .map_err(|error| AppError::Internal(format!("serialize Codex refresh request: {error}")))?;
    let response = http
        .post(&token_url)?
        .header(
            header::CONTENT_TYPE,
            HeaderValue::from_static("application/json"),
        )
        .body(token_body)
        .send()
        .await
        .map_err(|error| AppError::Upstream(error.into()))?;
    let status = response.status();
    if !status.is_success() {
        return Err(
            if matches!(
                status,
                StatusCode::BAD_REQUEST | StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN
            ) {
                AppError::Unauthorized("Codex device OAuth refresh was rejected".into())
            } else {
                device_upstream_error("Codex device OAuth refresh", status)
            },
        );
    }
    let bytes = response.bytes().await.map_err(AppError::Upstream)?;
    let tokens: TokenResponse = serde_json::from_slice(&bytes).map_err(|error| {
        AppError::Internal(format!("invalid Codex OAuth refresh response: {error}"))
    })?;
    let access = tokens.access_token.ok_or_else(|| {
        AppError::Unauthorized("Codex device OAuth refresh did not return an access token".into())
    })?;
    credential.access = Some(access.clone());
    if let Some(refresh) = tokens.refresh_token {
        credential.refresh = refresh;
    }
    let refreshed_account_id = tokens
        .id_token
        .as_deref()
        .and_then(account_id_from_jwt)
        .or_else(|| account_id_from_jwt(&access));
    if let (Some(previous), Some(current)) = (
        credential.account_id.as_deref(),
        refreshed_account_id.as_deref(),
    ) && previous != current
    {
        return Err(AppError::Unauthorized(
            "Codex device OAuth refresh returned a different ChatGPT account".into(),
        ));
    }
    credential.account_id = refreshed_account_id.or(credential.account_id);
    let account_id = credential.account_id.clone().ok_or_else(|| {
        AppError::Unauthorized("Codex device OAuth refresh is missing a ChatGPT account id".into())
    })?;
    credential.expires_at = tokens
        .expires_in
        .map(|seconds| Utc::now().timestamp() + seconds)
        .or_else(|| exp_from_jwt(&access));
    let serialized = serde_json::to_string(&credential)
        .map_err(|error| AppError::Internal(format!("serialize Codex credential: {error}")))?;
    db.update_provider_account_secret(&current.account.id, &serialized)
        .await?;
    Ok(CodexSubscriptionAuthorization {
        access_token: access,
        account_id: Some(account_id),
        endpoint,
        originator: crate::codex_subscription::CODEX_DEVICE_ORIGINATOR,
    })
}

fn device_upstream_error(operation: &str, status: StatusCode) -> AppError {
    if status == StatusCode::NOT_FOUND {
        AppError::BadRequest(format!("{operation} is not enabled by the Codex server"))
    } else if status.is_client_error() {
        AppError::Unauthorized(format!("{operation} was rejected by the Codex server"))
    } else {
        AppError::Internal(format!("{operation} failed with upstream status {status}"))
    }
}

fn deserialize_interval<'de, D>(deserializer: D) -> Result<u64, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let value = Value::deserialize(deserializer)?;
    Ok(match value {
        Value::Number(number) => number.as_u64().unwrap_or(DEFAULT_POLL_INTERVAL_SECONDS),
        Value::String(value) => value
            .trim()
            .parse()
            .unwrap_or(DEFAULT_POLL_INTERVAL_SECONDS),
        _ => DEFAULT_POLL_INTERVAL_SECONDS,
    })
}

fn account_id_from_jwt(token: &str) -> Option<String> {
    let claims = jwt_claims(token)?;
    claims
        .chatgpt_account_id
        .or_else(|| claims.auth.and_then(|auth| auth.chatgpt_account_id))
        .or_else(|| {
            claims
                .namespaced_auth
                .and_then(|auth| auth.chatgpt_account_id)
        })
}

fn exp_from_jwt(token: &str) -> Option<i64> {
    jwt_claims(token)?.exp
}

fn jwt_claims(token: &str) -> Option<JwtClaims> {
    let payload = token.split('.').nth(1)?;
    let bytes = URL_SAFE_NO_PAD
        .decode(payload)
        .or_else(|_| base64::engine::general_purpose::URL_SAFE.decode(payload))
        .ok()?;
    serde_json::from_slice(&bytes).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extracts_account_and_expiration_from_nested_claims() {
        let payload = URL_SAFE_NO_PAD.encode(
            serde_json::to_vec(&serde_json::json!({
                "exp": 1_800_000_000,
                "https://api.openai.com/auth": {"chatgpt_account_id": "account-123"}
            }))
            .expect("serialize claims"),
        );
        let token = format!("header.{payload}.signature");
        assert_eq!(account_id_from_jwt(&token).as_deref(), Some("account-123"));
        assert_eq!(exp_from_jwt(&token), Some(1_800_000_000));
    }

    #[test]
    fn interval_accepts_numeric_and_string_values() {
        let numeric: UserCodeResponse = serde_json::from_value(serde_json::json!({
            "device_auth_id": "device",
            "user_code": "code",
            "interval": 7
        }))
        .expect("parse numeric interval");
        let string: UserCodeResponse = serde_json::from_value(serde_json::json!({
            "device_auth_id": "device",
            "user_code": "code",
            "interval": "9"
        }))
        .expect("parse string interval");
        assert_eq!(numeric.interval, 7);
        assert_eq!(string.interval, 9);
    }

    #[test]
    fn expired_polling_flows_are_retained_during_cleanup() {
        let now = Utc::now();
        let flow = DeviceFlow {
            owner_hash: "owner".into(),
            name: "Codex".into(),
            device_auth_id: "device".into(),
            user_code: "code".into(),
            poll_interval_seconds: 5,
            expires_at: now - Duration::seconds(1),
            next_poll_at: now,
            polling: true,
            status: FlowStatus::Pending,
            account_id: None,
            error: None,
        };
        assert!(retain_flow(&flow, now));

        let mut abandoned = flow;
        abandoned.expires_at =
            now - Duration::minutes(POLLING_CLEANUP_GRACE_MINUTES) - Duration::seconds(1);
        assert!(!retain_flow(&abandoned, now));

        let mut completed = abandoned;
        completed.polling = false;
        assert!(!retain_flow(&completed, now));
    }

    #[tokio::test]
    async fn finishing_a_missing_flow_returns_not_found_instead_of_panicking() {
        let path =
            std::env::temp_dir().join(format!("token-toxication-{}.sqlite3", Uuid::new_v4()));
        let db = Db::open(&path).await.expect("open test database");
        let error = finish_failed(
            &CodexDeviceOAuthStore::default(),
            &db,
            "missing-flow",
            AppError::Internal("synthetic failure".into()),
        )
        .await
        .expect_err("missing flow should be reported");
        assert!(matches!(error, AppError::NotFound(_)));
        drop(db);
        let _ = std::fs::remove_file(&path);
        let _ = std::fs::remove_file(path.with_extension("sqlite3-wal"));
        let _ = std::fs::remove_file(path.with_extension("sqlite3-shm"));
    }

    #[tokio::test]
    async fn finishing_a_flow_marks_it_failed_and_stops_polling() {
        let path =
            std::env::temp_dir().join(format!("token-toxication-{}.sqlite3", Uuid::new_v4()));
        let db = Db::open(&path).await.expect("open test database");
        let store = CodexDeviceOAuthStore::default();
        store.flows.lock().await.insert(
            "flow-1".into(),
            DeviceFlow {
                owner_hash: "owner".into(),
                name: "Codex".into(),
                device_auth_id: "device".into(),
                user_code: "code".into(),
                poll_interval_seconds: 5,
                expires_at: Utc::now() + Duration::minutes(15),
                next_poll_at: Utc::now(),
                polling: true,
                status: FlowStatus::Pending,
                account_id: None,
                error: None,
            },
        );

        let response = finish_failed(
            &store,
            &db,
            "flow-1",
            AppError::Internal("synthetic failure".into()),
        )
        .await
        .expect("failed flow response");
        assert_eq!(response.status, "failed");
        assert_eq!(response.error.as_deref(), Some("synthetic failure"));
        assert!(!store.flows.lock().await["flow-1"].polling);

        drop(db);
        let _ = std::fs::remove_file(&path);
        let _ = std::fs::remove_file(path.with_extension("sqlite3-wal"));
        let _ = std::fs::remove_file(path.with_extension("sqlite3-shm"));
    }
}
