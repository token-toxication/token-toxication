use axum::http::HeaderMap;
use serde_json::Value;
use sha2::{Digest, Sha256};

pub const SESSION_ID_HEADER: &str = "token-toxication-session-id";
const MAX_SESSION_ID_BYTES: usize = 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionSource {
    ExplicitHeader,
    NativeHeader,
    NativeBody,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionAffinity {
    pub key_hash: [u8; 32],
    pub source: SessionSource,
    pub client_kind: &'static str,
    pub conflict: bool,
}

pub fn extract(wire_api: &str, headers: &HeaderMap, body: &Value) -> Option<SessionAffinity> {
    let client_kind = client_kind(headers, wire_api);
    let candidates = match wire_api {
        "anthropic-messages" => anthropic_candidates(headers, body),
        "gemini-generate-content" => gemini_candidates(headers, body),
        _ => openai_candidates(headers, body),
    };
    let mut present = candidates
        .into_iter()
        .filter_map(|(value, source)| value.map(|value| (value, source)));
    let (selected, source) = present.next()?;
    let selected = normalize(Some(&selected))?;
    let conflict = present
        .filter_map(|(value, _)| normalize(Some(&value)))
        .any(|value| value != selected);
    Some(SessionAffinity {
        key_hash: hash_session(&selected),
        source,
        client_kind,
        conflict,
    })
}

pub fn client_kind(headers: &HeaderMap, _wire_api: &str) -> &'static str {
    let user_agent = headers
        .get("user-agent")
        .and_then(|value| value.to_str().ok())
        .unwrap_or("")
        .to_ascii_lowercase();
    if user_agent.contains("opencode") {
        "opencode"
    } else if user_agent.contains("codex") {
        "codex"
    } else if user_agent.contains("deepseek") || user_agent.contains("dsh") {
        "dsh"
    } else if user_agent.contains("claude") || headers.contains_key("x-claude-code-session-id") {
        "claude_code"
    } else if user_agent.contains("oh-my-pi")
        || user_agent.starts_with("pi/")
        || user_agent.starts_with("omp/")
    {
        "pi"
    } else {
        "unknown"
    }
}

fn openai_candidates(headers: &HeaderMap, body: &Value) -> Vec<(Option<String>, SessionSource)> {
    vec![
        (
            header(headers, SESSION_ID_HEADER),
            SessionSource::ExplicitHeader,
        ),
        (header(headers, "session-id"), SessionSource::NativeHeader),
        (header(headers, "session_id"), SessionSource::NativeHeader),
        (
            owned(body.get("session_id").and_then(Value::as_str)),
            SessionSource::NativeBody,
        ),
        (
            owned(
                body.pointer("/client_metadata/session_id")
                    .and_then(Value::as_str),
            ),
            SessionSource::NativeBody,
        ),
        (
            owned(
                body.pointer("/client_metadata/thread_id")
                    .and_then(Value::as_str),
            ),
            SessionSource::NativeBody,
        ),
        (
            owned(body.get("prompt_cache_key").and_then(Value::as_str)),
            SessionSource::NativeBody,
        ),
        (
            header(headers, "x-client-request-id"),
            SessionSource::NativeHeader,
        ),
    ]
}

fn anthropic_candidates(headers: &HeaderMap, body: &Value) -> Vec<(Option<String>, SessionSource)> {
    vec![
        (
            header(headers, SESSION_ID_HEADER),
            SessionSource::ExplicitHeader,
        ),
        (
            header(headers, "x-claude-code-session-id"),
            SessionSource::NativeHeader,
        ),
        (
            body.pointer("/metadata/user_id")
                .and_then(Value::as_str)
                .and_then(parse_claude_user_id),
            SessionSource::NativeBody,
        ),
    ]
}

fn gemini_candidates(headers: &HeaderMap, body: &Value) -> Vec<(Option<String>, SessionSource)> {
    vec![
        (
            header(headers, SESSION_ID_HEADER),
            SessionSource::ExplicitHeader,
        ),
        (
            owned(body.get("sessionId").and_then(Value::as_str)),
            SessionSource::NativeBody,
        ),
        (
            owned(body.get("session_id").and_then(Value::as_str)),
            SessionSource::NativeBody,
        ),
    ]
}

fn owned(value: Option<&str>) -> Option<String> {
    value.map(str::to_owned)
}

fn header(headers: &HeaderMap, name: &str) -> Option<String> {
    headers.get(name).map(|value| {
        value
            .to_str()
            .map(str::to_owned)
            .unwrap_or_else(|_| String::new())
    })
}

fn normalize(value: Option<&str>) -> Option<String> {
    let value = value?.trim();
    (!value.is_empty() && value.len() <= MAX_SESSION_ID_BYTES).then(|| value.to_string())
}

fn parse_claude_user_id(value: &str) -> Option<String> {
    if value.trim_start().starts_with('{') {
        return serde_json::from_str::<Value>(value)
            .ok()
            .and_then(|parsed| parsed.get("session_id")?.as_str().map(str::to_owned));
    }
    value
        .rsplit_once("_session_")
        .map(|(_, session)| session)
        .filter(|session| !session.is_empty())
        .map(str::to_owned)
}

fn hash_session(value: &str) -> [u8; 32] {
    let mut hash = Sha256::new();
    hash.update(b"token-toxication/session-affinity/v1\0");
    hash.update(value.as_bytes());
    hash.finalize().into()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn explicit_header_wins_and_reports_conflict() {
        let mut headers = HeaderMap::new();
        headers.insert(SESSION_ID_HEADER, "explicit".parse().unwrap());
        headers.insert("session-id", "native".parse().unwrap());
        let affinity = extract(
            "openai-responses",
            &headers,
            &json!({"client_metadata":{"session_id":"body"}}),
        )
        .unwrap();
        assert_eq!(affinity.source, SessionSource::ExplicitHeader);
        assert!(affinity.conflict);
    }

    #[test]
    fn supports_openai_body_fallbacks_in_order() {
        let affinity = extract(
            "openai-responses",
            &HeaderMap::new(),
            &json!({"client_metadata":{"thread_id":"thread"},"prompt_cache_key":"cache"}),
        )
        .unwrap();
        assert_eq!(affinity.source, SessionSource::NativeBody);
        assert!(affinity.conflict);
    }

    #[test]
    fn parses_claude_json_and_legacy_metadata() {
        let json_affinity = extract(
            "anthropic-messages",
            &HeaderMap::new(),
            &json!({"metadata":{"user_id":"{\"device_id\":\"d\",\"session_id\":\"s-json\"}"}}),
        )
        .unwrap();
        let legacy_affinity = extract(
            "anthropic-messages",
            &HeaderMap::new(),
            &json!({"metadata":{"user_id":"user_a_account_b_session_s-legacy"}}),
        )
        .unwrap();
        assert_ne!(json_affinity.key_hash, legacy_affinity.key_hash);
    }

    #[test]
    fn oversized_values_are_missing() {
        let mut headers = HeaderMap::new();
        headers.insert(
            SESSION_ID_HEADER,
            "x".repeat(MAX_SESSION_ID_BYTES + 1).parse().unwrap(),
        );
        assert!(extract("openai-chat", &headers, &json!({})).is_none());
    }

    #[test]
    fn invalid_explicit_value_does_not_fall_back_to_native_affinity() {
        let mut headers = HeaderMap::new();
        headers.insert(
            SESSION_ID_HEADER,
            "x".repeat(MAX_SESSION_ID_BYTES + 1).parse().unwrap(),
        );
        headers.insert("session-id", "native".parse().unwrap());
        assert!(extract("openai-responses", &headers, &json!({})).is_none());
    }

    #[test]
    fn supports_client_setup_native_sources() {
        let mut codex = HeaderMap::new();
        codex.insert("session-id", "codex-session".parse().unwrap());
        assert!(extract("openai-responses", &codex, &json!({})).is_some());

        let mut pi_openai = HeaderMap::new();
        pi_openai.insert("x-client-request-id", "pi-session".parse().unwrap());
        assert!(extract("openai-chat", &pi_openai, &json!({})).is_some());

        let mut pi_anthropic = HeaderMap::new();
        pi_anthropic.insert(
            "x-claude-code-session-id",
            "pi-anthropic-session".parse().unwrap(),
        );
        assert!(extract("anthropic-messages", &pi_anthropic, &json!({})).is_some());

        assert!(
            extract(
                "openai-chat",
                &HeaderMap::new(),
                &json!({"prompt_cache_key":"opencode-or-dsh-session"}),
            )
            .is_some()
        );
    }

    #[test]
    fn recognizes_client_setup_user_agents_without_session_headers() {
        for (user_agent, expected) in [
            ("codex_cli_rs/1.0", "codex"),
            ("opencode/1.0", "opencode"),
            ("omp/17.2.12", "pi"),
            (
                "deepseek-harness/1.0 (+https://github.com/deepseek-ai/deepseek-harness)",
                "dsh",
            ),
            ("claude-code/1.0", "claude_code"),
        ] {
            let mut headers = HeaderMap::new();
            headers.insert("user-agent", user_agent.parse().unwrap());
            assert_eq!(client_kind(&headers, "openai-chat"), expected);
        }
    }
}
