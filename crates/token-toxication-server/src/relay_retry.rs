use std::time::Duration;

use axum::http::HeaderMap;
use chrono::{DateTime, Utc};
use rand::Rng;

pub(crate) const MAX_RETRIES: u8 = 10;
pub(crate) const RETRY_AFTER_MAX: Duration = Duration::from_secs(15 * 60);

/// Parses the two common Retry-After forms without trusting an unbounded delay.
pub(crate) fn retry_after(headers: &HeaderMap, now: DateTime<Utc>) -> Option<Duration> {
    if let Some(value) = headers
        .get("retry-after-ms")
        .and_then(|value| value.to_str().ok())
        && let Ok(milliseconds) = value.trim().parse::<u64>()
    {
        return Some(Duration::from_millis(milliseconds).min(RETRY_AFTER_MAX));
    }

    let value = headers.get("retry-after")?.to_str().ok()?.trim();
    if let Ok(seconds) = value.parse::<u64>() {
        return Some(Duration::from_secs(seconds).min(RETRY_AFTER_MAX));
    }

    let date = DateTime::parse_from_rfc2822(value)
        .or_else(|_| DateTime::parse_from_str(value, "%a, %d %b %Y %H:%M:%S GMT"))
        .ok()?
        .with_timezone(&Utc);
    let milliseconds = (date - now).num_milliseconds().max(0) as u64;
    Some(Duration::from_millis(milliseconds).min(RETRY_AFTER_MAX))
}

/// OpenCode's retry schedule: 2, 4, 8, then a ten-second cap, with jitter.
pub(crate) fn delay(retry: u8, retry_after: Option<Duration>, rng: &mut impl Rng) -> Duration {
    let base_seconds = match retry {
        0 | 1 => 2,
        2 => 4,
        3 => 8,
        _ => 10,
    };
    let jitter_seconds = rng.random_range(0..=base_seconds);
    let scheduled = Duration::from_secs(base_seconds + jitter_seconds)
        .min(RETRY_AFTER_MAX.min(Duration::from_secs(10)));
    retry_after
        .unwrap_or(Duration::ZERO)
        .max(scheduled)
        .min(RETRY_AFTER_MAX)
}

pub(crate) fn maximum_delay(retry: u8, retry_after: Option<Duration>) -> Duration {
    let base_seconds = match retry {
        0 | 1 => 2,
        2 => 4,
        3 => 8,
        _ => 10,
    };
    Duration::from_secs(base_seconds)
        .max(retry_after.unwrap_or(Duration::ZERO))
        .min(RETRY_AFTER_MAX)
}

pub(crate) fn normalize_provider_code(code: &str) -> Option<String> {
    let code = code.trim();
    (!code.is_empty()
        && code.len() <= 64
        && code
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-')))
    .then(|| code.to_owned())
}

pub(crate) fn retryable_provider_code(code: Option<&str>) -> bool {
    match code {
        None => true,
        Some(
            "api_error"
            | "internal_error"
            | "modelstreamerrorexception"
            | "overloaded_error"
            | "server_error"
            | "server_is_overloaded"
            | "service_unavailable"
            | "serviceunavailableexception"
            | "slow_down",
        ) => true,
        Some("rate_limit_exceeded" | "too_many_requests") => true,
        Some(_) => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::HeaderValue;
    use chrono::TimeZone;
    use rand::{SeedableRng, rngs::StdRng};

    #[test]
    fn parses_bounded_retry_after_seconds_and_dates() {
        let now = Utc.with_ymd_and_hms(2026, 10, 15, 0, 0, 0).unwrap();
        let mut headers = HeaderMap::new();
        headers.insert("retry-after", HeaderValue::from_static("2"));
        assert_eq!(retry_after(&headers, now), Some(Duration::from_secs(2)));

        headers.insert(
            "retry-after",
            HeaderValue::from_static("Thu, 15 Oct 2026 00:01:00 GMT"),
        );
        assert_eq!(retry_after(&headers, now), Some(Duration::from_secs(60)));

        headers.insert("retry-after", HeaderValue::from_static("999999"));
        assert_eq!(retry_after(&headers, now), Some(RETRY_AFTER_MAX));
    }

    #[test]
    fn delay_respects_backoff_and_provider_minimum() {
        let mut rng = StdRng::seed_from_u64(7);
        assert!(
            matches!(delay(1, None, &mut rng), value if (Duration::from_secs(2)..=Duration::from_secs(4)).contains(&value))
        );
        assert!(
            matches!(delay(3, None, &mut rng), value if (Duration::from_secs(8)..=Duration::from_secs(10)).contains(&value))
        );
        assert_eq!(
            delay(1, Some(Duration::from_secs(30)), &mut rng),
            Duration::from_secs(30)
        );
    }

    #[test]
    fn normalizes_bounded_provider_codes() {
        assert_eq!(
            normalize_provider_code(" server_error "),
            Some("server_error".into())
        );
        assert_eq!(normalize_provider_code("bad code"), None);
        assert_eq!(normalize_provider_code(&"x".repeat(65)), None);
    }

    #[test]
    fn recognizes_provider_internal_codes_but_not_deterministic_failures() {
        assert!(retryable_provider_code(Some("server_error")));
        assert!(retryable_provider_code(Some("server_is_overloaded")));
        assert!(retryable_provider_code(None));
        assert!(!retryable_provider_code(Some("invalid_request")));
    }
}
