use super::*;

#[tokio::test]
async fn concurrent_refreshes_share_a_snapshot_and_cache_is_bounded() {
    let db = crate::db::Db::open(std::path::Path::new(":memory:"))
        .await
        .unwrap();
    let reader = db.analytics;
    let (a, b, c) = tokio::join!(
        reader.history(Default::default()),
        reader.history(Default::default()),
        reader.history(Default::default())
    );
    assert_eq!(a.unwrap().generated_at, b.unwrap().generated_at);
    assert!(c.is_ok());
    for i in 0..20 {
        reader
            .history(AnalyticsQuery {
                model: Some(format!("model-{i}")),
                ..Default::default()
            })
            .await
            .unwrap();
    }
    assert_eq!(reader.cache.lock().unwrap().len(), MAX_CACHE_ENTRIES);
}

#[tokio::test]
async fn cancellation_does_not_release_the_blocking_worker_permit_early() {
    let db = crate::db::Db::open(std::path::Path::new(":memory:"))
        .await
        .unwrap();
    let reader = db.analytics.clone();
    let (entered_tx, entered_rx) = tokio::sync::oneshot::channel();
    let (release_tx, release_rx) = std::sync::mpsc::channel();
    let task_reader = reader.clone();
    let task = tokio::spawn(async move {
        task_reader
            .read(move |_| {
                entered_tx.send(()).unwrap();
                release_rx.recv().unwrap();
                Ok(())
            })
            .await
    });
    entered_rx.await.unwrap();
    task.abort();
    let _ = task.await;
    assert_eq!(reader.gate.available_permits(), 0);
    release_tx.send(()).unwrap();
    let _permit = tokio::time::timeout(Duration::from_secs(2), reader.gate.acquire())
        .await
        .unwrap()
        .unwrap();
}

#[tokio::test]
async fn expensive_sql_is_interrupted_with_a_sanitized_error() {
    let db = crate::db::Db::open(std::path::Path::new(":memory:"))
        .await
        .unwrap();
    let error=db.analytics.read(|conn| {
        conn.query_row("WITH RECURSIVE n(x) AS (VALUES(1) UNION ALL SELECT x+1 FROM n WHERE x<1000000000) SELECT SUM(x) FROM n",[],|row|row.get::<_,i64>(0))
    }).await.unwrap_err();
    assert!(matches!(error, AppError::ServiceUnavailable(_)));
    assert!(!error.to_string().contains("RECURSIVE"));
    assert_eq!(db.analytics.gate.available_permits(), 1);
}
