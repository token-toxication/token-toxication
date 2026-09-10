use super::*;
use crate::models::UpdateProviderModelRouteRequest;

// Synthetic protocol data only. Never substitute captured user requests here.
fn coding_request() -> Value {
    json!({
        "model": "public-coding",
        "instructions": "synthetic-private-instructions",
        "input": [
            {"role": "user", "content": [
                {"type": "input_text", "text": "synthetic-private-text"},
                {"type": "input_image", "image_url": "data:image/png;base64,c3ludGhldGlj", "detail": "original"}
            ]},
            {"type": "reasoning", "id": "rs_1", "summary": [], "encrypted_content": "synthetic-encrypted-reasoning"},
            {"type": "function_call", "call_id": "call_f", "name": "lookup", "arguments": "{}"},
            {"type": "function_call_output", "call_id": "call_f", "output": "synthetic-function-result"},
            {"type": "custom_tool_call", "call_id": "call_c", "name": "apply_patch", "input": "synthetic-patch"},
            {"type": "custom_tool_call_output", "call_id": "call_c", "output": "synthetic-patch-result"}
        ],
        "tools": [
            {"type": "function", "name": "lookup", "parameters": {"type": "object", "properties": {}}},
            {"type": "custom", "name": "apply_patch", "format": {"type": "text"}}
        ],
        "reasoning": {"effort": "max"},
        "text": {"verbosity": "low"},
        "parallel_tool_calls": true,
        "include": ["reasoning.encrypted_content"],
        "store": false,
        "stream": true,
        "max_output_tokens": 512,
        "remove_for_route": "synthetic-stripped-value",
        "future_extension": {"nested": [1, true, "preserve-me"]}
    })
}

#[tokio::test]
async fn coding_models_preserve_payloads_and_auth_boundaries() {
    for (auth_mode, upstream_model) in ["bearer", "codex-oauth"].into_iter().flat_map(|auth| {
        [
            "gpt-6-astra",
            "gpt-5.6-sol",
            "gpt-5.6-terra",
            "gpt-5.6-luna",
        ]
        .into_iter()
        .map(move |model| (auth, model))
    }) {
        let captured = Arc::new(Mutex::new(Vec::<(HeaderMap, Value)>::new()));
        let capture = captured.clone();
        let upstream_path = if auth_mode == "bearer" {
            "/v1/responses"
        } else {
            "/codex/responses"
        };
        let stream = concat!(
            "event: response.output_item.done\n",
            "data: {\"type\":\"response.output_item.done\",\"item\":{\"type\":\"custom_tool_call\",\"call_id\":\"call_next\",\"name\":\"apply_patch\",\"input\":\"synthetic-next-patch\"}}\n\n",
            "event: response.completed\n",
            "data: {\"type\":\"response.completed\",\"response\":{\"usage\":{\"input_tokens\":13,\"input_tokens_details\":{\"cached_tokens\":3},\"output_tokens\":2}}}\n\n"
        );
        let upstream = Router::new().route(
            upstream_path,
            post(move |headers: HeaderMap, Json(body): Json<Value>| {
                let capture = capture.clone();
                async move {
                    capture.lock().await.push((headers, body));
                    ([(header::CONTENT_TYPE, "text/event-stream")], stream)
                }
            }),
        );
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind upstream");
        let address = listener.local_addr().expect("upstream address");
        let server = tokio::spawn(async move { axum::serve(listener, upstream).await });
        let database_path = test_database_path();
        let db = Db::open(&database_path).await.expect("open database");
        let provider_secret = if auth_mode == "bearer" {
            "synthetic-provider-key".to_string()
        } else {
            json!({
                "type": "token-toxication-codex-oauth-v1",
                "refresh": "synthetic-refresh",
                "access": "synthetic-provider-key",
                "expires": Utc::now().timestamp_millis() + 3_600_000,
                "accountId": "synthetic-account"
            })
            .to_string()
        };
        let seed = seed_relay_route(
            &db,
            RelayRouteSeed {
                base_url: format!("http://{address}"),
                provider: "openai-compatible",
                auth_mode,
                provider_secret,
                wire_api: "openai-responses",
                public_model: "public-coding",
                upstream_model,
            },
        )
        .await;
        db.update_provider_model_route(
            &seed.route_id,
            UpdateProviderModelRouteRequest {
                public_model_id: None,
                provider_account_id: None,
                upstream_model_id: None,
                wire_api: None,
                role: None,
                enabled: None,
                strip_params: Some(vec!["remove_for_route".to_string()]),
            },
        )
        .await
        .expect("configure strip parameter");
        let state = test_state(db, database_path.clone());
        let mut headers = relay_headers(&seed.relay_secret);
        headers.insert(
            "chatgpt-account-id",
            HeaderValue::from_static("untrusted-account"),
        );
        headers.insert(
            "originator",
            HeaderValue::from_static("untrusted-originator"),
        );
        let input = coding_request();
        let response = relay_openai_responses(
            State(state.clone()),
            headers,
            Uri::from_static("/openai/v1/responses"),
            Bytes::from(serde_json::to_vec(&input).expect("serialize request")),
        )
        .await
        .expect("relay response");
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            to_bytes(response.into_body(), usize::MAX)
                .await
                .expect("body"),
            stream
        );

        let captured = captured.lock().await;
        assert_eq!(captured.len(), 1);
        let (headers, body) = &captured[0];
        let mut expected = input;
        expected["model"] = json!(upstream_model);
        let expected_object = expected.as_object_mut().expect("object");
        expected_object.remove("remove_for_route");
        if auth_mode == "codex-oauth" {
            expected_object.remove("max_output_tokens");
            assert_eq!(headers["chatgpt-account-id"], "synthetic-account");
            assert_eq!(headers["originator"], "opencode");
        } else {
            assert!(!headers.contains_key("chatgpt-account-id"));
            assert!(!headers.contains_key("originator"));
        }
        assert_eq!(*body, expected);
        assert_eq!(
            headers[header::AUTHORIZATION],
            "Bearer synthetic-provider-key"
        );

        let logs = state.db.list_request_logs(10).await.expect("logs");
        assert_eq!(logs.len(), 1);
        assert_eq!(
            (
                logs[0].input_tokens,
                logs[0].output_tokens,
                logs[0].cached_input_tokens
            ),
            (13, 2, 3)
        );
        let serialized_logs = serde_json::to_string(&logs).expect("serialize logs");
        for private_value in [
            "synthetic-private",
            "synthetic-encrypted",
            "synthetic-function-result",
            "synthetic-patch",
            "synthetic-next-patch",
            "synthetic-provider-key",
            "c3ludGhldGlj",
        ] {
            assert!(
                !serialized_logs.contains(private_value),
                "logs leaked {private_value}"
            );
        }
        server.abort();
        drop(state);
        remove_test_database(&database_path);
    }
}
