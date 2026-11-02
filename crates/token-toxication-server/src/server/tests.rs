use std::{io, net::SocketAddr, time::Duration};

use aioduct::tls::{Certificate, RustlsConnector, TlsConnect};
use axum::{
    Router,
    body::Bytes,
    extract::{ConnectInfo, ws::WebSocketUpgrade},
    http::{Request, StatusCode},
    response::{Sse, sse::Event},
    routing::{get, post},
};
use axum_server::{
    Handle,
    tls_rustls::{RustlsAcceptor, RustlsConfig},
};
use futures_util::{SinkExt, StreamExt, future::join_all};
use hyper_util::rt::TokioIo;
use tokio::{
    io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt},
    net::{TcpListener, TcpStream},
    task::JoinHandle,
    time::{Instant, sleep, timeout},
};

use super::{ShutdownSignal, configure_listener, serve_listener, serve_plain_listener};
use crate::acme::{ChallengeStore, http01_router};

const HEADER_WAIT: Duration = Duration::from_secs(1);
const DRAIN_WAIT: Duration = Duration::from_secs(3);
const TEST_WAIT: Duration = Duration::from_secs(4);

trait TestIo: AsyncRead + AsyncWrite + Unpin + Send {}
impl<T: AsyncRead + AsyncWrite + Unpin + Send> TestIo for T {}
type Connection = Box<dyn TestIo>;

struct RunningServer {
    addr: SocketAddr,
    shutdown: ShutdownSignal,
    task: JoinHandle<io::Result<()>>,
    tls: Option<RustlsConnector>,
}

impl RunningServer {
    async fn start(tls: bool, app: Router) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let shutdown = ShutdownSignal::for_test();
        let (task, tls) = if tls {
            let key = rcgen::KeyPair::generate().unwrap();
            let cert = rcgen::CertificateParams::new(vec!["localhost".into()])
                .unwrap()
                .self_signed(&key)
                .unwrap();
            let config = RustlsConfig::from_der(vec![cert.der().to_vec()], key.serialize_der())
                .await
                .unwrap();
            let client =
                RustlsConnector::with_extra_roots(&[Certificate::from_der(cert.der().to_vec())]);
            let server = configure_listener(
                axum_server::Server::from_listener(listener).acceptor(RustlsAcceptor::new(config)),
                HEADER_WAIT,
                "test HTTPS",
            );
            let handle = Handle::new();
            let task = tokio::spawn(serve_listener(
                server
                    .handle(handle.clone())
                    .serve(app.into_make_service_with_connect_info::<SocketAddr>()),
                handle,
                shutdown.clone(),
                DRAIN_WAIT,
            ));
            (task, Some(client))
        } else {
            (
                tokio::spawn(serve_plain_listener(
                    listener,
                    HEADER_WAIT,
                    app,
                    shutdown.clone(),
                    "test HTTP",
                )),
                None,
            )
        };
        Self {
            addr,
            shutdown,
            task,
            tls,
        }
    }

    async fn connect(&self, protocol: &[u8]) -> Connection {
        let tcp = TcpStream::connect(self.addr).await.unwrap();
        match &self.tls {
            None => Box::new(tcp),
            Some(connector) => {
                let mut connector = connector.clone();
                connector.config_mut().alpn_protocols = vec![protocol.to_vec()];
                let stream = connector
                    .connect("localhost", TokioIo::new(tcp))
                    .await
                    .unwrap();
                Box::new(TokioIo::new(stream))
            }
        }
    }

    async fn stop(mut self) {
        self.shutdown.cancel();
        timeout(TEST_WAIT, &mut self.task)
            .await
            .expect("listener must drain promptly")
            .unwrap()
            .unwrap();
    }
}

impl Drop for RunningServer {
    fn drop(&mut self) {
        self.shutdown.cancel();
        self.task.abort();
    }
}

async fn health(stream: &mut Connection) {
    stream
        .write_all(b"GET /health HTTP/1.1\r\nHost: localhost\r\n\r\n")
        .await
        .unwrap();
    assert!(
        read_until(stream, b"\r\n\r\nhealthy")
            .await
            .starts_with(b"HTTP/1.1 200")
    );
}

async fn read_until(stream: &mut Connection, suffix: &[u8]) -> Vec<u8> {
    let mut response = Vec::new();
    timeout(TEST_WAIT, async {
        while !response.ends_with(suffix) {
            response.push(stream.read_u8().await.unwrap());
        }
    })
    .await
    .expect("response marker must arrive");
    response
}

async fn closed(stream: &mut Connection) {
    let mut remaining = Vec::new();
    let result = timeout(TEST_WAIT, stream.read_to_end(&mut remaining))
        .await
        .expect("idle connection must release its socket");
    if let Err(error) = result {
        // A timeout can drop TLS without a close_notify.
        assert!(
            matches!(
                error.kind(),
                io::ErrorKind::UnexpectedEof | io::ErrorKind::ConnectionReset
            ),
            "{error}"
        );
    }
}

#[tokio::test]
async fn idle_and_partial_http1_connections_are_reclaimed() {
    for tls in [false, true] {
        let server = RunningServer::start(tls, app()).await;
        let mut idle = server.connect(b"http/1.1").await;
        health(&mut idle).await;
        health(&mut idle).await; // Reuse is supported, not disabled as a shortcut.
        let mut partial = server.connect(b"http/1.1").await;
        partial
            .write_all(b"GET /health HTTP/1.1\r\nHost: localhost\r\n")
            .await
            .unwrap();
        let mut connections = [idle, partial];
        join_all(connections.iter_mut().map(closed)).await;
        health(&mut server.connect(b"http/1.1").await).await;
        server.stop().await;
    }
}

#[tokio::test]
async fn acme_http01_uses_the_same_idle_policy() {
    let server = RunningServer::start(false, http01_router(ChallengeStore::default())).await;
    let mut stream = server.connect(b"http/1.1").await;
    stream
        .write_all(b"GET /.well-known/acme-challenge/missing HTTP/1.1\r\nHost: localhost\r\n\r\n")
        .await
        .unwrap();
    assert!(
        read_until(&mut stream, b"\r\n\r\n")
            .await
            .starts_with(b"HTTP/1.1 404")
    );
    closed(&mut stream).await;
    server.stop().await;
}

#[tokio::test]
async fn graceful_shutdown_closes_silent_connections() {
    for tls in [false, true] {
        let server = RunningServer::start(tls, app()).await;
        let mut stream = server.connect(b"http/1.1").await;
        sleep(Duration::from_millis(20)).await;
        server.shutdown.cancel();
        timeout(Duration::from_millis(500), closed(&mut stream))
            .await
            .expect("native server shutdown must close silent connections promptly");
        server.stop().await;
    }
}

fn app() -> Router {
    Router::new()
        .route(
            "/health",
            get(|ConnectInfo(peer): ConnectInfo<SocketAddr>| async move {
                assert!(peer.ip().is_loopback());
                "healthy"
            }),
        )
        .route(
            "/slow",
            get(|| async {
                sleep(HEADER_WAIT * 2).await;
                "completed"
            }),
        )
        .route("/upload", post(|body: Bytes| async move { body }))
        .route(
            "/sse",
            get(|| async {
                Sse::new(futures_util::stream::unfold(0, |n| async move {
                    if n == 2 {
                        return None;
                    }
                    if n != 0 {
                        sleep(HEADER_WAIT * 2).await;
                    }
                    Some((
                        Ok::<_, std::convert::Infallible>(
                            Event::default().data(format!("synthetic-{n}")),
                        ),
                        n + 1,
                    ))
                }))
            }),
        )
        .route(
            "/ws",
            get(|upgrade: WebSocketUpgrade| async move {
                upgrade.on_upgrade(|mut socket| async move {
                    while let Some(Ok(message)) = socket.recv().await {
                        if socket.send(message).await.is_err() {
                            break;
                        }
                    }
                })
            }),
        )
        .route(
            "/rate-limit",
            get(|| async { StatusCode::TOO_MANY_REQUESTS }),
        )
}

async fn response(mut stream: Connection, request: &[u8]) -> Vec<u8> {
    stream.write_all(request).await.unwrap();
    let mut bytes = Vec::new();
    timeout(TEST_WAIT, stream.read_to_end(&mut bytes))
        .await
        .unwrap()
        .unwrap();
    assert!(bytes.starts_with(b"HTTP/1.1 200"));
    bytes
}

#[tokio::test]
async fn header_deadlines_do_not_limit_uploads_handlers_or_sse() {
    for tls in [false, true] {
        let server = RunningServer::start(tls, app()).await;
        let slow = response(
            server.connect(b"http/1.1").await,
            b"GET /slow HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n",
        );
        let sse = response(
            server.connect(b"http/1.1").await,
            b"GET /sse HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n",
        );
        let upload = async {
            let mut stream = server.connect(b"http/1.1").await;
            stream.write_all(b"POST /upload HTTP/1.1\r\nHost: localhost\r\nContent-Length: 5\r\nConnection: close\r\n\r\nh").await.unwrap();
            sleep(HEADER_WAIT * 2).await;
            response(stream, b"ello").await
        };
        let (slow, sse, upload) = tokio::join!(slow, sse, upload);
        assert!(slow.ends_with(b"\r\n\r\ncompleted"));
        let sse = String::from_utf8(sse).unwrap();
        assert!(sse.contains("data: synthetic-0\n\n") && sse.contains("data: synthetic-1\n\n"));
        assert!(upload.ends_with(b"\r\n\r\nhello"));
        server.stop().await;
    }
}

#[tokio::test]
async fn upgraded_websockets_survive_the_http_header_deadline() {
    for tls in [false, true] {
        let server = RunningServer::start(tls, app()).await;
        let (mut socket, _) =
            tokio_tungstenite::client_async("ws://localhost/ws", server.connect(b"http/1.1").await)
                .await
                .unwrap();
        sleep(HEADER_WAIT * 2).await;
        let message = tokio_tungstenite::tungstenite::Message::Text("synthetic-after-idle".into());
        socket.send(message.clone()).await.unwrap();
        assert_eq!(
            timeout(TEST_WAIT, socket.next())
                .await
                .unwrap()
                .unwrap()
                .unwrap(),
            message
        );
        socket.close(None).await.unwrap();
        server.stop().await;
    }
}

async fn h2_body(response: h2::client::ResponseFuture) -> (StatusCode, Vec<u8>) {
    let response = timeout(TEST_WAIT, response).await.unwrap().unwrap();
    let status = response.status();
    let mut body = response.into_body();
    let mut bytes = Vec::new();
    while let Some(chunk) = timeout(TEST_WAIT, body.data()).await.unwrap() {
        let chunk = chunk.unwrap();
        body.flow_control().release_capacity(chunk.len()).unwrap();
        bytes.extend_from_slice(&chunk);
    }
    (status, bytes)
}

#[tokio::test]
async fn http2_idle_and_multiplexed_streams_keep_their_existing_policy() {
    for tls in [false, true] {
        let server = RunningServer::start(tls, app()).await;
        let (mut client, connection) = h2::client::handshake(server.connect(b"h2").await)
            .await
            .unwrap();
        let driver = tokio::spawn(async move {
            let _ = connection.await;
        });
        let request = |path: &str| {
            Request::builder()
                .uri(format!("http://localhost{path}"))
                .body(())
                .unwrap()
        };
        let (slow, _) = client.send_request(request("/slow"), true).unwrap();
        let (limited, _) = client.send_request(request("/rate-limit"), true).unwrap();
        let (fast, _) = client.send_request(request("/health"), true).unwrap();
        assert_eq!(h2_body(limited).await.0, StatusCode::TOO_MANY_REQUESTS);
        assert_eq!(h2_body(fast).await, (StatusCode::OK, b"healthy".to_vec()));
        assert_eq!(client.is_extended_connect_protocol_enabled(), !tls);
        assert_eq!(h2_body(slow).await, (StatusCode::OK, b"completed".to_vec()));
        sleep(HEADER_WAIT * 2).await;
        let (again, _) = client.send_request(request("/health"), true).unwrap();
        assert_eq!(h2_body(again).await.0, StatusCode::OK);
        drop(client);
        server.stop().await;
        driver.abort();
    }
}

#[tokio::test]
async fn graceful_shutdown_drains_an_active_request() {
    for tls in [false, true] {
        let server = RunningServer::start(tls, app()).await;
        let mut stream = server.connect(b"http/1.1").await;
        stream
            .write_all(b"GET /sse HTTP/1.1\r\nHost: localhost\r\n\r\n")
            .await
            .unwrap();
        read_until(&mut stream, b"synthetic-0\n\n").await;
        server.shutdown.cancel();
        let mut remaining = Vec::new();
        timeout(TEST_WAIT, stream.read_to_end(&mut remaining))
            .await
            .unwrap()
            .unwrap();
        assert!(
            String::from_utf8(remaining)
                .unwrap()
                .contains("synthetic-1\n\n")
        );
        server.stop().await;
    }
}

#[tokio::test]
async fn native_shutdown_deadline_closes_a_stalled_response() {
    let app = app().route(
        "/stalled",
        get(|| async {
            Sse::new(
                futures_util::stream::once(async {
                    Ok::<_, std::convert::Infallible>(Event::default().data("started"))
                })
                .chain(futures_util::stream::pending()),
            )
        }),
    );
    let server = RunningServer::start(true, app).await;
    let mut stream = server.connect(b"http/1.1").await;
    stream
        .write_all(b"GET /stalled HTTP/1.1\r\nHost: localhost\r\n\r\n")
        .await
        .unwrap();
    read_until(&mut stream, b"started\n\n").await;
    let started = Instant::now();
    server.shutdown.cancel();
    closed(&mut stream).await;
    assert!(started.elapsed() >= DRAIN_WAIT);
    server.stop().await;
}
