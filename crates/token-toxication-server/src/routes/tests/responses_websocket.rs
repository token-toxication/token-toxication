use super::*;
use axum::extract::ws::{Message, WebSocketUpgrade};
use axum::response::IntoResponse;
use futures_util::SinkExt;
use tokio_tungstenite::{
    connect_async,
    tungstenite::{self, client::IntoClientRequest},
};

#[tokio::test]
async fn websocket_failures_preserve_status_health_and_privacy() {
    for (mode, status) in [
        ("handshake", 401),
        ("handshake", 403),
        ("handshake", 429),
        ("handshake", 503),
        ("refresh", 401),
        ("refresh", 403),
        ("refresh", 500),
        ("application", 429),
        ("application", 502),
        ("wrapped", 429),
        ("wrapped_alias", 403),
        ("wrapped_code", 429),
    ] {
        let upstream = Router::new()
            .route("/oauth/token", post(move || async move {
                (StatusCode::from_u16(status).unwrap(), "synthetic-private-auth-error")
            }))
            .route("/v1/responses", get(move |ws: WebSocketUpgrade| async move {
                if mode == "handshake" {
                    return (StatusCode::from_u16(status).unwrap(), "synthetic-private-handshake").into_response();
                }
                ws.on_upgrade(move |mut socket| async move {
                    let _ = socket.next().await;
                    let code = if status == 429 { "rate_limit_exceeded" } else { "server_error" };
                    let event = if mode.starts_with("wrapped") {
                        let mut event = json!({"type":"error","error":{"code":code,"message":"synthetic-private-error"}});
                        if mode != "wrapped_code" {
                            event[if mode == "wrapped_alias" { "status_code" } else { "status" }] = json!(status);
                        }
                        event
                    } else {
                        json!({"type":"response.failed","response":{"id":"resp_failure","error":{"code":code,"message":"synthetic-private-error"},"usage":{"input_tokens":7,"output_tokens":2}}})
                    };
                    socket.send(Message::Text(event.to_string().into())).await.unwrap();
                    while let Some(Ok(message)) = socket.next().await {
                        if matches!(message, Message::Close(_)) { break; }
                    }
                })
            }));
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base_url = format!("http://{}", listener.local_addr().unwrap());
        let upstream_task = tokio::spawn(async move { axum::serve(listener, upstream).await });
        let database_path = test_database_path();
        let db = Db::open(&database_path).await.unwrap();
        let secret = if mode == "refresh" {
            json!({"type":"token-toxication-codex-oauth-v1","refresh":"synthetic-refresh","issuer":base_url}).to_string()
        } else {
            "synthetic-provider-key".into()
        };
        let seed = seed_relay_route(
            &db,
            RelayRouteSeed {
                base_url,
                provider: "openai-compatible",
                auth_mode: if mode == "refresh" {
                    "codex-oauth"
                } else {
                    "bearer"
                },
                provider_secret: secret,
                wire_api: "openai-responses",
                public_model: "public-coding",
                upstream_model: "mock",
            },
        )
        .await;
        let state = test_state(db, database_path.clone());
        let relay = crate::app(state.clone(), PathBuf::from("nonexistent-static-test-dir"));
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let mut request = format!(
            "ws://{}/openai/v1/responses",
            listener.local_addr().unwrap()
        )
        .into_client_request()
        .unwrap();
        let relay_task = tokio::spawn(async move { axum::serve(listener, relay).await });
        request.headers_mut().insert(
            header::AUTHORIZATION,
            HeaderValue::from_str(&format!("Bearer {}", seed.relay_secret)).unwrap(),
        );
        let (mut client, _) = connect_async(request).await.unwrap();
        client
            .send(tungstenite::Message::Text(
                json!({"type":"response.create","model":"public-coding","input":"synthetic"})
                    .to_string()
                    .into(),
            ))
            .await
            .unwrap();
        let events = tokio::time::timeout(Duration::from_secs(3), async {
            let mut events = Vec::<Value>::new();
            while let Some(Ok(message)) = client.next().await {
                if let tungstenite::Message::Text(text) = message {
                    events.push(serde_json::from_str(&text).unwrap());
                } else if matches!(message, tungstenite::Message::Close(_)) {
                    break;
                }
            }
            events
        })
        .await
        .unwrap();
        let expected = if mode == "refresh" && status == 403 {
            401
        } else {
            status
        };
        let logs = state.db.list_request_logs(10).await.unwrap();
        assert_eq!(logs.len(), 1, "{mode}/{status}");
        assert_eq!(logs[0].status_code, expected, "{mode}/{status}");
        assert_eq!(
            logs[0].input_tokens,
            if mode == "application" { 7 } else { 0 }
        );
        let route = state
            .db
            .get_provider_model_route(&seed.route_id)
            .await
            .unwrap()
            .unwrap();
        let account = state
            .db
            .get_provider_account(&seed.account_id)
            .await
            .unwrap()
            .unwrap();
        if matches!(expected, 401 | 403) {
            assert_eq!(account.status, "blocked", "{mode}/{status}");
            assert_eq!(route.status, "degraded");
        } else {
            assert_eq!(route.status, "cooling_down", "{mode}/{status}");
            assert!(route.cooldown_until.is_some());
        }
        assert_eq!(route.last_status_code, Some(expected));
        assert_eq!(events.len(), 1, "one terminal error: {mode}/{status}");
        if matches!(mode, "handshake" | "refresh") {
            assert_eq!(events[0]["status"], expected);
            assert!(events[0]["error"]["code"].as_str().is_some());
            assert!(!events[0].to_string().contains("synthetic-private"));
        }
        assert!(
            !serde_json::to_string(&logs)
                .unwrap()
                .contains("synthetic-private")
        );
        assert!(
            !serde_json::to_string(&account)
                .unwrap()
                .contains("synthetic-private")
        );
        relay_task.abort();
        upstream_task.abort();
        drop(client);
        drop(state);
        remove_test_database(&database_path);
    }
}

#[tokio::test]
async fn websocket_terminations_record_once_and_close_upstream() {
    for mode in [
        "disconnect",
        "shutdown",
        "timeout",
        "failed",
        "incomplete",
        "overlap",
    ] {
        let closed = Arc::new(tokio::sync::Notify::new());
        let server_closed = closed.clone();
        let upstream = Router::new().route("/v1/responses", get(move |ws: WebSocketUpgrade| {
            let closed = server_closed.clone();
            async move { ws.on_upgrade(move |mut socket| async move {
                let _ = socket.next().await;
                let created = json!({"type":"response.created","response":{"id":"resp_test"}});
                let _ = socket.send(Message::Text(created.to_string().into())).await;
                if matches!(mode, "failed" | "incomplete") {
                    let event = json!({"type":format!("response.{mode}"),"response":{"id":"resp_test","error":{"code":"server_error","message":"synthetic-private-error"},"usage":{"input_tokens":7,"output_tokens":2}}});
                    let _ = socket.send(Message::Text(event.to_string().into())).await;
                }
                while let Some(Ok(message)) = socket.next().await {
                    if matches!(message, Message::Close(_)) { break; }
                }
                closed.notify_one();
            }) }
        }));
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let upstream_address = listener.local_addr().unwrap();
        let upstream_task = tokio::spawn(async move { axum::serve(listener, upstream).await });
        let database_path = test_database_path();
        let db = Db::open(&database_path).await.unwrap();
        let seed = seed_relay_route(
            &db,
            RelayRouteSeed {
                base_url: format!("http://{upstream_address}"),
                provider: "openai-compatible",
                auth_mode: "bearer",
                provider_secret: "synthetic-provider-key".into(),
                wire_api: "openai-responses",
                public_model: "public-coding",
                upstream_model: "private-coding",
            },
        )
        .await;
        let mut state = test_state(db, database_path.clone());
        if mode == "timeout" {
            state.relay_stream_idle_timeout = Duration::from_millis(100);
        }
        let relay = crate::app(state.clone(), PathBuf::from("nonexistent-static-test-dir"));
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let relay_task = tokio::spawn(async move { axum::serve(listener, relay).await });
        let mut request = format!("ws://{address}/openai/v1/responses")
            .into_client_request()
            .unwrap();
        request.headers_mut().insert(
            header::AUTHORIZATION,
            HeaderValue::from_str(&format!("Bearer {}", seed.relay_secret)).unwrap(),
        );
        let (mut client, _) = connect_async(request).await.unwrap();
        let event = json!({"type":"response.create","model":"public-coding","input":"synthetic-private-input"}).to_string();
        client
            .send(tungstenite::Message::Text(event.clone().into()))
            .await
            .unwrap();
        let created = tokio::time::timeout(Duration::from_secs(3), client.next())
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        assert!(created.to_text().unwrap().contains("response.created"));
        match mode {
            "disconnect" => {
                client.close(None).await.unwrap();
            }
            "shutdown" => state.shutdown.cancel(),
            "overlap" => {
                client
                    .send(tungstenite::Message::Text(event.into()))
                    .await
                    .unwrap();
            }
            _ => {}
        }
        tokio::time::timeout(Duration::from_secs(3), closed.notified())
            .await
            .expect("upstream socket released");
        let logs = tokio::time::timeout(Duration::from_secs(3), async {
            loop {
                let logs = state.db.list_request_logs(10).await.unwrap();
                if !logs.is_empty() {
                    break logs;
                }
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        })
        .await
        .unwrap();
        assert_eq!(logs.len(), 1, "{mode}");
        let expected = match mode {
            "disconnect" | "shutdown" => 499,
            "overlap" => 400,
            _ => 502,
        };
        assert_eq!(logs[0].status_code, expected, "{mode}");
        assert_eq!(
            logs[0].input_tokens,
            if matches!(mode, "failed" | "incomplete") {
                7
            } else {
                0
            }
        );
        assert!(
            !serde_json::to_string(&logs)
                .unwrap()
                .contains("synthetic-private")
        );
        relay_task.abort();
        upstream_task.abort();
        drop(client);
        drop(state);
        remove_test_database(&database_path);
    }
}

#[tokio::test]
async fn websocket_binds_account_and_preserves_tool_and_steering_continuations() {
    for auth_mode in ["bearer", "codex-oauth"] {
        let captures = Arc::new(Mutex::new(Vec::<Value>::new()));
        let handshakes = Arc::new(Mutex::new(Vec::<HeaderMap>::new()));
        let capture = captures.clone();
        let handshake = handshakes.clone();
        let path = if auth_mode == "bearer" {
            "/v1/responses"
        } else {
            "/codex/responses"
        };
        let upstream = Router::new().route(path, get(move |headers: HeaderMap, ws: WebSocketUpgrade| {
            let capture = capture.clone();
            let handshake = handshake.clone();
            async move {
                handshake.lock().await.push(headers);
                ws.on_upgrade(move |mut socket| async move {
                    let mut index = 0;
                    while let Some(Ok(Message::Text(text))) = socket.next().await {
                        capture.lock().await.push(serde_json::from_str(&text).unwrap());
                        index += 1;
                        let value = json!({"type":"response.completed","response":{"id":format!("resp_{index}"),"usage":{"input_tokens":13,"output_tokens":2,"input_tokens_details":{"cached_tokens":3}}}});
                        socket.send(Message::Text(value.to_string().into())).await.unwrap();
                    }
                })
            }
        }));
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let upstream_address = listener.local_addr().unwrap();
        let upstream_task = tokio::spawn(async move { axum::serve(listener, upstream).await });
        let database_path = test_database_path();
        let db = Db::open(&database_path).await.unwrap();
        let secret = if auth_mode == "bearer" {
            "synthetic-provider-key".to_owned()
        } else {
            json!({"type":"token-toxication-codex-oauth-v1","refresh":"synthetic-refresh","access":"synthetic-provider-key","expires":Utc::now().timestamp_millis()+3_600_000,"accountId":"synthetic-account"}).to_string()
        };
        let seed = seed_relay_route(
            &db,
            RelayRouteSeed {
                base_url: format!("http://{upstream_address}"),
                provider: "openai-compatible",
                auth_mode,
                provider_secret: secret,
                wire_api: "openai-responses",
                public_model: "public-coding",
                upstream_model: "private-coding",
            },
        )
        .await;
        let state = test_state(db, database_path.clone());
        let relay = crate::app(state.clone(), PathBuf::from("nonexistent-static-test-dir"));
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let relay_task = tokio::spawn(async move { axum::serve(listener, relay).await });
        let url = format!("ws://{address}/openai/v1/responses");
        assert!(
            matches!(connect_async(&url).await, Err(tungstenite::Error::Http(response)) if response.status() == StatusCode::UNAUTHORIZED)
        );
        let mut request = url.into_client_request().unwrap();
        request.headers_mut().insert(
            header::AUTHORIZATION,
            HeaderValue::from_str(&format!("Bearer {}", seed.relay_secret)).unwrap(),
        );
        request.headers_mut().insert(
            "chatgpt-account-id",
            HeaderValue::from_static("untrusted-account"),
        );
        request.headers_mut().insert(
            "x-openai-internal-untrusted",
            HeaderValue::from_static("do-not-forward"),
        );
        let (mut client, _) = connect_async(request).await.unwrap();
        let base = json!({"type":"response.create","model":"public-coding","input":[{"type":"additional_tools","role":"developer","tools":[]}],"client_metadata":{"ws_request_header_x_openai_internal_codex_responses_lite":"true","ws_request_header_authorization":"synthetic-private-identity"},"max_output_tokens":100});
        client
            .send(tungstenite::Message::Text(base.to_string().into()))
            .await
            .unwrap();
        let first = tokio::time::timeout(Duration::from_secs(3), client.next())
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        assert!(first.to_text().unwrap().contains("resp_1"));
        let mut next = base.clone();
        next["previous_response_id"] = json!("resp_1");
        next["input"] = json!([
            {"type":"function_call_output","call_id":"call_pending","output":"synthetic-private-output"},
            {"role":"user","content":[{"type":"input_text","text":"synthetic-private-steering"}]}
        ]);
        client
            .send(tungstenite::Message::Text(next.to_string().into()))
            .await
            .unwrap();
        let second = tokio::time::timeout(Duration::from_secs(3), client.next())
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        assert!(second.to_text().unwrap().contains("resp_2"));
        next["previous_response_id"] = json!("another-connection-response");
        client
            .send(tungstenite::Message::Text(next.to_string().into()))
            .await
            .unwrap();
        let error = tokio::time::timeout(Duration::from_secs(3), client.next())
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        assert!(
            error
                .to_text()
                .unwrap()
                .contains("invalid WebSocket continuation")
        );
        let captures = captures.lock().await;
        assert_eq!(captures.len(), 2);
        assert_eq!(captures[0]["model"], "private-coding");
        assert_eq!(
            captures[0].get("max_output_tokens").is_some(),
            auth_mode == "bearer"
        );
        assert!(
            captures[0]["client_metadata"]
                .get("ws_request_header_authorization")
                .is_none()
        );
        assert_eq!(captures[1]["input"][0]["call_id"], "call_pending");
        assert_eq!(
            captures[1]["input"][1]["content"][0]["text"],
            "synthetic-private-steering"
        );
        let handshakes = handshakes.lock().await;
        assert_eq!(handshakes.len(), 1);
        assert_eq!(
            handshakes[0][header::AUTHORIZATION],
            "Bearer synthetic-provider-key"
        );
        assert_eq!(handshakes[0]["openai-model"], "private-coding");
        assert!(!handshakes[0].contains_key("x-openai-internal-untrusted"));
        assert_eq!(
            handshakes[0]
                .get("chatgpt-account-id")
                .map(|v| v.to_str().unwrap()),
            (auth_mode == "codex-oauth").then_some("synthetic-account")
        );
        let logs = state.db.list_request_logs(10).await.unwrap();
        assert_eq!(logs.len(), 2);
        for log in &logs {
            assert_eq!(
                (log.input_tokens, log.output_tokens, log.cached_input_tokens),
                (13, 2, 3)
            );
        }
        assert!(
            !serde_json::to_string(&logs)
                .unwrap()
                .contains("synthetic-private")
        );
        relay_task.abort();
        upstream_task.abort();
        drop(client);
        drop(state);
        remove_test_database(&database_path);
    }
}
