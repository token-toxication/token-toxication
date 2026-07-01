use aioduct::TokioClient;
use axum::http::StatusCode;
use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use chrono::Utc;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::{db::Db, error::AppError, models::ProviderAccountRecord};

const CODEX_CLIENT_ID: &str = "app_EMoamEEZ73f0CkXaXp7hrann";
const DEFAULT_CODEX_ISSUER: &str = "https://auth.openai.com";
const DEFAULT_CODEX_ENDPOINT: &str = "https://chatgpt.com/backend-api/codex/responses";
const REFRESH_SAFETY_MARGIN_MS: i64 = 30_000;
const STORED_CODEX_CREDENTIAL_TYPE: &str = "token-toxication-codex-oauth-v1";
const RAW_REFRESH_TOKEN_HELP: &str = "Codex subscription expects a raw refresh token. \
Use `jq -r '.tokens.refresh_token' ~/.codex/auth.json` for Codex CLI or \
`jq -r '.openai.refresh' ~/.local/share/opencode/auth.json` for opencode.";

#[derive(Debug, Clone)]
pub struct CodexSubscriptionAuthorization {
    pub access_token: String,
    pub account_id: Option<String>,
    pub endpoint: String,
}

#[derive(Debug, Clone)]
struct CodexSubscriptionCredential {
    refresh: String,
    access: Option<String>,
    expires: Option<i64>,
    account_id: Option<String>,
    issuer: String,
    endpoint: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
struct StoredCodexSubscriptionCredential {
    #[serde(default)]
    r#type: String,
    #[serde(default)]
    refresh: String,
    #[serde(default)]
    access: Option<String>,
    #[serde(default)]
    expires: Option<i64>,
    #[serde(default, rename = "accountId")]
    account_id: Option<String>,
    #[serde(default)]
    issuer: Option<String>,
    #[serde(default)]
    endpoint: Option<String>,
}

#[derive(Debug, Deserialize)]
struct TokenResponse {
    #[serde(default)]
    id_token: String,
    access_token: String,
    refresh_token: String,
    expires_in: Option<i64>,
}

pub fn is_codex_subscription_auth(auth_mode: &str) -> bool {
    auth_mode == "codex-oauth"
}

pub fn codex_subscription_endpoint(base_url: &str) -> String {
    let base_url = base_url.trim().trim_end_matches('/');
    if base_url.is_empty() {
        return DEFAULT_CODEX_ENDPOINT.to_string();
    }
    if base_url.ends_with("/backend-api/codex/responses") || base_url.ends_with("/responses") {
        return base_url.to_string();
    }
    format!("{base_url}/responses")
}

pub async fn codex_subscription_authorization(
    db: &Db,
    http: &TokioClient,
    account: &ProviderAccountRecord,
) -> Result<CodexSubscriptionAuthorization, AppError> {
    let mut credential = parse_codex_subscription_credential(&account.api_key)?;
    if credential
        .access
        .as_deref()
        .is_some_and(|access| !access.trim().is_empty())
        && !credential_is_expired(credential.expires)
    {
        return Ok(CodexSubscriptionAuthorization {
            access_token: credential.access.unwrap_or_default(),
            account_id: credential.account_id,
            endpoint: credential
                .endpoint
                .unwrap_or_else(|| codex_subscription_endpoint(&account.account.base_url)),
        });
    }

    let tokens = refresh_codex_subscription_token(http, &credential).await?;
    credential.access = Some(tokens.access_token.clone());
    credential.refresh = tokens.refresh_token.clone();
    credential.expires =
        Some(Utc::now().timestamp_millis() + tokens.expires_in.unwrap_or(3600) * 1000);
    credential.account_id = extract_account_id(&tokens).or(credential.account_id);
    db.update_provider_account_secret(&account.account.id, &serialize_credential(&credential))
        .await?;

    Ok(CodexSubscriptionAuthorization {
        access_token: tokens.access_token,
        account_id: credential.account_id,
        endpoint: credential
            .endpoint
            .unwrap_or_else(|| codex_subscription_endpoint(&account.account.base_url)),
    })
}

fn parse_codex_subscription_credential(
    value: &str,
) -> Result<CodexSubscriptionCredential, AppError> {
    let value = value.trim();
    if value.is_empty() {
        return Err(AppError::BadRequest(
            "Codex subscription refresh token is required".into(),
        ));
    }
    if !value.starts_with('{') {
        return Ok(CodexSubscriptionCredential {
            refresh: value.to_string(),
            access: None,
            expires: None,
            account_id: None,
            issuer: DEFAULT_CODEX_ISSUER.to_string(),
            endpoint: None,
        });
    }

    let stored: StoredCodexSubscriptionCredential = serde_json::from_str(value)
        .map_err(|_| AppError::BadRequest(RAW_REFRESH_TOKEN_HELP.into()))?;
    if stored.r#type != STORED_CODEX_CREDENTIAL_TYPE {
        return Err(AppError::BadRequest(RAW_REFRESH_TOKEN_HELP.into()));
    }
    if stored.refresh.trim().is_empty() {
        return Err(AppError::BadRequest(
            "stored Codex subscription credential is missing a refresh token".into(),
        ));
    }
    Ok(CodexSubscriptionCredential {
        refresh: stored.refresh,
        access: stored.access,
        expires: normalize_expires(stored.expires),
        account_id: stored.account_id,
        issuer: stored
            .issuer
            .filter(|issuer| !issuer.trim().is_empty())
            .unwrap_or_else(|| DEFAULT_CODEX_ISSUER.to_string())
            .trim_end_matches('/')
            .to_string(),
        endpoint: stored
            .endpoint
            .filter(|endpoint| !endpoint.trim().is_empty())
            .map(|endpoint| endpoint.trim_end_matches('/').to_string()),
    })
}

async fn refresh_codex_subscription_token(
    http: &TokioClient,
    credential: &CodexSubscriptionCredential,
) -> Result<TokenResponse, AppError> {
    let token_url = format!("{}/oauth/token", credential.issuer.trim_end_matches('/'));
    let response = http
        .post(&token_url)?
        .form(&[
            ("grant_type", "refresh_token"),
            ("refresh_token", credential.refresh.as_str()),
            ("client_id", CODEX_CLIENT_ID),
        ])
        .send()
        .await
        .map_err(|error| AppError::Upstream(error.into()))?;
    let status = response.status();
    let bytes = response.bytes().await?;
    if !status.is_success() {
        let body = String::from_utf8_lossy(&bytes);
        let message = format!(
            "Codex token refresh failed: {} {}",
            status.as_u16(),
            body.trim()
        );
        return if matches!(
            status,
            StatusCode::BAD_REQUEST | StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN
        ) {
            Err(AppError::Unauthorized(message))
        } else {
            Err(AppError::Internal(message))
        };
    }
    serde_json::from_slice(&bytes)
        .map_err(|error| AppError::Internal(format!("invalid Codex token response: {error}")))
}

fn serialize_credential(credential: &CodexSubscriptionCredential) -> String {
    let stored = StoredCodexSubscriptionCredential {
        r#type: STORED_CODEX_CREDENTIAL_TYPE.to_string(),
        refresh: credential.refresh.clone(),
        access: credential.access.clone(),
        expires: credential.expires,
        account_id: credential.account_id.clone(),
        issuer: (credential.issuer != DEFAULT_CODEX_ISSUER).then(|| credential.issuer.clone()),
        endpoint: credential.endpoint.clone(),
    };
    serde_json::to_string(&stored).unwrap_or_else(|_| credential.refresh.clone())
}

fn credential_is_expired(expires: Option<i64>) -> bool {
    let Some(expires) = expires else {
        return true;
    };
    Utc::now().timestamp_millis() + REFRESH_SAFETY_MARGIN_MS >= expires
}

fn normalize_expires(value: Option<i64>) -> Option<i64> {
    value.map(|expires| {
        if expires > 10_000_000_000 {
            expires
        } else {
            expires * 1000
        }
    })
}

fn extract_account_id(tokens: &TokenResponse) -> Option<String> {
    extract_account_id_from_jwt(&tokens.id_token)
        .or_else(|| extract_account_id_from_jwt(&tokens.access_token))
}

fn extract_account_id_from_jwt(token: &str) -> Option<String> {
    let payload = token.split('.').nth(1)?;
    let bytes = URL_SAFE_NO_PAD.decode(payload).ok()?;
    let claims: Value = serde_json::from_slice(&bytes).ok()?;
    claims
        .get("chatgpt_account_id")
        .and_then(Value::as_str)
        .or_else(|| {
            claims
                .get("https://api.openai.com/auth")
                .and_then(|auth| auth.get("chatgpt_account_id"))
                .and_then(Value::as_str)
        })
        .or_else(|| {
            claims
                .get("organizations")
                .and_then(Value::as_array)
                .and_then(|organizations| organizations.first())
                .and_then(|organization| organization.get("id"))
                .and_then(Value::as_str)
        })
        .map(ToOwned::to_owned)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn jwt(claims: Value) -> String {
        let header = URL_SAFE_NO_PAD.encode(r#"{"alg":"none"}"#);
        let payload = URL_SAFE_NO_PAD.encode(claims.to_string());
        format!("{header}.{payload}.sig")
    }

    #[test]
    fn raw_value_is_treated_as_refresh_token() {
        let credential = parse_codex_subscription_credential("refresh-old").expect("parse");

        assert_eq!(credential.refresh, "refresh-old");
        assert!(credential.access.is_none());
        assert_eq!(credential.issuer, DEFAULT_CODEX_ISSUER);
    }

    #[test]
    fn rejects_opencode_oauth_json() {
        let error = parse_codex_subscription_credential(
            r#"{
                "type": "oauth",
                "refresh": "refresh-old",
                "access": "access-old",
                "expires": 4102444800000,
                "accountId": "acc-123"
            }"#,
        )
        .expect_err("reject opencode credential object");

        assert!(error.to_string().contains("raw refresh token"));
    }

    #[test]
    fn parses_stored_token_cache() {
        let credential = parse_codex_subscription_credential(
            r#"{
                "type": "token-toxication-codex-oauth-v1",
                "refresh": "refresh-old",
                "access": "access-old",
                "expires": 4102444800000,
                "accountId": "acc-456"
            }"#,
        )
        .expect("parse");

        assert_eq!(credential.refresh, "refresh-old");
        assert_eq!(credential.access.as_deref(), Some("access-old"));
        assert_eq!(credential.expires, Some(4102444800000));
        assert_eq!(credential.account_id.as_deref(), Some("acc-456"));
    }

    #[test]
    fn extracts_account_id_from_jwt_claims() {
        let token = jwt(serde_json::json!({
            "https://api.openai.com/auth": {
                "chatgpt_account_id": "acc-nested"
            }
        }));

        assert_eq!(
            extract_account_id_from_jwt(&token).as_deref(),
            Some("acc-nested")
        );
    }

    #[test]
    fn codex_endpoint_accepts_exact_or_base_url() {
        assert_eq!(
            codex_subscription_endpoint("https://chatgpt.com/backend-api/codex"),
            DEFAULT_CODEX_ENDPOINT
        );
        assert_eq!(
            codex_subscription_endpoint(DEFAULT_CODEX_ENDPOINT),
            DEFAULT_CODEX_ENDPOINT
        );
    }
}
