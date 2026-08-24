//! Authenticated Preview tunnel into loopback development servers.
//!
//! The hosted AURA browser cannot reach a dev server bound inside this
//! runtime through its own `localhost`. This endpoint exposes a deliberately
//! narrow binary WebSocket-to-TCP bridge so AURA can carry browser traffic to
//! a known development port without exposing the port publicly or permitting
//! arbitrary internal-network access.

use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::{Path, State};
use axum::response::IntoResponse;
use futures_util::{SinkExt, StreamExt};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;

use super::run_ws;
use crate::gateway::state::RouterState;

const CONNECT_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(3);
const MAX_TUNNEL_MESSAGE_BYTES: usize = 64 * 1024;

/// Explicit allowlist of common local development-server ports. The tunnel
/// always connects to `127.0.0.1`; callers cannot supply a host.
pub(crate) fn is_preview_port_allowed(port: u16) -> bool {
    matches!(
        port,
        3000 | 3001
            | 3002
            | 3003
            | 3030
            | 4000
            | 4200
            | 4321
            | 5000
            | 5173
            | 5174
            | 5500
            | 5501
            | 5555
            | 6006
            | 7000
            | 7070
            | 8000
            | 8001
            | 8080
            | 8081
            | 8088
            | 8888
            | 9000
            | 9001
            | 9090
    )
}

/// `GET /ws/preview/tcp/:port`
///
/// Authentication is applied by the gateway's protected-router middleware.
/// The swarm gateway separately verifies agent ownership before forwarding
/// here. Only binary frames carry TCP bytes; text frames are rejected.
pub(crate) async fn preview_tcp_ws(
    ws: WebSocketUpgrade,
    State(state): State<RouterState>,
    Path(port): Path<u16>,
) -> axum::response::Response {
    if !is_preview_port_allowed(port) {
        return (
            axum::http::StatusCode::BAD_REQUEST,
            "port is not allowed for Preview",
        )
            .into_response();
    }

    let Some(permit) = run_ws::try_acquire_ws_slot(&state.ws_slots) else {
        return axum::http::StatusCode::SERVICE_UNAVAILABLE.into_response();
    };

    let tcp = match tokio::time::timeout(CONNECT_TIMEOUT, connect_loopback(port)).await {
        Ok(Ok(stream)) => stream,
        Ok(Err(_)) => {
            return (
                axum::http::StatusCode::BAD_GATEWAY,
                "Preview development server is not accepting connections",
            )
                .into_response();
        }
        Err(_) => return axum::http::StatusCode::GATEWAY_TIMEOUT.into_response(),
    };

    ws.max_message_size(MAX_TUNNEL_MESSAGE_BYTES)
        .max_frame_size(MAX_TUNNEL_MESSAGE_BYTES)
        .on_upgrade(move |socket| async move {
            bridge(socket, tcp).await;
            drop(permit);
        })
        .into_response()
}

async fn connect_loopback(port: u16) -> std::io::Result<TcpStream> {
    match TcpStream::connect((std::net::Ipv4Addr::LOCALHOST, port)).await {
        Ok(stream) => Ok(stream),
        Err(ipv4_error) => TcpStream::connect((std::net::Ipv6Addr::LOCALHOST, port))
            .await
            .map_err(|_| ipv4_error),
    }
}

async fn bridge(socket: WebSocket, tcp: TcpStream) {
    let (mut ws_write, mut ws_read) = socket.split();
    let (mut tcp_read, mut tcp_write) = tcp.into_split();

    let websocket_to_tcp = async {
        while let Some(message) = ws_read.next().await {
            match message {
                Ok(Message::Binary(bytes)) => {
                    if tcp_write.write_all(&bytes).await.is_err() {
                        break;
                    }
                }
                Ok(Message::Close(_)) | Err(_) => break,
                Ok(Message::Text(_)) => break,
                Ok(Message::Ping(_)) | Ok(Message::Pong(_)) => {}
            }
        }
        let _ = tcp_write.shutdown().await;
    };

    let tcp_to_websocket = async {
        let mut buffer = vec![0_u8; 16 * 1024];
        loop {
            match tcp_read.read(&mut buffer).await {
                Ok(0) | Err(_) => break,
                Ok(read) => {
                    if ws_write
                        .send(Message::Binary(buffer[..read].to_vec()))
                        .await
                        .is_err()
                    {
                        break;
                    }
                }
            }
        }
        let _ = ws_write.send(Message::Close(None)).await;
    };

    tokio::select! {
        _ = websocket_to_tcp => {}
        _ = tcp_to_websocket => {}
    }
}

#[cfg(test)]
mod tests {
    use super::is_preview_port_allowed;

    #[test]
    fn allows_known_dev_ports() {
        for port in [3000, 4200, 5173, 6006, 8000, 8080, 9090] {
            assert!(is_preview_port_allowed(port), "port {port}");
        }
    }

    #[test]
    fn rejects_non_dev_and_privileged_ports() {
        for port in [0, 22, 80, 443, 2375, 5432, 6379, 65535] {
            assert!(!is_preview_port_allowed(port), "port {port}");
        }
    }
}
