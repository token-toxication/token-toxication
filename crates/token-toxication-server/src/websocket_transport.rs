use std::time::Duration;

use axum::http::{HeaderMap, StatusCode, header};
use tokio_tungstenite::{
    WebSocketStream,
    tungstenite::{
        handshake::derive_accept_key,
        protocol::{Role, WebSocketConfig},
    },
};

/// A separate policy client keeps upgrades from inheriting HTTP redirects or retries.
pub fn build_client(idle: Duration) -> Result<aioduct::TokioClient, aioduct::Error> {
    build_with_tls(idle, aioduct::tls::RustlsConnector::with_webpki_roots())
}

fn build_with_tls(
    idle: Duration,
    mut tls: aioduct::tls::RustlsConnector,
) -> Result<aioduct::TokioClient, aioduct::Error> {
    tls.config_mut().alpn_protocols = vec![b"http/1.1".to_vec()];
    aioduct::TokioClient::builder()
        .tls(tls)
        .user_agent("token-toxication/0.1")
        .redirect_policy(aioduct::RedirectPolicy::None)
        .retry(aioduct::RetryConfig::default().max_retries(0))
        .timeout(idle)
        .read_timeout(idle)
        .build()
}

#[derive(Debug, thiserror::Error)]
pub(crate) enum ConnectError {
    #[error("upstream rejected WebSocket upgrade with {status}")]
    Http {
        status: StatusCode,
        headers: HeaderMap,
    },
    #[error("invalid upstream WebSocket handshake")]
    Handshake,
    #[error("upstream WebSocket transport failed")]
    Transport,
}

pub(crate) async fn connect(
    client: &aioduct::TokioClient,
    endpoint: &str,
    headers: HeaderMap,
) -> Result<WebSocketStream<aioduct::upgrade::UpgradedSend>, ConnectError> {
    let request = client
        .get(endpoint)
        .map_err(|_| ConnectError::Transport)?
        .headers(headers)
        .upgrade()
        .retry(aioduct::RetryConfig::default().max_retries(0));
    let key = request
        .headers_ref()
        .get(header::SEC_WEBSOCKET_KEY)
        .ok_or(ConnectError::Handshake)?;
    let expected_accept = derive_accept_key(key.as_bytes());
    let response = request.send().await.map_err(|_| ConnectError::Transport)?;
    if response.status() != StatusCode::SWITCHING_PROTOCOLS {
        if response.status().is_success() {
            return Err(ConnectError::Handshake);
        }
        return Err(ConnectError::Http {
            status: response.status(),
            headers: response.headers().clone(),
        });
    }
    validate_handshake(response.headers(), &expected_accept)?;
    let io = response
        .upgrade()
        .await
        .map_err(|_| ConnectError::Transport)?;
    let config = WebSocketConfig::default()
        .max_message_size(Some(crate::RELAY_BODY_LIMIT_BYTES))
        .max_frame_size(Some(crate::RELAY_BODY_LIMIT_BYTES));
    Ok(WebSocketStream::from_raw_socket(io, Role::Client, Some(config)).await)
}

fn validate_handshake(headers: &HeaderMap, expected_accept: &str) -> Result<(), ConnectError> {
    let mut accepts = headers.get_all(header::SEC_WEBSOCKET_ACCEPT).iter();
    let accept_matches = accepts
        .next()
        .is_some_and(|value| value.as_bytes() == expected_accept.as_bytes())
        && accepts.next().is_none();
    let upgrades: Vec<_> = headers.get_all(header::UPGRADE).iter().collect();
    let upgrade_matches = upgrades.len() == 1
        && upgrades[0]
            .to_str()
            .is_ok_and(|value| value.eq_ignore_ascii_case("websocket"));
    let connection_matches = headers.get_all(header::CONNECTION).iter().any(|value| {
        value.to_str().is_ok_and(|value| {
            value
                .split(',')
                .any(|token| token.trim().eq_ignore_ascii_case("upgrade"))
        })
    });
    // No extensions or subprotocols are requested, so none may be negotiated.
    if !accept_matches
        || !upgrade_matches
        || !connection_matches
        || headers.contains_key(header::SEC_WEBSOCKET_PROTOCOL)
        || headers.contains_key(header::SEC_WEBSOCKET_EXTENSIONS)
    {
        return Err(ConnectError::Handshake);
    }
    Ok(())
}

#[cfg(test)]
mod tests;
