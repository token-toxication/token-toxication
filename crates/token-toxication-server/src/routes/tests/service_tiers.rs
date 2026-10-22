use super::*;
use axum::extract::ws::{Message, WebSocketUpgrade};
use futures_util::SinkExt;
use tokio_tungstenite::{
    connect_async,
    tungstenite::{self, client::IntoClientRequest},
};

#[tokio::test]
async fn http_service_tiers_record_upstream_reports_without_inferring_or_rewriting_them() {
    for (stream, created, reported, failed, expected) in [
        (false, None, Some("default"), false, Some("default")),
        (false, None, None, false, None),
        (false, None, Some("default"), true, Some("default")),
        (
            false,
            None,
            Some("synthetic-private-tier"),
            false,
            Some("unknown"),
        ),
        (
            true,
            Some("auto"),
            Some("priority"),
            false,
            Some("priority"),
        ),
        (true, Some("ultrafast"), None, false, Some("ultrafast")),
        (true, None, None, false, None),
        (
            true,
            None,
            Some("synthetic-private-tier"),
            false,
            Some("unknown"),
        ),
        (
            true,
            Some("priority"),
            Some("default"),
            true,
            Some("default"),
        ),
    ] {
        let mut result = json!({"id":"resp_synthetic","usage":{"input_tokens":13,"input_tokens_details":{"cached_tokens":3},"output_tokens":2}});
        if let Some(tier) = reported {
            result["service_tier"] = json!(tier);
        }
        if failed {
            result["error"] = json!({"code":"server_error","message":"synthetic-private-error"});
        }
        let body = if stream {
            let mut first = json!({"type":"response.created","response":{"id":"resp_synthetic"}});
            if let Some(tier) = created {
                first["response"]["service_tier"] = json!(tier);
            }
            let last = json!({"type":if failed {"response.failed"} else {"response.completed"},"response":result});
            format!("data: {first}\n\ndata: {last}\n\n")
        } else {
            result.to_string()
        };
        let served = body.clone();
        let upstream = Router::new().route(
            "/v1/responses",
            post(move || {
                let body = served.clone();
                async move {
                    (
                        if failed && !stream {
                            StatusCode::BAD_GATEWAY
                        } else {
                            StatusCode::OK
                        },
                        [(
                            "content-type",
                            // Production can send SSE under application/json. Observe
                            // the requested stream, not a guessed content-type contract.
                            if stream && created != Some("auto") {
                                "text/event-stream"
                            } else {
                                "application/json"
                            },
                        )],
                        body,
                    )
                }
            }),
        );
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let task = tokio::spawn(async move { axum::serve(listener, upstream).await });
        let path = test_database_path();
        let db = Db::open(&path).await.unwrap();
        let seed = seed_relay_route(
            &db,
            RelayRouteSeed {
                base_url: format!("http://{address}"),
                provider: "openai-compatible",
                auth_mode: "bearer",
                provider_secret: "synthetic-provider-key".into(),
                wire_api: "openai-responses",
                public_model: "public-coding",
                upstream_model: "private-coding",
            },
        )
        .await;
        let state = test_state(db, path.clone());
        let response = relay_openai_responses(State(state.clone()),relay_headers(&seed.relay_secret),
            Uri::from_static("/openai/v1/responses"),
            Bytes::from(json!({"model":"public-coding","input":"synthetic-private-input","stream":stream,"service_tier":"priority"}).to_string())
        ).await.unwrap();
        assert_eq!(
            to_bytes(response.into_body(), usize::MAX).await.unwrap(),
            body
        );
        let logs = state.db.list_request_logs(5).await.unwrap();
        assert_eq!(logs.len(), 1);
        assert_eq!(logs[0].status_code, if failed { 502 } else { 200 });
        let stored = serde_json::to_value(&logs[0]).unwrap();
        assert_eq!(stored["requestSummary"]["serviceTier"], "priority");
        assert_eq!(
            stored["requestSummary"]["upstreamServiceTier"],
            json!(expected)
        );
        assert!(!stored.to_string().contains("synthetic-private"));
        // Existing JSON summaries survive adding the optional observation field.
        let mut legacy = stored["requestSummary"].clone();
        legacy
            .as_object_mut()
            .unwrap()
            .remove("upstreamServiceTier");
        let legacy: RequestSummary = serde_json::from_value(legacy).unwrap();
        assert!(legacy.upstream_service_tier.is_none());
        drop(state);
        let reopened = Db::open(&path).await.unwrap();
        assert_eq!(
            serde_json::to_value(&reopened.list_request_logs(1).await.unwrap()[0]).unwrap()["requestSummary"]
                ["upstreamServiceTier"],
            json!(expected)
        );
        drop(reopened);
        task.abort();
        remove_test_database(&path);
    }
}

#[tokio::test]
async fn websocket_service_tiers_are_scoped_to_each_response_and_terminal_errors() {
    let upstream = Router::new().route("/v1/responses",get(|ws:WebSocketUpgrade| async move {
        ws.on_upgrade(|mut socket| async move {
            for index in 0..3 {
                let Some(Ok(Message::Text(_)))=socket.next().await else {break};
                let mut response=json!({"id":format!("resp_{index}"),"usage":{"input_tokens":13,"input_tokens_details":{"cached_tokens":3},"output_tokens":2}});
                if index==0 { response["service_tier"]=json!("ultrafast"); }
                if index==2 { response["service_tier"]=json!("default"); response["error"]=json!({"code":"server_error","message":"synthetic-private-error"}); }
                let value=json!({"type":if index==2 {"response.failed"} else {"response.completed"},"response":response});
                socket.send(Message::Text(value.to_string().into())).await.unwrap();
            }
            while let Some(Ok(message))=socket.next().await { if matches!(message,Message::Close(_)) {break} }
        })
    }));
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let upstream_task = tokio::spawn(async move { axum::serve(listener, upstream).await });
    let path = test_database_path();
    let db = Db::open(&path).await.unwrap();
    let seed = seed_relay_route(
        &db,
        RelayRouteSeed {
            base_url: format!("http://{address}"),
            provider: "openai-compatible",
            auth_mode: "bearer",
            provider_secret: "synthetic-provider-key".into(),
            wire_api: "openai-responses",
            public_model: "public-coding",
            upstream_model: "private-coding",
        },
    )
    .await;
    let state = test_state(db, path.clone());
    let relay = crate::app(state.clone(), PathBuf::from("nonexistent-static-test-dir"));
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let mut request = format!(
        "ws://{}/openai/v1/responses",
        listener.local_addr().unwrap()
    )
    .into_client_request()
    .unwrap();
    request.headers_mut().insert(
        header::AUTHORIZATION,
        HeaderValue::from_str(&format!("Bearer {}", seed.relay_secret)).unwrap(),
    );
    let relay_task = tokio::spawn(async move { axum::serve(listener, relay).await });
    let (mut client, _) = connect_async(request).await.unwrap();
    for (index, expected) in [Some("ultrafast"), None, Some("default")]
        .into_iter()
        .enumerate()
    {
        client.send(tungstenite::Message::Text(json!({"type":"response.create","model":"public-coding","input":"synthetic-private-input","service_tier":"priority"}).to_string().into())).await.unwrap();
        let result = tokio::time::timeout(Duration::from_secs(3), client.next())
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        let result: Value = serde_json::from_str(result.to_text().unwrap()).unwrap();
        assert_eq!(result["response"]["service_tier"], json!(expected));
        let logs = state.db.list_request_logs(10).await.unwrap();
        assert_eq!(logs.len(), index + 1);
        let stored = serde_json::to_value(&logs[0]).unwrap();
        assert_eq!(stored["requestSummary"]["serviceTier"], "priority");
        assert_eq!(
            stored["requestSummary"]["upstreamServiceTier"],
            json!(expected)
        );
        assert_eq!(logs[0].status_code, if index == 2 { 502 } else { 200 });
        assert!(!stored.to_string().contains("synthetic-private"));
    }
    let _ = client.close(None).await;
    relay_task.abort();
    upstream_task.abort();
    drop(client);
    drop(state);
    remove_test_database(&path);
}

#[test]
fn service_tier_observation_follows_final_reports_and_resets_on_retries() {
    use crate::service_tier::ReportedServiceTier;
    let mut usage = UsageUpdate::default();
    usage.merge(usage_update(
        &json!({"type":"response.created","response":{"service_tier":"auto"}}),
    ));
    usage.merge(usage_update(
        &json!({"type":"response.completed","response":{"service_tier":"priority"}}),
    ));
    assert_eq!(
        usage.tokens().upstream_service_tier,
        Some(ReportedServiceTier::Priority)
    );
    usage.merge(usage_update(&json!({"usage":{"output_tokens":7}})));
    assert_eq!(
        usage.tokens().upstream_service_tier,
        Some(ReportedServiceTier::Priority)
    );
    usage = UsageUpdate::default();
    usage.merge(usage_update(
        &json!({"type":"response.failed","response":{"error":{"code":"server_error"}}}),
    ));
    assert_eq!(usage.tokens().upstream_service_tier, None);
}

#[test]
fn fragmented_sse_service_tiers_survive_usage_only_events() {
    use crate::service_tier::ReportedServiceTier;
    let data = concat!(
        "data: {\"type\":\"response.created\",\"response\":{\"service_tier\":\"auto\"}}\n\n",
        "data: {\"type\":\"response.in_progress\",\"response\":{\"service_tier\":\"priority\"}}\n\n",
        "data: {\"type\":\"response.completed\",\"response\":{\"usage\":{\"input_tokens\":13,\"input_tokens_details\":{\"cached_tokens\":3},\"output_tokens\":2}}}\n\n"
    );
    let mut observation = StreamingResponseObservation::default();
    for bytes in data.as_bytes().chunks(7) {
        observation.observe(bytes, Instant::now());
    }
    let recorded = observation.tokens();
    assert_eq!(
        recorded.upstream_service_tier,
        Some(ReportedServiceTier::Priority)
    );
    assert!(recorded.complete);
    assert_eq!(
        (
            recorded.input_tokens,
            recorded.cached_input_tokens,
            recorded.output_tokens
        ),
        (13, 3, 2)
    );
}
