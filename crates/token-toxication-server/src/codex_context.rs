use axum::http::{HeaderMap, HeaderName, HeaderValue};
use serde_json::Value;

const MAX_CONTEXT_VALUE_BYTES: usize = 8 * 1024;
const MAX_IDENTIFIER_VALUE_BYTES: usize = 1024;

pub const RESPONSES_LITE_HEADER: &str = "x-openai-internal-codex-responses-lite";
const CONTEXT_HEADERS: [&str; 6] = [
    "session-id",
    "thread-id",
    "x-client-request-id",
    "x-codex-beta-features",
    "x-codex-turn-state",
    "x-openai-subagent",
];

const RESPONSE_HEADERS: [&str; 12] = [
    "x-codex-turn-state",
    "retry-after",
    "x-request-id",
    "openai-model",
    "x-reasoning-included",
    "x-models-etag",
    "x-ratelimit-limit-requests",
    "x-ratelimit-remaining-requests",
    "x-ratelimit-reset-requests",
    "x-ratelimit-limit-tokens",
    "x-ratelimit-remaining-tokens",
    "x-ratelimit-reset-tokens",
];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HeaderError {
    InvalidValue(&'static str),
}

impl std::fmt::Display for HeaderError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidValue(name) => write!(f, "invalid Codex {name} header"),
        }
    }
}

impl std::error::Error for HeaderError {}

/// Return the small, reviewed set of request context headers that may cross the relay.
/// Authentication, account selection, originator, model, and all unknown internal headers are
/// intentionally excluded and are added by the relay itself where appropriate.
pub fn request_headers(headers: &HeaderMap) -> Result<HeaderMap, HeaderError> {
    let mut forwarded = HeaderMap::new();
    for name in CONTEXT_HEADERS {
        if let Some(value) = headers.get(name) {
            validate_value(name, value)?;
            forwarded.insert(HeaderName::from_static(name), value.clone());
        }
    }
    Ok(forwarded)
}

/// WebSocket clients carry the first request's session metadata in the event body while Codex
/// uses the same values as HTTP handshake headers. Fill missing handshake context from that
/// metadata without allowing it to override an explicit, already validated header.
pub fn request_headers_from_event(
    headers: &HeaderMap,
    event: &Value,
) -> Result<HeaderMap, HeaderError> {
    let mut forwarded = request_headers(headers)?;
    let Some(metadata) = event.get("client_metadata").and_then(Value::as_object) else {
        return Ok(forwarded);
    };
    for (metadata_key, header_name) in [
        ("session_id", "session-id"),
        ("thread_id", "thread-id"),
        ("x-codex-turn-state", "x-codex-turn-state"),
        ("x-openai-subagent", "x-openai-subagent"),
    ] {
        if forwarded.contains_key(header_name) {
            continue;
        }
        let Some(value) = metadata.get(metadata_key).and_then(Value::as_str) else {
            continue;
        };
        let value =
            HeaderValue::from_str(value).map_err(|_| HeaderError::InvalidValue(header_name))?;
        validate_value(header_name, &value)?;
        forwarded.insert(HeaderName::from_static(header_name), value);
    }
    if !forwarded.contains_key("x-client-request-id")
        && let Some(value) = forwarded.get("thread-id").cloned()
    {
        forwarded.insert(HeaderName::from_static("x-client-request-id"), value);
    }
    Ok(forwarded)
}

pub fn copy_response_headers(source: &HeaderMap, destination: &mut HeaderMap) {
    for name in RESPONSE_HEADERS {
        if let Some(value) = source.get(name) {
            destination.insert(HeaderName::from_static(name), value.clone());
        }
    }
}

fn validate_value(name: &'static str, value: &HeaderValue) -> Result<(), HeaderError> {
    let bytes = value.as_bytes();
    let limit = match name {
        "session-id" | "thread-id" | "x-client-request-id" | "x-openai-subagent" => {
            MAX_IDENTIFIER_VALUE_BYTES
        }
        _ => MAX_CONTEXT_VALUE_BYTES,
    };
    if bytes.is_empty()
        || bytes.len() > limit
        || !bytes.iter().all(|byte| (0x20..=0x7e).contains(byte))
    {
        return Err(HeaderError::InvalidValue(name));
    }
    if name == "x-codex-beta-features" {
        // Codex builds this list from its enabled feature catalog. Keep the
        // value bounded and tokenized so newer catalog entries remain compatible
        // without turning this into an arbitrary internal-header tunnel.
        let valid = bytes.split(|byte| *byte == b',').all(|feature| {
            let feature = feature.strip_prefix(b" ").unwrap_or(feature);
            !feature.is_empty()
                && feature.len() <= 128
                && feature
                    .iter()
                    .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || *byte == b'_')
        });
        if !valid {
            return Err(HeaderError::InvalidValue(name));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn forwards_context_and_drops_auth_and_internal_headers() {
        let mut input = HeaderMap::new();
        input.insert("session-id", HeaderValue::from_static("session"));
        input.insert("thread-id", HeaderValue::from_static("thread"));
        input.insert(
            "x-codex-beta-features",
            HeaderValue::from_static("remote_compaction_v2"),
        );
        input.insert("authorization", HeaderValue::from_static("Bearer secret"));
        input.insert(
            "x-openai-internal-secret",
            HeaderValue::from_static("secret"),
        );
        let output = request_headers(&input).expect("valid context");
        assert_eq!(output.len(), 3);
        assert!(!output.contains_key("authorization"));
        assert!(!output.contains_key("x-openai-internal-secret"));
    }

    #[test]
    fn rejects_unknown_beta_feature_and_oversized_context() {
        let mut headers = HeaderMap::new();
        headers.insert(
            "x-codex-beta-features",
            HeaderValue::from_static("remote_compaction_v2,multi_agent_v2"),
        );
        assert!(request_headers(&headers).is_ok());
        headers.insert(
            "x-codex-beta-features",
            HeaderValue::from_static("unknown feature"),
        );
        assert!(request_headers(&headers).is_err());
        let mut headers = HeaderMap::new();
        headers.insert(
            "session-id",
            HeaderValue::from_str(&"x".repeat(1025)).unwrap(),
        );
        assert!(request_headers(&headers).is_err());
    }

    #[test]
    fn derives_websocket_context_from_first_event() {
        let event = serde_json::json!({
            "client_metadata": {
                "session_id": "session",
                "thread_id": "thread",
                "x-codex-turn-state": "turn"
            }
        });
        let headers = request_headers_from_event(&HeaderMap::new(), &event).expect("context");
        assert_eq!(headers["session-id"], "session");
        assert_eq!(headers["thread-id"], "thread");
        assert_eq!(headers["x-client-request-id"], "thread");
        assert_eq!(headers["x-codex-turn-state"], "turn");
    }

    #[test]
    fn copies_only_codex_response_metadata() {
        let mut source = HeaderMap::new();
        source.insert("x-codex-turn-state", HeaderValue::from_static("turn"));
        source.insert("x-request-id", HeaderValue::from_static("request"));
        source.insert("set-cookie", HeaderValue::from_static("secret"));
        let mut destination = HeaderMap::new();
        copy_response_headers(&source, &mut destination);
        assert_eq!(destination.len(), 2);
        assert!(!destination.contains_key("set-cookie"));
    }
}
