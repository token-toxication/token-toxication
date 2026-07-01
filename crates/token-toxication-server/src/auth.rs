use std::time::Duration;

use axum::{
    Json,
    extract::{Request, State},
    http::{HeaderMap, StatusCode},
    middleware::Next,
    response::Response,
};
use chrono::Utc;
use rand::RngCore;
use sha2::{Digest, Sha256};

use crate::{
    AppState,
    error::AppError,
    models::{AdminUser, LoginRequest, LoginResponse},
};

const ADMIN_SESSION_HOURS: i64 = 24;

pub fn hash_secret(secret: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(secret.as_bytes());
    hex::encode(hasher.finalize())
}

pub fn generate_secret(prefix: &str) -> String {
    let mut bytes = [0_u8; 32];
    rand::rng().fill_bytes(&mut bytes);
    format!("{prefix}{}", hex::encode(bytes))
}

pub fn key_preview(secret: &str) -> String {
    let visible = secret
        .char_indices()
        .nth(24)
        .map_or(secret.len(), |(idx, _)| idx);
    format!("{}...", &secret[..visible])
}

pub fn extract_bearer(headers: &HeaderMap) -> Option<String> {
    let value = headers
        .get(axum::http::header::AUTHORIZATION)?
        .to_str()
        .ok()?;
    value
        .strip_prefix("Bearer ")
        .or_else(|| value.strip_prefix("bearer "))
        .map(str::trim)
        .filter(|token| !token.is_empty())
        .map(ToOwned::to_owned)
}

pub fn extract_api_key(headers: &HeaderMap, query: Option<&str>) -> Option<String> {
    for name in ["x-api-key", "x-goog-api-key", "api-key"] {
        if let Some(value) = headers
            .get(name)
            .and_then(|value| value.to_str().ok())
            .map(str::trim)
            .filter(|value| !value.is_empty())
        {
            return Some(value.to_string());
        }
    }

    if let Some(value) = extract_bearer(headers) {
        return Some(value);
    }

    query.and_then(|raw| {
        raw.split('&').find_map(|pair| {
            let (key, value) = pair.split_once('=')?;
            (key == "key" && !value.is_empty()).then(|| value.to_string())
        })
    })
}

pub async fn login(
    State(state): State<AppState>,
    Json(request): Json<LoginRequest>,
) -> Result<Json<LoginResponse>, AppError> {
    if request.username != state.config.admin_username
        || request.password != state.config.admin_password
    {
        return Err(AppError::Unauthorized("invalid admin credentials".into()));
    }

    let token = generate_secret("tt_admin_");
    let expires_at = Utc::now()
        + chrono::Duration::from_std(Duration::from_secs((ADMIN_SESSION_HOURS * 60 * 60) as u64))
            .expect("valid duration");
    state
        .db
        .create_admin_session(&token, &request.username, expires_at)
        .await?;

    Ok(Json(LoginResponse {
        token,
        username: request.username,
        expires_at,
    }))
}

pub async fn me(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Json<AdminUser>, AppError> {
    let token =
        extract_bearer(&headers).ok_or_else(|| AppError::Unauthorized("missing token".into()))?;
    let user = state
        .db
        .validate_admin_session(&token, Utc::now())
        .await?
        .ok_or_else(|| AppError::Unauthorized("invalid or expired token".into()))?;
    Ok(Json(AdminUser { username: user }))
}

pub async fn require_admin(
    State(state): State<AppState>,
    mut request: Request,
    next: Next,
) -> Result<Response, AppError> {
    let token = extract_bearer(request.headers())
        .ok_or_else(|| AppError::Unauthorized("missing admin bearer token".into()))?;
    let user = state
        .db
        .validate_admin_session(&token, Utc::now())
        .await?
        .ok_or_else(|| AppError::Unauthorized("invalid or expired admin session".into()))?;
    request
        .extensions_mut()
        .insert(AdminUser { username: user });
    Ok(next.run(request).await)
}

pub async fn logout(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<StatusCode, AppError> {
    if let Some(token) = extract_bearer(&headers) {
        state.db.delete_admin_session(&token).await?;
    }
    Ok(StatusCode::NO_CONTENT)
}
