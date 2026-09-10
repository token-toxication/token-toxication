use super::*;
use futures_util::{SinkExt, StreamExt};
use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpListener,
    time::timeout,
};
use tokio_tungstenite::tungstenite::Message;

#[tokio::test]
async fn validates_upgrade_and_preserves_buffered_frames() {
    for mode in [
        "valid",
        "accept",
        "duplicate",
        "upgrade",
        "connection",
        "protocol",
        "extension",
        "redirect",
        "retry",
        "not_upgrade",
        "oversized",
    ] {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let endpoint = format!("http://{}/responses", listener.local_addr().unwrap());
        let target = endpoint.clone();
        let count = Arc::new(AtomicUsize::new(0));
        let observed = count.clone();
        let server = tokio::spawn(async move {
            loop {
                let (mut io, _) = listener.accept().await.unwrap();
                observed.fetch_add(1, Ordering::SeqCst);
                let mut request = Vec::new();
                while !request.ends_with(b"\r\n\r\n") {
                    request.push(io.read_u8().await.unwrap());
                }
                let request = String::from_utf8(request).unwrap();
                assert!(request.starts_with("GET /responses HTTP/1.1\r\n"));
                let key = request
                    .lines()
                    .find_map(|line| {
                        let (name, value) = line.split_once(':')?;
                        name.eq_ignore_ascii_case("sec-websocket-key")
                            .then_some(value.trim())
                    })
                    .unwrap();
                let accept = derive_accept_key(key.as_bytes());
                let response = if mode == "redirect" {
                    format!("HTTP/1.1 302 Found\r\nLocation: {target}\r\nContent-Length: 0\r\n\r\n")
                } else if mode == "retry" {
                    "HTTP/1.1 503 Unavailable\r\nContent-Length: 0\r\n\r\n".into()
                } else if mode == "not_upgrade" {
                    "HTTP/1.1 200 OK\r\nContent-Length: 0\r\n\r\n".into()
                } else {
                    format!(
                        "HTTP/1.1 101 Switching Protocols\r\nUpgrade: {}\r\nConnection: {}\r\nSec-WebSocket-Accept: {}\r\n{}\r\n",
                        if mode == "upgrade" {
                            "not-websocket"
                        } else {
                            "WebSocket"
                        },
                        if mode == "connection" {
                            "keep-alive"
                        } else {
                            "keep-alive, Upgrade"
                        },
                        if mode == "accept" { "invalid" } else { &accept },
                        match mode {
                            "duplicate" => format!("Sec-WebSocket-Accept: {accept}\r\n"),
                            "protocol" => "Sec-WebSocket-Protocol: unrequested\r\n".into(),
                            "extension" =>
                                "Sec-WebSocket-Extensions: permessage-deflate\r\n".into(),
                            _ => String::new(),
                        }
                    )
                };
                let mut bytes = response.into_bytes();
                if mode == "valid" {
                    bytes.extend_from_slice(b"\x81\x05hello");
                }
                if mode == "oversized" {
                    bytes.extend_from_slice(b"\x82\x7f");
                    bytes.extend_from_slice(
                        &(crate::RELAY_BODY_LIMIT_BYTES as u64 + 1).to_be_bytes(),
                    );
                }
                io.write_all(&bytes).await.unwrap();
                if mode == "valid" {
                    let mut socket = WebSocketStream::from_raw_socket(io, Role::Server, None).await;
                    let message = socket.next().await.unwrap().unwrap();
                    assert_eq!(message, Message::Text("client".into()));
                    socket.send(Message::Text("reply".into())).await.unwrap();
                }
            }
        });
        let client = build_client(Duration::from_secs(2)).unwrap();
        let result = timeout(
            Duration::from_secs(3),
            connect(&client, &endpoint, HeaderMap::new()),
        )
        .await
        .unwrap();
        match mode {
            "oversized" => {
                let mut socket = result.unwrap();
                assert!(matches!(
                    socket.next().await,
                    Some(Err(tokio_tungstenite::tungstenite::Error::Capacity(_)))
                ));
            }
            "valid" => {
                let mut socket = result.unwrap();
                assert_eq!(
                    socket.next().await.unwrap().unwrap(),
                    Message::Text("hello".into())
                );
                socket.send(Message::Text("client".into())).await.unwrap();
                assert_eq!(
                    socket.next().await.unwrap().unwrap(),
                    Message::Text("reply".into())
                );
            }
            "redirect" => assert!(matches!(
                result,
                Err(ConnectError::Http {
                    status: StatusCode::FOUND,
                    ..
                })
            )),
            "retry" => assert!(matches!(
                result,
                Err(ConnectError::Http {
                    status: StatusCode::SERVICE_UNAVAILABLE,
                    ..
                })
            )),
            _ => assert!(
                matches!(result, Err(ConnectError::Handshake)),
                "{mode}: {result:?}"
            ),
        }
        assert_eq!(count.load(Ordering::SeqCst), 1, "{mode} must not reconnect");
        server.abort();
    }
}

#[tokio::test]
async fn tls_upgrade_verifies_certificates_and_transfers_frames() {
    use axum::{Router, extract::ws::WebSocketUpgrade, routing::get};
    let key = rcgen::KeyPair::generate().unwrap();
    let cert = rcgen::CertificateParams::new(vec!["localhost".into()])
        .unwrap()
        .self_signed(&key)
        .unwrap();
    let config = axum_server::tls_rustls::RustlsConfig::from_pem(
        cert.pem().into_bytes(),
        key.serialize_pem().into_bytes(),
    )
    .await
    .unwrap();
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    listener.set_nonblocking(true).unwrap();
    let port = listener.local_addr().unwrap().port();
    let app = Router::new().route(
        "/responses",
        get(|ws: WebSocketUpgrade| async {
            ws.on_upgrade(|mut socket| async move {
                if let Some(Ok(message)) = socket.next().await {
                    socket.send(message).await.unwrap();
                }
            })
        }),
    );
    let server = tokio::spawn(
        axum_server::from_tcp_rustls(listener, config)
            .unwrap()
            .serve(app.into_make_service()),
    );
    let endpoint = format!("https://localhost:{port}/responses");
    let untrusted = build_client(Duration::from_secs(3)).unwrap();
    assert!(matches!(
        connect(&untrusted, &endpoint, HeaderMap::new()).await,
        Err(ConnectError::Transport)
    ));
    let tls =
        aioduct::tls::RustlsConnector::with_extra_roots(&[aioduct::tls::Certificate::from_der(
            cert.der().to_vec(),
        )]);
    let trusted = build_with_tls(Duration::from_secs(3), tls).unwrap();
    let mut socket = connect(&trusted, &endpoint, HeaderMap::new())
        .await
        .unwrap();
    socket
        .send(Message::Text("tls-frame".into()))
        .await
        .unwrap();
    assert_eq!(
        socket.next().await.unwrap().unwrap(),
        Message::Text("tls-frame".into())
    );
    assert!(matches!(
        connect(
            &trusted,
            &format!("https://127.0.0.1:{port}/responses"),
            HeaderMap::new()
        )
        .await,
        Err(ConnectError::Transport)
    ));
    server.abort();
}

#[tokio::test]
async fn cancelling_stalled_upgrade_releases_connection() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let endpoint = format!("http://{}/responses", listener.local_addr().unwrap());
    let (started, ready) = tokio::sync::oneshot::channel();
    let server = tokio::spawn(async move {
        let (mut io, _) = listener.accept().await.unwrap();
        let mut request = Vec::new();
        while !request.ends_with(b"\r\n\r\n") {
            request.push(io.read_u8().await.unwrap());
        }
        started.send(()).unwrap();
        let mut byte = [0];
        assert_eq!(
            timeout(Duration::from_secs(2), io.read(&mut byte))
                .await
                .unwrap()
                .unwrap(),
            0
        );
    });
    let client = build_client(Duration::from_secs(5)).unwrap();
    let request_client = client.clone();
    let request =
        tokio::spawn(async move { connect(&request_client, &endpoint, HeaderMap::new()).await });
    timeout(Duration::from_secs(2), ready)
        .await
        .unwrap()
        .unwrap();
    request.abort();
    let _ = request.await;
    timeout(Duration::from_secs(3), server)
        .await
        .unwrap()
        .unwrap();
}
