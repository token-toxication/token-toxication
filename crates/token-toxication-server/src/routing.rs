use axum::http::{HeaderMap, StatusCode};
use chrono::{DateTime, Duration, Utc};
use rand::Rng;
use sha2::{Digest, Sha256};

use crate::{db::ProviderRouteSelection, session_affinity::SessionAffinity};

#[derive(Debug, Clone)]
pub struct RouteFailure {
    pub provider_status: Option<&'static str>,
    pub route_status: &'static str,
    pub cooldown_until: Option<DateTime<Utc>>,
    pub error: String,
    pub status_code: Option<u16>,
}

pub struct RoutingScope<'a> {
    pub api_key_id: &'a str,
    pub public_model_id: &'a str,
    pub wire_api: &'a str,
}

pub fn select_route<'a>(
    candidates: &'a [ProviderRouteSelection],
    affinity: Option<&SessionAffinity>,
    scope: &RoutingScope<'_>,
    rng: &mut impl Rng,
) -> Option<&'a ProviderRouteSelection> {
    let role = if candidates
        .iter()
        .any(|candidate| candidate.role == "primary")
    {
        "primary"
    } else if candidates
        .iter()
        .any(|candidate| candidate.role == "backup")
    {
        "backup"
    } else {
        return None;
    };
    let eligible: Vec<_> = candidates
        .iter()
        .filter(|candidate| candidate.role == role && candidate.weight > 0)
        .collect();
    if eligible.is_empty() {
        return None;
    }
    match affinity {
        Some(affinity) => eligible.into_iter().min_by(|left, right| {
            rendezvous_score(left, affinity, scope)
                .total_cmp(&rendezvous_score(right, affinity, scope))
        }),
        None => {
            let total: u64 = eligible
                .iter()
                .map(|candidate| u64::from(candidate.weight))
                .sum();
            let mut selected = rng.random_range(0..total);
            eligible.into_iter().find(|candidate| {
                let weight = u64::from(candidate.weight);
                if selected < weight {
                    true
                } else {
                    selected -= weight;
                    false
                }
            })
        }
    }
}

fn rendezvous_score(
    candidate: &ProviderRouteSelection,
    affinity: &SessionAffinity,
    scope: &RoutingScope<'_>,
) -> f64 {
    let mut hash = Sha256::new();
    for component in [
        scope.api_key_id.as_bytes(),
        scope.public_model_id.as_bytes(),
        scope.wire_api.as_bytes(),
        affinity.key_hash.as_slice(),
        candidate.route_id.as_bytes(),
    ] {
        hash.update((component.len() as u64).to_be_bytes());
        hash.update(component);
    }
    let digest: [u8; 32] = hash.finalize().into();
    let value = u64::from_be_bytes(digest[..8].try_into().expect("SHA-256 prefix"));
    let unit = (value as f64 + 1.0) / (u64::MAX as f64 + 2.0);
    -unit.ln() / f64::from(candidate.weight)
}

pub fn classify_response_failure(
    provider: &str,
    status: StatusCode,
    headers: &HeaderMap,
    _body: &[u8],
    now: DateTime<Utc>,
) -> RouteFailure {
    let error = response_error(status);
    if matches!(status, StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN) {
        return RouteFailure {
            provider_status: Some("blocked"),
            route_status: "degraded",
            cooldown_until: None,
            error,
            status_code: Some(status.as_u16()),
        };
    }

    if status == StatusCode::TOO_MANY_REQUESTS {
        let cooldown = if minimax_model_quota_exhausted(provider, headers) {
            Duration::hours(1)
        } else {
            Duration::seconds(60)
        };
        return RouteFailure {
            provider_status: None,
            route_status: "cooling_down",
            cooldown_until: Some(now + cooldown),
            error,
            status_code: Some(status.as_u16()),
        };
    }

    if status.is_server_error() {
        return RouteFailure {
            provider_status: None,
            route_status: "cooling_down",
            cooldown_until: Some(now + Duration::seconds(30)),
            error,
            status_code: Some(status.as_u16()),
        };
    }

    RouteFailure {
        provider_status: None,
        route_status: "degraded",
        cooldown_until: None,
        error,
        status_code: Some(status.as_u16()),
    }
}

pub fn classify_transport_failure(error: String, now: DateTime<Utc>) -> RouteFailure {
    RouteFailure {
        provider_status: None,
        route_status: "cooling_down",
        cooldown_until: Some(now + Duration::seconds(30)),
        error,
        status_code: None,
    }
}

pub fn classify_upstream_application_failure(
    code: Option<&str>,
    error: String,
    now: DateTime<Utc>,
) -> Option<RouteFailure> {
    let (status, cooldown) = match code {
        Some("server_error") => (StatusCode::BAD_GATEWAY, Duration::seconds(30)),
        Some("rate_limit_exceeded") => (StatusCode::TOO_MANY_REQUESTS, Duration::seconds(60)),
        _ => return None,
    };

    Some(RouteFailure {
        provider_status: None,
        route_status: "cooling_down",
        cooldown_until: Some(now + cooldown),
        error,
        status_code: Some(status.as_u16()),
    })
}

fn response_error(status: StatusCode) -> String {
    format!("upstream returned {}", status.as_u16())
}

fn minimax_model_quota_exhausted(provider: &str, headers: &HeaderMap) -> bool {
    if !provider.starts_with("minimax") {
        return false;
    }

    headers
        .get("x-model-quota-remaining")
        .and_then(|value| value.to_str().ok())
        .is_some_and(|value| {
            value.split([',', ';']).any(|part| {
                let part = part.trim();
                part == "0"
                    || part
                        .rsplit_once('=')
                        .is_some_and(|(_, remaining)| remaining.trim() == "0")
            })
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        db::ProviderRouteSelection,
        models::{ProviderAccount, ProviderAccountRecord},
        session_affinity::{SessionAffinity, SessionSource},
    };
    use rand::{SeedableRng, rngs::StdRng};

    fn candidate(id: &str, role: &str, weight: u32) -> ProviderRouteSelection {
        ProviderRouteSelection {
            account: ProviderAccountRecord {
                account: ProviderAccount {
                    id: format!("account-{id}"),
                    name: id.to_string(),
                    provider: "openai".to_string(),
                    base_url: "https://example.com".to_string(),
                    auth_mode: "bearer".to_string(),
                    wire_api: "openai-responses".to_string(),
                    is_active: true,
                    status: "healthy".to_string(),
                    last_error: None,
                    created_at: Utc::now(),
                    last_used_at: None,
                },
                api_key: "secret".to_string(),
            },
            route_id: id.to_string(),
            public_model_id: "model".to_string(),
            upstream_model_id: id.to_string(),
            role: role.to_string(),
            weight,
            strip_params: Vec::new(),
        }
    }

    fn affinity(seed: u64) -> SessionAffinity {
        let mut key_hash = [0; 32];
        key_hash[..8].copy_from_slice(&seed.to_be_bytes());
        SessionAffinity {
            key_hash,
            source: SessionSource::NativeHeader,
            client_kind: "codex",
            conflict: false,
        }
    }

    fn scope() -> RoutingScope<'static> {
        RoutingScope {
            api_key_id: "api-key",
            public_model_id: "model",
            wire_api: "openai-responses",
        }
    }

    #[test]
    fn weighted_rendezvous_is_stable_and_respects_primary_tier() {
        let candidates = vec![
            candidate("primary-a", "primary", 100),
            candidate("primary-b", "primary", 300),
            candidate("backup", "backup", 10_000),
        ];
        let affinity = affinity(42);
        let mut rng = StdRng::seed_from_u64(1);
        let first = select_route(&candidates, Some(&affinity), &scope(), &mut rng).unwrap();
        for _ in 0..10 {
            let selected = select_route(&candidates, Some(&affinity), &scope(), &mut rng).unwrap();
            assert_eq!(selected.route_id, first.route_id);
            assert_eq!(selected.role, "primary");
        }
    }

    #[test]
    fn weighted_rendezvous_distribution_tracks_weights() {
        let candidates = vec![
            candidate("one", "primary", 100),
            candidate("three", "primary", 300),
        ];
        let mut rng = StdRng::seed_from_u64(1);
        let selected_three = (0..10_000)
            .filter(|seed| {
                select_route(&candidates, Some(&affinity(*seed)), &scope(), &mut rng)
                    .is_some_and(|candidate| candidate.route_id == "three")
            })
            .count();
        assert!((7_250..7_750).contains(&selected_three));
    }

    #[test]
    fn rendezvous_route_addition_only_remaps_sessions_to_the_new_route() {
        let original = vec![
            candidate("one", "primary", 100),
            candidate("two", "primary", 100),
        ];
        let expanded = vec![
            candidate("one", "primary", 100),
            candidate("two", "primary", 100),
            candidate("three", "primary", 100),
        ];
        let mut rng = StdRng::seed_from_u64(1);
        for seed in 0..5_000 {
            let before =
                select_route(&original, Some(&affinity(seed)), &scope(), &mut rng).unwrap();
            let after = select_route(&expanded, Some(&affinity(seed)), &scope(), &mut rng).unwrap();
            if after.route_id != "three" {
                assert_eq!(after.route_id, before.route_id);
            }
        }
    }

    #[test]
    fn rendezvous_weight_change_remaps_toward_the_heavier_route() {
        let balanced = vec![
            candidate("one", "primary", 100),
            candidate("two", "primary", 100),
        ];
        let weighted = vec![
            candidate("one", "primary", 100),
            candidate("two", "primary", 400),
        ];
        let mut rng = StdRng::seed_from_u64(1);
        let mut balanced_two = 0;
        let mut weighted_two = 0;
        let mut changed = 0;
        for seed in 0..5_000 {
            let before =
                select_route(&balanced, Some(&affinity(seed)), &scope(), &mut rng).unwrap();
            let after = select_route(&weighted, Some(&affinity(seed)), &scope(), &mut rng).unwrap();
            balanced_two += usize::from(before.route_id == "two");
            weighted_two += usize::from(after.route_id == "two");
            changed += usize::from(before.route_id != after.route_id);
        }
        assert!(changed > 0);
        assert!(weighted_two > balanced_two);
    }

    #[test]
    fn weighted_random_tracks_weights_and_uses_backup_only_without_primary() {
        let candidates = vec![
            candidate("one", "primary", 100),
            candidate("three", "primary", 300),
            candidate("backup", "backup", 10_000),
        ];
        let mut rng = StdRng::seed_from_u64(7);
        let mut selected_three = 0;
        for _ in 0..10_000 {
            let selected = select_route(&candidates, None, &scope(), &mut rng).unwrap();
            assert_eq!(selected.role, "primary");
            selected_three += usize::from(selected.route_id == "three");
        }
        assert!((7_250..7_750).contains(&selected_three));

        let backup_only = [candidate("backup", "backup", 100)];
        assert_eq!(
            select_route(&backup_only, None, &scope(), &mut rng)
                .unwrap()
                .role,
            "backup"
        );
    }

    #[test]
    fn generic_rate_limit_cools_route_for_one_minute() {
        let now = Utc::now();
        let failure = classify_response_failure(
            "deepseek",
            StatusCode::TOO_MANY_REQUESTS,
            &HeaderMap::new(),
            b"{}",
            now,
        );

        assert_eq!(failure.provider_status, None);
        assert_eq!(failure.route_status, "cooling_down");
        assert_eq!(failure.cooldown_until, Some(now + Duration::seconds(60)));
    }

    #[test]
    fn minimax_model_quota_cools_only_route_for_one_hour() {
        let now = Utc::now();
        let mut headers = HeaderMap::new();
        headers.insert("x-model-quota-remaining", "MiniMax-M3=0".parse().unwrap());

        let failure = classify_response_failure(
            "minimax-coding-plan",
            StatusCode::TOO_MANY_REQUESTS,
            &headers,
            b"{}",
            now,
        );

        assert_eq!(failure.provider_status, None);
        assert_eq!(failure.route_status, "cooling_down");
        assert_eq!(failure.cooldown_until, Some(now + Duration::hours(1)));
    }

    #[test]
    fn authorization_failure_blocks_account() {
        let failure = classify_response_failure(
            "openai",
            StatusCode::UNAUTHORIZED,
            &HeaderMap::new(),
            b"{}",
            Utc::now(),
        );

        assert_eq!(failure.provider_status, Some("blocked"));
        assert_eq!(failure.route_status, "degraded");
        assert!(failure.cooldown_until.is_none());
    }

    #[test]
    fn application_server_errors_cool_the_route_for_thirty_seconds() {
        let now = Utc::now();
        let failure = classify_upstream_application_failure(
            Some("server_error"),
            "upstream stream failed (server_error)".to_string(),
            now,
        )
        .expect("classify server error");

        assert_eq!(failure.status_code, Some(502));
        assert_eq!(failure.route_status, "cooling_down");
        assert_eq!(failure.cooldown_until, Some(now + Duration::seconds(30)));
    }

    #[test]
    fn application_rate_limits_cool_the_route_for_sixty_seconds() {
        let now = Utc::now();
        let failure = classify_upstream_application_failure(
            Some("rate_limit_exceeded"),
            "upstream stream failed (rate_limit_exceeded)".to_string(),
            now,
        )
        .expect("classify rate limit");

        assert_eq!(failure.status_code, Some(429));
        assert_eq!(failure.route_status, "cooling_down");
        assert_eq!(failure.cooldown_until, Some(now + Duration::seconds(60)));
    }

    #[test]
    fn unknown_application_errors_do_not_change_route_health() {
        assert!(
            classify_upstream_application_failure(
                Some("provider_specific_error"),
                "upstream stream failed (provider_specific_error)".to_string(),
                Utc::now(),
            )
            .is_none()
        );
    }
}
