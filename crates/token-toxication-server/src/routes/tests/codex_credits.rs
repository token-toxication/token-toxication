use super::*;
use crate::models::UpdateProviderAccountRequest;
use axum::extract::ws::{Message, WebSocketUpgrade};
use axum::response::IntoResponse;
use futures_util::SinkExt;
use std::sync::atomic::{AtomicUsize, Ordering};
use tokio::sync::Notify;
use tokio_tungstenite::{
    connect_async,
    tungstenite::{self, client::IntoClientRequest},
};

fn quota_payload() -> Value {
    json!({"plan_type":"plus", "credits":{"has_credits":true,"balance":"100"},
        "rate_limit":{"allowed":false,"limit_reached":true,
            "primary_window":{"used_percent":100,"reset_after_seconds":3600}}})
}

fn quota_routes() -> Router {
    Router::new()
        .route(
            "/backend-api/wham/usage",
            get(|| async { Json(quota_payload()) }),
        )
        .route(
            "/backend-api/wham/rate-limit-reset-credits",
            get(|| async { Json(json!({"available_count":0,"credits":[]})) }),
        )
}

async fn seeded_state(
    upstream: Router,
    max_running: u32,
) -> (AppState, SeededRelayRoute, tokio::task::JoinHandle<()>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}/backend-api", listener.local_addr().unwrap());
    let task = tokio::spawn(async move {
        axum::serve(listener, upstream).await.unwrap();
    });
    let path = test_database_path();
    let db = Db::open(&path).await.unwrap();
    let seed = seed_relay_route(
        &db,
        RelayRouteSeed {
            base_url: base,
            provider: "codex-subscription",
            auth_mode: "codex-manual-refresh",
            provider_secret:
                json!({"type":"token-toxication-codex-oauth-v1", "refresh":"synthetic-refresh",
            "access":"synthetic-access", "expires":Utc::now().timestamp_millis()+3_600_000})
                .to_string(),
            wire_api: "openai-responses",
            public_model: "model",
            upstream_model: "upstream",
        },
    )
    .await;
    db.update_provider_account(
        &seed.account_id,
        serde_json::from_value(json!({"maxRunningRequests":max_running})).unwrap(),
    )
    .await
    .unwrap();
    (test_state(db, path), seed, task)
}

async fn set_policy(state: &AppState, id: &str, allow: bool) -> Result<ProviderAccount, AppError> {
    let input: UpdateProviderAccountRequest =
        serde_json::from_value(json!({"allowCredits":allow})).unwrap();
    update_provider_account(State(state.clone()), Path(id.to_owned()), Json(input))
        .await
        .map(|Json(response)| response.data)
}

async fn request(
    state: &AppState,
    seed: &SeededRelayRoute,
    stream: bool,
) -> Result<Response, AppError> {
    relay_openai_responses(
        State(state.clone()),
        relay_headers(&seed.relay_secret),
        Uri::from_static("/openai/v1/responses"),
        Bytes::from(json!({"model":"model","input":"synthetic","stream":stream}).to_string()),
    )
    .await
}

#[tokio::test]
async fn credits_policy_controls_http_and_sse_without_reviving_real_failures() {
    for mode in ["json", "device", "sse", "http_failure", "sse_failure"] {
        let calls = Arc::new(AtomicUsize::new(0));
        let counter = calls.clone();
        let upstream = quota_routes().route("/backend-api/codex/responses", post(move |Json(body): Json<Value>| {
            let counter = counter.clone();
            async move {
                counter.fetch_add(1, Ordering::Relaxed);
                assert_eq!(body["model"], "upstream");
                assert_eq!(body["input"], "synthetic");
                assert_eq!(body["store"], false);
                if matches!(mode, "json" | "device") { return Json(json!({"id":"synthetic","usage":{"input_tokens":1}})).into_response(); }
                if mode == "http_failure" {
                    return (StatusCode::TOO_MANY_REQUESTS, Json(json!({"error":{"code":"credit_balance_exhausted","message":"synthetic-private-error"}}))).into_response();
                }
                let terminal = if mode == "sse_failure" {
                    json!({"type":"response.failed","response":{"id":"synthetic","error":{"code":"credit_balance_exhausted","message":"synthetic-private-error"}}})
                } else { json!({"type":"response.completed","response":{"id":"synthetic","usage":{"input_tokens":1}}}) };
                ([ ("content-type", "text/event-stream") ], format!("data: {terminal}\n\n")).into_response()
            }
        }));
        let (state, seed, task) = seeded_state(upstream, 0).await;
        if mode == "device" {
            state.db.update_provider_account(&seed.account_id, serde_json::from_value(json!({
                "authMode":"codex-device-oauth", "apiKey":json!({
                    "type":"token-toxication-codex-device-oauth-v1", "refresh":"synthetic-refresh",
                    "access":"synthetic-access", "expiresAt":Utc::now().timestamp()+3600,
                    "accountId":"synthetic-account"
                }).to_string()
            })).unwrap()).await.unwrap();
        }
        let stream = matches!(mode, "sse" | "sse_failure");
        let quota = codex_account_quota(
            &state.db,
            &state.http,
            &state.codex_device_oauth,
            &seed.account_id,
        )
        .await
        .unwrap();
        assert!(quota.credits_usable);
        assert_eq!(
            request(&state, &seed, stream).await.unwrap_err().status(),
            StatusCode::TOO_MANY_REQUESTS
        );
        let account = set_policy(&state, &seed.account_id, true).await.unwrap();
        assert!(account.allow_credits);
        assert_eq!(account.status, "quota_exhausted");
        let response = request(&state, &seed, stream).await.unwrap();
        let status = response.status();
        let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        if mode == "sse_failure" {
            assert_eq!(status, StatusCode::OK);
            let original = json!({"type":"response.failed","response":{"id":"synthetic","error":{"code":"credit_balance_exhausted","message":"synthetic-private-error"}}});
            assert_eq!(body, format!("data: {original}\n\n"));
        }
        assert_eq!(calls.load(Ordering::Relaxed), 1);
        let logs = state.db.list_request_logs(10).await.unwrap();
        assert_eq!(logs.len(), 1, "{mode}");
        assert_eq!(
            logs[0].status_code,
            if mode.ends_with("failure") { 429 } else { 200 },
            "{mode}"
        );
        assert!(
            !serde_json::to_string(&logs)
                .unwrap()
                .contains("synthetic-private-error")
        );
        if mode.ends_with("failure") {
            // A fresh positive read and another opt-in do not erase rejection.
            set_policy(&state, &seed.account_id, true).await.unwrap();
        } else {
            set_policy(&state, &seed.account_id, false).await.unwrap();
        }
        assert_eq!(
            request(&state, &seed, false).await.unwrap_err().status(),
            StatusCode::TOO_MANY_REQUESTS
        );
        assert_eq!(calls.load(Ordering::Relaxed), 1);
        task.abort();
        remove_test_database(&state.config.database_path);
    }
}

#[tokio::test]
async fn credits_policy_validates_account_type_and_preserves_blocks() {
    let (state, seed, task) = seeded_state(quota_routes(), 0).await;
    for patch in [
        json!({"allowCredits":true,"name":"mixed"}),
        json!({"authMode":"bearer"}),
    ] {
        let input = serde_json::from_value(patch.clone()).unwrap();
        let result = update_provider_account(
            State(state.clone()),
            Path(seed.account_id.clone()),
            Json(input),
        )
        .await;
        if patch.get("allowCredits").is_some() {
            assert_eq!(result.unwrap_err().status(), StatusCode::BAD_REQUEST);
        } else {
            let _ = result.unwrap();
        }
    }
    assert_eq!(
        set_policy(&state, &seed.account_id, true)
            .await
            .unwrap_err()
            .status(),
        StatusCode::BAD_REQUEST
    );
    state
        .db
        .update_provider_account(
            &seed.account_id,
            serde_json::from_value(json!({"authMode":"codex-manual-refresh"})).unwrap(),
        )
        .await
        .unwrap();
    state
        .db
        .mark_provider_result(&seed.account_id, "blocked", Some("synthetic auth block"))
        .await
        .unwrap();
    assert_eq!(
        set_policy(&state, &seed.account_id, true)
            .await
            .unwrap_err()
            .status(),
        StatusCode::BAD_REQUEST
    );
    let account = set_policy(&state, &seed.account_id, false).await.unwrap();
    assert_eq!(account.status, "blocked");
    assert_eq!(account.last_error.as_deref(), Some("synthetic auth block"));
    task.abort();
    remove_test_database(&state.config.database_path);
}

#[tokio::test]
async fn failed_or_incomplete_quota_reads_do_not_authorize_credits() {
    for incomplete in [false, true] {
        let upstream = Router::new().route(
            "/backend-api/wham/usage",
            get(move || async move {
                if incomplete {
                    Json(json!({"plan_type":"plus"})).into_response()
                } else {
                    StatusCode::SERVICE_UNAVAILABLE.into_response()
                }
            }),
        );
        let (state, seed, task) = seeded_state(upstream, 0).await;
        state
            .db
            .apply_codex_quota_observation(
                &seed.account_id,
                Utc::now() - chrono::Duration::minutes(3),
                Some(("codex_usage_limit_with_credits", None)),
            )
            .await
            .unwrap();
        let result = set_policy(&state, &seed.account_id, true).await;
        if incomplete {
            assert!(result.unwrap().allow_credits);
        } else {
            assert!(result.is_err());
            assert!(
                !state
                    .db
                    .get_provider_account(&seed.account_id)
                    .await
                    .unwrap()
                    .unwrap()
                    .allow_credits
            );
        }
        assert!(
            state
                .db
                .list_provider_route_candidates("openai-responses", "model")
                .await
                .unwrap()
                .is_empty()
        );
        set_policy(&state, &seed.account_id, false).await.unwrap();
        task.abort();
        remove_test_database(&state.config.database_path);
    }
}

#[tokio::test]
async fn disabling_credits_rechecks_queued_requests_without_cancelling_running_work() {
    let arrived = Arc::new(Notify::new());
    let release = Arc::new(Notify::new());
    let calls = Arc::new(AtomicUsize::new(0));
    let (a, r, c) = (arrived.clone(), release.clone(), calls.clone());
    let upstream = quota_routes().route(
        "/backend-api/codex/responses",
        post(move || {
            let (a, r, c) = (a.clone(), r.clone(), c.clone());
            async move {
                c.fetch_add(1, Ordering::Relaxed);
                a.notify_one();
                r.notified().await;
                Json(json!({"id":"done"}))
            }
        }),
    );
    let (state, seed, task) = seeded_state(upstream, 1).await;
    set_policy(&state, &seed.account_id, true).await.unwrap();
    let spawn_request = || {
        let state = state.clone();
        let secret = seed.relay_secret.clone();
        tokio::spawn(async move {
            relay_openai_responses(
                State(state),
                relay_headers(&secret),
                Uri::from_static("/openai/v1/responses"),
                Bytes::from_static(br#"{"model":"model","input":"synthetic"}"#),
            )
            .await
        })
    };
    let running = spawn_request();
    tokio::time::timeout(Duration::from_secs(3), arrived.notified())
        .await
        .unwrap();
    let queued = spawn_request();
    tokio::time::timeout(Duration::from_secs(3), async {
        while state.account_concurrency.load(&seed.account_id).queued == 0 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    set_policy(&state, &seed.account_id, false).await.unwrap();
    release.notify_one();
    let response = running.await.unwrap().unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    to_bytes(response.into_body(), usize::MAX).await.unwrap();
    assert_eq!(
        queued.await.unwrap().unwrap_err().status(),
        StatusCode::TOO_MANY_REQUESTS
    );
    assert_eq!(calls.load(Ordering::Relaxed), 1);
    assert_eq!(state.account_concurrency.load(&seed.account_id).running, 0);
    task.abort();
    remove_test_database(&state.config.database_path);
}

#[tokio::test]
async fn reused_websockets_recheck_credits_and_keep_the_client_connection_open() {
    for mode in ["disable", "wrapped_failure", "application_failure"] {
        let calls = Arc::new(AtomicUsize::new(0));
        let count = calls.clone();
        let upstream = quota_routes().route("/backend-api/codex/responses", get(move |ws: WebSocketUpgrade| {
            let count = count.clone();
            async move { ws.on_upgrade(move |mut socket| async move {
                while let Some(Ok(Message::Text(_))) = socket.next().await {
                    let index = count.fetch_add(1, Ordering::Relaxed);
                    let event = match mode {
                        "wrapped_failure" => json!({"type":"error","status":429,"error":{"code":"credit_balance_exhausted"}}),
                        "application_failure" => json!({"type":"response.failed","response":{"id":"failed","error":{"code":"credit_balance_exhausted"}}}),
                        _ => json!({"type":"response.completed","response":{"id":format!("done_{index}"),"usage":{"input_tokens":1}}}),
                    };
                    socket.send(Message::Text(event.to_string().into())).await.unwrap();
                }
            }) }
        }));
        let (state, seed, task) = seeded_state(upstream, 0).await;
        set_policy(&state, &seed.account_id, true).await.unwrap();
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let mut ws_request = format!(
            "ws://{}/openai/v1/responses",
            listener.local_addr().unwrap()
        )
        .into_client_request()
        .unwrap();
        ws_request.headers_mut().insert(
            header::AUTHORIZATION,
            HeaderValue::from_str(&format!("Bearer {}", seed.relay_secret)).unwrap(),
        );
        let app = crate::app(state.clone(), PathBuf::from("nonexistent-static-test-dir"));
        let relay = tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        let (mut client, _) = connect_async(ws_request).await.unwrap();
        let event = tungstenite::Message::Text(
            json!({"type":"response.create","model":"model","input":"synthetic"})
                .to_string()
                .into(),
        );
        client.send(event.clone()).await.unwrap();
        let first = tokio::time::timeout(Duration::from_secs(3), client.next())
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        assert!(matches!(first, tungstenite::Message::Text(_)));
        if mode == "disable" {
            set_policy(&state, &seed.account_id, false).await.unwrap();
        } else {
            set_policy(&state, &seed.account_id, true).await.unwrap();
        }
        client.send(event.clone()).await.unwrap();
        let rejected = tokio::time::timeout(Duration::from_secs(3), client.next())
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        let tungstenite::Message::Text(text) = rejected else {
            panic!("expected request-scoped error")
        };
        let rejected: Value = serde_json::from_str(&text).unwrap();
        assert_eq!(rejected["status"], 429);
        assert_eq!(rejected["error"]["code"], "credits_policy_unavailable");
        assert_eq!(calls.load(Ordering::Relaxed), 1);
        client
            .send(tungstenite::Message::Ping(vec![1].into()))
            .await
            .unwrap();
        assert!(matches!(
            tokio::time::timeout(Duration::from_secs(3), client.next())
                .await
                .unwrap()
                .unwrap()
                .unwrap(),
            tungstenite::Message::Pong(_)
        ));
        if mode == "disable" {
            set_policy(&state, &seed.account_id, true).await.unwrap();
            client.send(event).await.unwrap();
            let resumed = tokio::time::timeout(Duration::from_secs(3), client.next())
                .await
                .unwrap()
                .unwrap()
                .unwrap();
            assert!(
                matches!(resumed, tungstenite::Message::Text(ref text) if text.contains("response.completed"))
            );
            assert_eq!(calls.load(Ordering::Relaxed), 2);
        }
        client.close(None).await.unwrap();
        relay.abort();
        task.abort();
        remove_test_database(&state.config.database_path);
    }
}

#[tokio::test]
async fn websocket_credits_rejections_before_binding_and_during_retry_are_request_scoped() {
    for mode in ["first_queued", "overload_retry"] {
        let calls = Arc::new(AtomicUsize::new(0));
        let arrived = Arc::new(Notify::new());
        let (count, notify) = (calls.clone(), arrived.clone());
        let upstream = quota_routes().route("/backend-api/codex/responses", get(move |ws: WebSocketUpgrade| {
            let (count, notify) = (count.clone(), notify.clone());
            async move { ws.on_upgrade(move |mut socket| async move {
                while let Some(Ok(Message::Text(_))) = socket.next().await {
                    let index = count.fetch_add(1, Ordering::Relaxed);
                    let event = if mode == "overload_retry" && index == 0 {
                        notify.notify_one();
                        json!({"type":"response.failed","response":{"id":"overloaded","error":{"code":"server_is_overloaded"}}})
                    } else {
                        json!({"type":"response.completed","response":{"id":format!("done_{index}")}})
                    };
                    socket.send(Message::Text(event.to_string().into())).await.unwrap();
                }
            }) }
        }));
        let (state, seed, upstream_task) = seeded_state(upstream, 1).await;
        set_policy(&state, &seed.account_id, true).await.unwrap();
        let blocker = if mode == "first_queued" {
            Some(
                state
                    .account_concurrency
                    .try_acquire(&seed.account_id, 1)
                    .unwrap(),
            )
        } else {
            None
        };
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let mut ws_request = format!(
            "ws://{}/openai/v1/responses",
            listener.local_addr().unwrap()
        )
        .into_client_request()
        .unwrap();
        ws_request.headers_mut().insert(
            header::AUTHORIZATION,
            HeaderValue::from_str(&format!("Bearer {}", seed.relay_secret)).unwrap(),
        );
        let app = crate::app(state.clone(), PathBuf::from("nonexistent-static-test-dir"));
        let relay_task = tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        let (mut client, _) = connect_async(ws_request).await.unwrap();
        let create = tungstenite::Message::Text(
            json!({"type":"response.create","model":"model","input":"synthetic"})
                .to_string()
                .into(),
        );
        client.send(create.clone()).await.unwrap();
        tokio::time::timeout(Duration::from_secs(3), async {
            if mode == "first_queued" {
                while state.account_concurrency.load(&seed.account_id).queued == 0 {
                    tokio::task::yield_now().await;
                }
            } else {
                arrived.notified().await;
            }
        })
        .await
        .unwrap();
        set_policy(&state, &seed.account_id, false).await.unwrap();
        drop(blocker);
        let message = tokio::time::timeout(Duration::from_secs(15), client.next())
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        let tungstenite::Message::Text(text) = message else {
            panic!("expected request error, not connection closure")
        };
        let event: Value = serde_json::from_str(&text).unwrap();
        assert_eq!(event["status"], 429, "{mode}: {text}");
        assert_eq!(event["error"]["code"], "credits_policy_unavailable");
        client
            .send(tungstenite::Message::Ping(vec![1].into()))
            .await
            .unwrap();
        assert!(matches!(
            tokio::time::timeout(Duration::from_secs(3), client.next())
                .await
                .unwrap()
                .unwrap()
                .unwrap(),
            tungstenite::Message::Pong(_)
        ));
        set_policy(&state, &seed.account_id, true).await.unwrap();
        client.send(create).await.unwrap();
        let response = tokio::time::timeout(Duration::from_secs(3), client.next())
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        assert!(
            matches!(response, tungstenite::Message::Text(ref text) if text.contains("response.completed"))
        );
        assert_eq!(
            calls.load(Ordering::Relaxed),
            if mode == "first_queued" { 1 } else { 2 }
        );
        client.close(None).await.unwrap();
        relay_task.abort();
        upstream_task.abort();
        remove_test_database(&state.config.database_path);
    }
}
