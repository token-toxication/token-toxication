use super::*;
use serde_json::json;

const WITH_CREDITS: &str = "codex_usage_limit_with_credits";

async fn seed(db: &Db) -> (String, String) {
    let account = db
        .create_provider_account(
            serde_json::from_value(json!({
                "name":"Synthetic Codex", "provider":"codex-subscription",
                "baseUrl":"https://example.test/backend-api", "authMode":"codex-manual-refresh",
                "wireApi":"openai-responses", "apiKey":"synthetic-key"
            }))
            .unwrap(),
        )
        .await
        .unwrap();
    assert!(!account.allow_credits);
    db.create_model_catalog_entry(serde_json::from_value(json!({"id":"model"})).unwrap())
        .await
        .unwrap();
    let route = db
        .create_provider_model_route(
            serde_json::from_value(json!({
                "publicModelId":"model", "providerAccountId":account.id,
                "upstreamModelId":"upstream", "wireApi":"openai-responses"
            }))
            .unwrap(),
        )
        .await
        .unwrap();
    (account.id, route.id)
}

async fn permit(db: &Db, id: &str, allow: bool) {
    db.update_provider_account(
        id,
        serde_json::from_value(json!({"allowCredits":allow})).unwrap(),
    )
    .await
    .unwrap();
}

async fn assert_available(db: &Db, expected: bool) {
    assert_eq!(
        !db.list_provider_route_candidates("openai-responses", "model")
            .await
            .unwrap()
            .is_empty(),
        expected
    );
    assert_eq!(
        !db.list_routable_model_catalog(&["openai-responses"])
            .await
            .unwrap()
            .is_empty(),
        expected
    );
    assert_eq!(
        db.has_limited_provider_route("openai-responses", "model")
            .await
            .unwrap(),
        !expected
    );
}

#[tokio::test]
async fn credits_permission_persists_without_erasing_included_quota() {
    let path = std::env::temp_dir().join(format!("tt-credits-{}.sqlite3", Uuid::new_v4()));
    let db = Db::open(&path).await.unwrap();
    let (id, _) = seed(&db).await;
    // Exercise migration from a pre-policy database, not just new defaults.
    db.conn
        .lock()
        .await
        .execute_batch(
            "DROP VIEW effective_provider_limits;
        ALTER TABLE provider_accounts DROP COLUMN allow_credits;
        ALTER TABLE provider_accounts DROP COLUMN codex_quota_checked_at;",
        )
        .unwrap();
    drop(db);
    let db = Db::open(&path).await.unwrap();
    assert!(
        !db.get_provider_account(&id)
            .await
            .unwrap()
            .unwrap()
            .allow_credits
    );
    db.apply_codex_quota_observation(&id, Utc::now(), Some((WITH_CREDITS, None)))
        .await
        .unwrap();
    assert_available(&db, false).await;
    permit(&db, &id, true).await;
    assert_available(&db, true).await;
    // A successful long-running response must not erase the observed quota.
    db.clear_due_provider_limits(&id, "upstream", Utc::now() + Duration::hours(3))
        .await
        .unwrap();
    drop(db);
    let db = Db::open(&path).await.unwrap();
    assert!(db.list_provider_accounts().await.unwrap()[0].allow_credits);
    assert_available(&db, true).await;
    permit(&db, &id, false).await;
    assert_available(&db, false).await;
    assert_eq!(
        db.get_provider_account(&id).await.unwrap().unwrap().status,
        "quota_exhausted"
    );
    drop(db);
    let _ = std::fs::remove_file(path);
}

#[tokio::test]
async fn stale_credit_observations_fail_closed_even_after_recheck_rescheduling() {
    let db = Db::open(Path::new(":memory:")).await.unwrap();
    let (id, _) = seed(&db).await;
    permit(&db, &id, true).await;
    db.apply_codex_quota_observation(
        &id,
        Utc::now() - Duration::minutes(3),
        Some((WITH_CREDITS, None)),
    )
    .await
    .unwrap();
    db.reschedule_provider_limits(&id, Utc::now() + Duration::minutes(15))
        .await
        .unwrap();
    assert_available(&db, false).await;
    db.apply_codex_quota_observation(&id, Utc::now(), Some((WITH_CREDITS, None)))
        .await
        .unwrap();
    assert_available(&db, true).await;
    db.apply_codex_quota_observation(&id, Utc::now() - Duration::minutes(2), None)
        .await
        .unwrap();
    permit(&db, &id, false).await;
    assert_available(&db, false).await;
    db.apply_codex_quota_observation(&id, Utc::now(), None)
        .await
        .unwrap();
    assert_available(&db, true).await;
}

#[tokio::test]
async fn credits_never_clear_actual_failures_or_authentication_blocks() {
    for scope in [Some("upstream"), None] {
        let db = Db::open(Path::new(":memory:")).await.unwrap();
        let (id, route) = seed(&db).await;
        permit(&db, &id, true).await;
        db.apply_codex_quota_observation(&id, Utc::now(), Some((WITH_CREDITS, None)))
            .await
            .unwrap();
        assert_available(&db, true).await;
        db.record_provider_limit(&id, scope, "quota_exhausted", "upstream returned 429", None)
            .await
            .unwrap();
        // Even a positive read after the failure is not an immediate recovery.
        db.apply_codex_quota_observation(&id, Utc::now(), Some((WITH_CREDITS, None)))
            .await
            .unwrap();
        assert_available(&db, false).await;
        permit(&db, &id, false).await;
        permit(&db, &id, true).await;
        assert_available(&db, false).await;
        assert_eq!(
            db.get_provider_model_route(&route)
                .await
                .unwrap()
                .unwrap()
                .status,
            "healthy"
        );
        db.mark_provider_result(&id, "blocked", Some("synthetic auth failure"))
            .await
            .unwrap();
        permit(&db, &id, false).await;
        db.apply_codex_quota_observation(&id, Utc::now(), None)
            .await
            .unwrap();
        let account = db.get_provider_account(&id).await.unwrap().unwrap();
        assert_eq!(account.status, "blocked");
        assert_eq!(
            account.last_error.as_deref(),
            Some("synthetic auth failure")
        );
    }
}

#[tokio::test]
async fn leaving_codex_clears_only_codex_quota_observations() {
    let db = Db::open(Path::new(":memory:")).await.unwrap();
    let (id, _) = seed(&db).await;
    db.apply_codex_quota_observation(&id, Utc::now(), Some((WITH_CREDITS, None)))
        .await
        .unwrap();
    permit(&db, &id, true).await;
    db.update_provider_account(
        &id,
        serde_json::from_value(json!({"authMode":"bearer"})).unwrap(),
    )
    .await
    .unwrap();
    assert!(
        !db.get_provider_account(&id)
            .await
            .unwrap()
            .unwrap()
            .allow_credits
    );
    assert_available(&db, true).await;
    db.record_provider_limit(
        &id,
        Some("upstream"),
        "rate_limited",
        "synthetic limit",
        Some(Utc::now() + Duration::minutes(1)),
    )
    .await
    .unwrap();
    db.update_provider_account(
        &id,
        serde_json::from_value(json!({"name":"renamed"})).unwrap(),
    )
    .await
    .unwrap();
    assert_available(&db, false).await;
}
