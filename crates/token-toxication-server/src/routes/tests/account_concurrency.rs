use super::*;
use crate::models::UpdateProviderAccountRequest;
use axum::body::Body;
use tokio::sync::{Notify, mpsc};

/// Upstream that holds every request open until `release` is notified, and
/// reports each arrival on `arrivals`.
async fn gated_upstream(
    release: Arc<Notify>,
    arrivals: mpsc::UnboundedSender<String>,
    label: &'static str,
) -> (SocketAddr, tokio::task::JoinHandle<()>) {
    let upstream = Router::new().route(
        "/chat/completions",
        post(move || {
            let release = Arc::clone(&release);
            let arrivals = arrivals.clone();
            async move {
                arrivals.send(label.to_string()).ok();
                release.notified().await;
                Json(json!({"id": "chat", "usage": {"prompt_tokens": 1, "completion_tokens": 1}}))
            }
        }),
    );
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let address = listener.local_addr().expect("address");
    let server = tokio::spawn(async move {
        axum::serve(listener, upstream).await.expect("serve");
    });
    (address, server)
}

async fn limit_account(db: &Db, account_id: &str, limit: u32) {
    db.update_provider_account(
        account_id,
        UpdateProviderAccountRequest {
            name: None,
            provider: None,
            base_url: None,
            auth_mode: None,
            wire_api: None,
            api_key: None,
            is_active: None,
            max_running_requests: Some(limit),
        },
    )
    .await
    .expect("set limit");
}

fn chat_request(state: &AppState, secret: &str, stream: bool) -> tokio::task::JoinHandle<Response> {
    let handle = chat_result(state, secret, stream);
    tokio::spawn(async move { handle.await.unwrap().expect("relay response") })
}

fn chat_result(
    state: &AppState,
    secret: &str,
    stream: bool,
) -> tokio::task::JoinHandle<Result<Response, AppError>> {
    let state = state.clone();
    let headers = relay_headers(secret);
    let body = if stream {
        br#"{"model":"public-chat-model","stream":true,"messages":[{"role":"user","content":"hi"}]}"#
            .as_slice()
    } else {
        br#"{"model":"public-chat-model","messages":[{"role":"user","content":"hi"}]}"#.as_slice()
    };
    tokio::spawn(async move {
        relay_openai_chat(
            State(state),
            headers,
            Uri::from_static("/openai/v1/chat/completions"),
            Bytes::from_static(body),
        )
        .await
    })
}

async fn wait_for_load(state: &AppState, account_id: &str, running: u32, queued: u32) {
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let load = state.account_concurrency.load(account_id);
            if (load.running, load.queued) == (running, queued) {
                return;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .unwrap_or_else(|_| {
        panic!(
            "expected running={running} queued={queued}, got {:?}",
            state.account_concurrency.load(account_id)
        )
    });
}

async fn seed(address: SocketAddr) -> (Db, PathBuf, SeededRelayRoute) {
    let database_path = test_database_path();
    let db = Db::open(&database_path).await.expect("open test database");
    let seeded = seed_relay_route(
        &db,
        RelayRouteSeed {
            base_url: format!("http://{address}"),
            provider: "openai-compatible",
            auth_mode: "bearer",
            provider_secret: "provider-secret".to_string(),
            wire_api: "openai-chat",
            public_model: "public-chat-model",
            upstream_model: "upstream-chat-model",
        },
    )
    .await;
    (db, database_path, seeded)
}

#[tokio::test]
async fn requests_queue_until_a_running_slot_is_released() {
    let release = Arc::new(Notify::new());
    let (arrivals_tx, mut arrivals) = mpsc::unbounded_channel();
    let (address, server) = gated_upstream(Arc::clone(&release), arrivals_tx, "a").await;
    let (db, database_path, seeded) = seed(address).await;
    limit_account(&db, &seeded.account_id, 1).await;
    let state = test_state(db, database_path.clone());

    let first = chat_request(&state, &seeded.relay_secret, false);
    arrivals.recv().await.expect("first reaches upstream");
    let second = chat_request(&state, &seeded.relay_secret, false);
    wait_for_load(&state, &seeded.account_id, 1, 1).await;
    assert!(
        arrivals.try_recv().is_err(),
        "queued request must not reach upstream"
    );

    release.notify_one();
    assert_eq!(first.await.unwrap().status(), StatusCode::OK);
    arrivals
        .recv()
        .await
        .expect("second reaches upstream after release");
    wait_for_load(&state, &seeded.account_id, 1, 0).await;
    release.notify_one();
    assert_eq!(second.await.unwrap().status(), StatusCode::OK);
    wait_for_load(&state, &seeded.account_id, 0, 0).await;

    server.abort();
    remove_test_database(&database_path);
}

#[tokio::test]
async fn cancelling_a_queued_request_frees_its_position() {
    let release = Arc::new(Notify::new());
    let (arrivals_tx, mut arrivals) = mpsc::unbounded_channel();
    let (address, server) = gated_upstream(Arc::clone(&release), arrivals_tx, "a").await;
    let (db, database_path, seeded) = seed(address).await;
    limit_account(&db, &seeded.account_id, 1).await;
    let state = test_state(db, database_path.clone());

    let first = chat_request(&state, &seeded.relay_secret, false);
    arrivals.recv().await.expect("first reaches upstream");
    // Spawn the handler directly so aborting it drops the handler future.
    let queued = chat_result(&state, &seeded.relay_secret, false);
    wait_for_load(&state, &seeded.account_id, 1, 1).await;

    // Client disconnect drops the handler future.
    queued.abort();
    let _ = queued.await;
    wait_for_load(&state, &seeded.account_id, 1, 0).await;

    release.notify_one();
    assert_eq!(first.await.unwrap().status(), StatusCode::OK);
    wait_for_load(&state, &seeded.account_id, 0, 0).await;
    assert!(
        arrivals.try_recv().is_err(),
        "cancelled request must never be sent"
    );

    server.abort();
    remove_test_database(&database_path);
}

#[tokio::test]
async fn saturated_account_overflows_to_same_tier_account() {
    let release = Arc::new(Notify::new());
    let (arrivals_tx, mut arrivals) = mpsc::unbounded_channel();
    let (address_a, server_a) =
        gated_upstream(Arc::clone(&release), arrivals_tx.clone(), "a").await;
    let (address_b, server_b) = gated_upstream(Arc::clone(&release), arrivals_tx, "b").await;
    let (db, database_path, seeded) = seed(address_a).await;
    limit_account(&db, &seeded.account_id, 1).await;
    let second_account = db
        .create_provider_account(CreateProviderAccountRequest {
            name: "second provider".to_string(),
            provider: "openai-compatible".to_string(),
            base_url: format!("http://{address_b}"),
            auth_mode: "bearer".to_string(),
            wire_api: "openai-chat".to_string(),
            api_key: "provider-secret".to_string(),
            is_active: true,
            max_running_requests: 1,
        })
        .await
        .expect("create second account");
    db.create_provider_model_route(CreateProviderModelRouteRequest {
        public_model_id: "public-chat-model".to_string(),
        provider_account_id: second_account.id.clone(),
        upstream_model_id: "upstream-chat-model".to_string(),
        wire_api: "openai-chat".to_string(),
        role: "primary".to_string(),
        enabled: true,
        weight: 100,
        strip_params: Vec::new(),
    })
    .await
    .expect("create second route");
    let state = test_state(db, database_path.clone());

    let first = chat_request(&state, &seeded.relay_secret, false);
    let second = chat_request(&state, &seeded.relay_secret, false);
    let mut seen = vec![
        arrivals.recv().await.expect("first arrival"),
        arrivals.recv().await.expect("second arrival"),
    ];
    seen.sort();
    assert_eq!(
        seen,
        vec!["a", "b"],
        "second request must overflow, not queue"
    );

    let third = chat_request(&state, &seeded.relay_secret, false);
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let queued = state.account_concurrency.load(&seeded.account_id).queued
                + state.account_concurrency.load(&second_account.id).queued;
            if queued == 1 {
                return;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .expect("third request queues once the tier is saturated");

    // Release the two running requests; the queued one then reaches upstream.
    release.notify_one();
    release.notify_one();
    arrivals
        .recv()
        .await
        .expect("queued request reaches upstream");
    release.notify_one();
    for handle in [first, second, third] {
        assert_eq!(handle.await.unwrap().status(), StatusCode::OK);
    }
    wait_for_load(&state, &seeded.account_id, 0, 0).await;
    wait_for_load(&state, &second_account.id, 0, 0).await;

    server_a.abort();
    server_b.abort();
    remove_test_database(&database_path);
}

#[tokio::test]
async fn streaming_response_holds_its_slot_until_the_body_ends() {
    let (chunks_tx, chunks_rx) = mpsc::unbounded_channel::<Result<Bytes, Infallible>>();
    let chunks_rx = Arc::new(Mutex::new(Some(chunks_rx)));
    let upstream = Router::new().route(
        "/chat/completions",
        post(move || {
            let chunks_rx = Arc::clone(&chunks_rx);
            async move {
                let receiver = chunks_rx.lock().await.take().expect("single stream");
                let stream = futures_util::stream::unfold(receiver, |mut receiver| async move {
                    receiver.recv().await.map(|chunk| (chunk, receiver))
                });
                Response::builder()
                    .header(header::CONTENT_TYPE, "text/event-stream")
                    .body(Body::from_stream(stream))
                    .unwrap()
            }
        }),
    );
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let address = listener.local_addr().expect("address");
    let server = tokio::spawn(async move {
        axum::serve(listener, upstream).await.expect("serve");
    });
    let (db, database_path, seeded) = seed(address).await;
    limit_account(&db, &seeded.account_id, 1).await;
    let state = test_state(db, database_path.clone());

    chunks_tx
        .send(Ok(Bytes::from_static(
            b"data: {\"choices\":[{\"delta\":{\"content\":\"hi\"}}]}\n\n",
        )))
        .unwrap();
    let response = chat_request(&state, &seeded.relay_secret, true)
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    // Headers have been returned but the body is still streaming.
    wait_for_load(&state, &seeded.account_id, 1, 0).await;

    let body = tokio::spawn(to_bytes(response.into_body(), usize::MAX));
    chunks_tx
        .send(Ok(Bytes::from_static(b"data: [DONE]\n\n")))
        .unwrap();
    drop(chunks_tx);
    body.await.unwrap().expect("read stream");
    wait_for_load(&state, &seeded.account_id, 0, 0).await;

    server.abort();
    remove_test_database(&database_path);
}

#[tokio::test]
async fn dropping_a_streaming_body_releases_its_slot() {
    let (chunks_tx, chunks_rx) = mpsc::unbounded_channel::<Result<Bytes, Infallible>>();
    let chunks_rx = Arc::new(Mutex::new(Some(chunks_rx)));
    let upstream = Router::new().route(
        "/chat/completions",
        post(move || {
            let chunks_rx = Arc::clone(&chunks_rx);
            async move {
                let receiver = chunks_rx.lock().await.take().expect("single stream");
                let stream = futures_util::stream::unfold(receiver, |mut receiver| async move {
                    receiver.recv().await.map(|chunk| (chunk, receiver))
                });
                Response::builder()
                    .header(header::CONTENT_TYPE, "text/event-stream")
                    .body(Body::from_stream(stream))
                    .unwrap()
            }
        }),
    );
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let address = listener.local_addr().expect("address");
    let server = tokio::spawn(async move {
        axum::serve(listener, upstream).await.expect("serve");
    });
    let (db, database_path, seeded) = seed(address).await;
    limit_account(&db, &seeded.account_id, 1).await;
    let state = test_state(db, database_path.clone());

    chunks_tx
        .send(Ok(Bytes::from_static(
            b"data: {\"choices\":[{\"delta\":{\"content\":\"hi\"}}]}\n\n",
        )))
        .unwrap();
    let response = chat_request(&state, &seeded.relay_secret, true)
        .await
        .unwrap();
    wait_for_load(&state, &seeded.account_id, 1, 0).await;

    // Client disconnects mid-stream; upstream stays open.
    drop(response);
    wait_for_load(&state, &seeded.account_id, 0, 0).await;

    drop(chunks_tx);
    server.abort();
    remove_test_database(&database_path);
}

#[tokio::test]
async fn shutdown_fails_queued_requests_with_503() {
    let release = Arc::new(Notify::new());
    let (arrivals_tx, mut arrivals) = mpsc::unbounded_channel();
    let (address, server) = gated_upstream(Arc::clone(&release), arrivals_tx, "a").await;
    let (db, database_path, seeded) = seed(address).await;
    limit_account(&db, &seeded.account_id, 1).await;
    let state = test_state(db, database_path.clone());

    let first = chat_request(&state, &seeded.relay_secret, false);
    arrivals.recv().await.expect("first reaches upstream");
    let queued = chat_result(&state, &seeded.relay_secret, false);
    wait_for_load(&state, &seeded.account_id, 1, 1).await;

    state.shutdown.cancel();
    let error = tokio::time::timeout(Duration::from_secs(5), queued)
        .await
        .expect("queued request ends on shutdown")
        .unwrap()
        .expect_err("queued request must fail");
    assert_eq!(error.status(), StatusCode::SERVICE_UNAVAILABLE);
    wait_for_load(&state, &seeded.account_id, 1, 0).await;

    // The running request is unaffected and nothing else reaches upstream.
    release.notify_one();
    assert_eq!(first.await.unwrap().status(), StatusCode::OK);
    assert!(arrivals.try_recv().is_err());
    wait_for_load(&state, &seeded.account_id, 0, 0).await;

    server.abort();
    remove_test_database(&database_path);
}

#[tokio::test]
async fn request_logs_separate_queue_wait_from_latency() {
    let release = Arc::new(Notify::new());
    let (arrivals_tx, mut arrivals) = mpsc::unbounded_channel();
    let (address, server) = gated_upstream(Arc::clone(&release), arrivals_tx, "a").await;
    let (db, database_path, seeded) = seed(address).await;
    limit_account(&db, &seeded.account_id, 1).await;
    let state = test_state(db, database_path.clone());

    let first = chat_request(&state, &seeded.relay_secret, false);
    arrivals.recv().await.expect("first reaches upstream");
    let second = chat_request(&state, &seeded.relay_secret, false);
    wait_for_load(&state, &seeded.account_id, 1, 1).await;
    tokio::time::sleep(Duration::from_millis(200)).await;
    release.notify_one();
    assert_eq!(first.await.unwrap().status(), StatusCode::OK);
    arrivals.recv().await.expect("second reaches upstream");
    release.notify_one();
    assert_eq!(second.await.unwrap().status(), StatusCode::OK);

    let logs = state.db.list_request_logs(2).await.expect("list logs");
    let queued = logs
        .iter()
        .find(|log| log.queue_wait_ms > 0)
        .expect("queued request records its wait");
    assert!(
        queued.queue_wait_ms >= 200,
        "queue wait {}",
        queued.queue_wait_ms
    );
    assert!(
        queued.latency_ms < queued.queue_wait_ms,
        "latency {} must exclude queue wait {}",
        queued.latency_ms,
        queued.queue_wait_ms
    );
    let direct = logs
        .iter()
        .find(|log| log.queue_wait_ms == 0)
        .expect("first request did not queue");
    assert!(direct.latency_ms >= 200, "running time is latency");

    server.abort();
    remove_test_database(&database_path);
}
