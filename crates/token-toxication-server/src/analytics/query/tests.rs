use super::*;

fn fixture() -> Connection {
    let conn = Connection::open_in_memory().unwrap();
    conn.execute_batch(
        "CREATE TABLE request_logs (
      id TEXT, model TEXT, provider_account_id TEXT, created_at TEXT,
      outcome TEXT, transport TEXT, request_summary TEXT, status_code INTEGER DEFAULT 200,
      input_tokens INTEGER DEFAULT 100, output_tokens INTEGER DEFAULT 10,
      cached_input_tokens INTEGER DEFAULT 80, attempt_index INTEGER DEFAULT 0,
      overload_retry_count INTEGER DEFAULT 0, is_overload_retry INTEGER, ttft_ms INTEGER, latency_ms INTEGER DEFAULT 1000,
      queue_wait_ms INTEGER DEFAULT 0, otps REAL DEFAULT 20);
      CREATE INDEX idx_request_logs_created_at ON request_logs(created_at);
      CREATE TABLE provider_accounts (id TEXT, name TEXT);",
    )
    .unwrap();
    conn
}

fn now() -> DateTime<Utc> {
    "2026-10-19T12:01:30Z".parse().unwrap()
}

fn seed(conn: &Connection, id: i32, outcome: Option<&str>, summary: Option<&str>, cached: i32) {
    conn.execute("INSERT INTO request_logs (id,model,provider_account_id,created_at,outcome,transport,request_summary,cached_input_tokens,ttft_ms)
      VALUES (?1,'model-a','deleted-account',?2,?3,'http_sse',?4,?5,?6)",
      params![id, (now()-chrono::Duration::seconds(20)).to_rfc3339(), outcome, summary, cached, id*100]).unwrap();
}

#[test]
fn computes_weighted_cache_percentiles_and_explicit_unknowns() {
    let mut conn = fixture();
    seed(
        &conn,
        1,
        Some("success"),
        Some(r#"{"usageComplete":true}"#),
        80,
    );
    seed(&conn, 2, Some("success"), None, 0);
    seed(
        &conn,
        3,
        Some("success"),
        Some(r#"{"usageComplete":true}"#),
        101,
    );
    seed(&conn, 4, Some("rate_limited"), Some("invalid-json"), 0);
    seed(&conn, 5, Some("client_disconnect"), None, 0);
    seed(&conn, 6, None, None, 0);
    let h = history(&mut conn, &AnalyticsQuery::default(), now()).unwrap();
    let s = &h.summary;
    assert_eq!(
        (
            s.attempts,
            s.success,
            s.failures,
            s.client_disconnect,
            s.unknown
        ),
        (6, 3, 1, 1, 1)
    );
    assert_eq!((s.input_tokens, s.output_tokens), (600, 60));
    assert_eq!(
        (
            s.classified_input_tokens,
            s.cached_input_tokens,
            s.cache_samples
        ),
        (100, 80, 1)
    );
    assert_eq!(s.ttft.samples, 3);
    assert_eq!((s.ttft.p50, s.ttft.p95), (Some(200.0), Some(300.0)));
    assert_eq!(
        h.buckets.iter().map(|b| b.stats.attempts).sum::<u64>(),
        s.attempts
    );
    assert!(h.accounts[0].deleted);
    assert_eq!(h.accounts[0].id.as_deref(), Some("deleted-account"));
    let text = serde_json::to_string(&h).unwrap();
    assert!(!text.contains("usageComplete"));
    assert!(!text.contains("invalid-json"));
}

#[test]
fn retry_totals_count_completed_internal_retries_not_cumulative_indexes() {
    let mut conn = fixture();
    for id in 0..6 {
        seed(
            &conn,
            id,
            Some(if id == 5 { "success" } else { "overload" }),
            None,
            0,
        );
        conn.execute("UPDATE request_logs SET attempt_index=?1,overload_retry_count=?1,is_overload_retry=?2 WHERE id=?1",
            params![id,id>0]).unwrap();
    }
    // A later cross-account failover inherits the cumulative count but is not
    // another internal retry. Legacy records have no reliable per-attempt flag.
    for id in 6..8 {
        seed(&conn, id, Some("success"), None, 0);
    }
    conn.execute("UPDATE request_logs SET attempt_index=6,overload_retry_count=5,is_overload_retry=0 WHERE id='6'",[]).unwrap();
    conn.execute(
        "UPDATE request_logs SET attempt_index=1,overload_retry_count=5 WHERE id='7'",
        [],
    )
    .unwrap();
    let h = history(&mut conn, &AnalyticsQuery::default(), now()).unwrap();
    assert_eq!(h.summary.retry_attempts, 7);
    assert_eq!(h.summary.overload_retries, 5);
    assert_eq!(
        h.buckets
            .iter()
            .map(|b| b.stats.overload_retries)
            .sum::<u64>(),
        5
    );
}

#[test]
fn empty_buckets_have_no_fabricated_performance_or_rates() {
    let mut conn = fixture();
    for (range, count) in [
        (AnalyticsRange::Hour, 60),
        (AnalyticsRange::Day, 288),
        (AnalyticsRange::Week, 168),
    ] {
        let h = history(
            &mut conn,
            &AnalyticsQuery {
                range,
                ..Default::default()
            },
            now(),
        )
        .unwrap();
        assert_eq!(h.buckets.len(), count);
        assert_eq!(h.summary.attempts, 0);
        assert!(h.buckets.iter().all(|b| b.stats.ttft.p50.is_none()));
        assert_eq!(h.buckets.iter().filter(|b| b.partial).count(), 1);
    }
}

#[test]
fn ranges_filters_and_other_preserve_totals() {
    let mut conn = fixture();
    for id in 0..15 {
        seed(&conn, id, Some("success"), None, 0);
    }
    conn.execute(
        "UPDATE request_logs SET model='model-'||id,provider_account_id='account-'||id",
        [],
    )
    .unwrap();
    let h = history(&mut conn, &AnalyticsQuery::default(), now()).unwrap();
    assert_eq!(h.models.len(), 11);
    assert!(h.models[10].other);
    assert_eq!(h.models.iter().map(|g| g.attempts).sum::<u64>(), 15);
    let q = AnalyticsQuery {
        account_id: Some("account-3".into()),
        ..Default::default()
    };
    let h = history(&mut conn, &q, now()).unwrap();
    assert_eq!(h.summary.attempts, 1);
    assert_eq!(h.models[0].id.as_deref(), Some("model-3"));
    let h = history(
        &mut conn,
        &AnalyticsQuery {
            model: Some("' OR 1=1 --".into()),
            ..Default::default()
        },
        now(),
    )
    .unwrap();
    assert_eq!(h.summary.attempts, 0);
}

#[test]
fn bucket_boundaries_and_missing_metrics() {
    let mut conn = fixture();
    seed(&conn, 1, Some("success"), None, 0);
    conn.execute(
        "UPDATE request_logs SET created_at='2026-10-19T12:00:00+00:00',ttft_ms=NULL,otps=NULL",
        [],
    )
    .unwrap();
    let h = history(
        &mut conn,
        &AnalyticsQuery {
            range: AnalyticsRange::Hour,
            ..Default::default()
        },
        now(),
    )
    .unwrap();
    assert_eq!(h.buckets[58].stats.attempts, 1);
    assert_eq!(h.buckets[59].stats.attempts, 0);
    assert_eq!(h.summary.ttft.samples, 0);
    assert_eq!(h.summary.otps.p50, None);
}

#[test]
fn historical_options_are_searchable_paginated_and_include_deleted_accounts() {
    let conn = fixture();
    for id in 0..60 {
        seed(&conn, id, Some("success"), None, 0);
    }
    conn.execute(
        "UPDATE request_logs SET provider_account_id='account-'||id",
        [],
    )
    .unwrap();
    let mut q = AnalyticsOptionsQuery {
        range: AnalyticsRange::Day,
        dimension: AnalyticsDimension::Account,
        search: String::new(),
        page: 0,
    };
    let options = options(&conn, &q, now()).unwrap();
    assert_eq!(options.data.len(), 50);
    assert!(options.has_more);
    assert!(options.data.iter().all(|g| g.deleted));
    q.page = 1;
    assert_eq!(super::options(&conn, &q, now()).unwrap().data.len(), 10);
    q.page = 0;
    q.search = "account-59".into();
    assert_eq!(super::options(&conn, &q, now()).unwrap().data.len(), 1);
}

#[tokio::test]
async fn migrated_named_memory_reader_caches_and_validates_filters() {
    let db = crate::db::Db::open(std::path::Path::new(":memory:"))
        .await
        .unwrap();
    let reader = db.analytics.clone();
    let a = reader.history(AnalyticsQuery::default()).await.unwrap();
    assert_eq!(a.summary.attempts, 0);
    let b = reader.history(AnalyticsQuery::default()).await.unwrap();
    assert_eq!(a.generated_at, b.generated_at);
    assert_eq!(reader.cache.lock().unwrap().len(), 1);
    assert!(
        AnalyticsQuery {
            model: Some("x".repeat(201)),
            ..Default::default()
        }
        .validate()
        .is_err()
    );
}

#[test]
fn scoped_queries_use_the_time_index() {
    let conn = fixture();
    let sql = format!("EXPLAIN QUERY PLAN {SCOPED} SELECT COUNT(*) FROM scoped");
    let plan = conn
        .prepare(&sql)
        .unwrap()
        .query_map(
            params![
                "2026-10-19",
                "2026-10-20",
                None::<String>,
                None::<String>,
                0,
                60
            ],
            |row| row.get::<_, String>(3),
        )
        .unwrap()
        .collect::<rusqlite::Result<Vec<_>>>()
        .unwrap()
        .join(" ");
    assert!(plan.contains("idx_request_logs_created_at"), "{plan}");
}

#[test]
fn production_scale_aggregation_has_bounded_output() {
    let mut conn = fixture();
    conn.execute_batch("WITH RECURSIVE n(x) AS (VALUES(1) UNION ALL SELECT x+1 FROM n WHERE x<250000)
      INSERT INTO request_logs(id,model,provider_account_id,created_at,outcome,transport,request_summary,ttft_ms)
      SELECT x,'model-'||(x%20),'account-'||(x%15),'2026-10-19T12:00:00+00:00','success','http_sse','{\"usageComplete\":true}',x%5000 FROM n;").unwrap();
    let started = std::time::Instant::now();
    let h = history(
        &mut conn,
        &AnalyticsQuery {
            range: AnalyticsRange::Week,
            ..Default::default()
        },
        now(),
    )
    .unwrap();
    assert_eq!(h.summary.attempts, 250000);
    assert_eq!(h.buckets.len(), 168);
    assert_eq!(h.accounts.len(), 11);
    assert_eq!(h.summary.ttft.samples, 250000);
    eprintln!(
        "analytics 250k attempts: {:?}, response {} bytes",
        started.elapsed(),
        serde_json::to_vec(&h).unwrap().len()
    );
}
