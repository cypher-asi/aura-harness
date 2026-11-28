//! Loopback HTTP/1.1 server for the web tool tests: serves canned
//! responses by path and records the requests it saw.

use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::{Arc, Mutex};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

#[derive(Clone)]
pub(crate) struct Reply {
    pub status: u16,
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
    /// Hold the connection this long before answering.
    pub delay: Option<std::time::Duration>,
}

impl Reply {
    pub(crate) fn ok(content_type: &str, body: impl Into<Vec<u8>>) -> Self {
        Self {
            status: 200,
            headers: vec![("Content-Type".into(), content_type.into())],
            body: body.into(),
            delay: None,
        }
    }

    pub(crate) fn redirect(location: &str) -> Self {
        Self {
            status: 302,
            headers: vec![("Location".into(), location.into())],
            body: Vec::new(),
            delay: None,
        }
    }

    pub(crate) fn status(status: u16, body: &str) -> Self {
        Self {
            status,
            headers: vec![("Content-Type".into(), "text/plain".into())],
            body: body.as_bytes().to_vec(),
            delay: None,
        }
    }
}

#[derive(Debug, Clone, Default)]
pub(crate) struct Seen {
    pub method: String,
    pub path_and_query: String,
    pub headers: Vec<(String, String)>,
    pub body: String,
}

impl Seen {
    pub(crate) fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(n, _)| n.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.as_str())
    }
}

pub(crate) struct TestServer {
    pub addr: SocketAddr,
    pub seen: Arc<Mutex<Vec<Seen>>>,
}

impl TestServer {
    pub(crate) fn url(&self, path: &str) -> String {
        format!("http://{}{path}", self.addr)
    }

    pub(crate) fn requests(&self) -> Vec<Seen> {
        self.seen.lock().unwrap().clone()
    }

    /// Serve `routes` (path without query -> reply); unknown paths get 404.
    pub(crate) async fn start(routes: Vec<(&str, Reply)>) -> Self {
        let routes: Arc<HashMap<String, Reply>> = Arc::new(
            routes
                .into_iter()
                .map(|(p, r)| (p.to_string(), r))
                .collect(),
        );
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let seen = Arc::new(Mutex::new(Vec::new()));
        let seen_task = Arc::clone(&seen);
        tokio::spawn(async move {
            while let Ok((mut sock, _)) = listener.accept().await {
                let routes = Arc::clone(&routes);
                let seen = Arc::clone(&seen_task);
                tokio::spawn(async move {
                    let request = read_request(&mut sock).await;
                    let path = request
                        .path_and_query
                        .split('?')
                        .next()
                        .unwrap_or_default()
                        .to_string();
                    let route = format!("{} {path}", request.method);
                    seen.lock().unwrap().push(request);
                    // `"METHOD /path"` routes win over plain `"/path"` ones.
                    let reply = routes
                        .get(&route)
                        .or_else(|| routes.get(&path))
                        .cloned()
                        .unwrap_or_else(|| Reply::status(404, "not found"));
                    if let Some(delay) = reply.delay {
                        tokio::time::sleep(delay).await;
                    }
                    let mut head = format!("HTTP/1.1 {} Status\r\n", reply.status);
                    for (name, value) in &reply.headers {
                        head.push_str(&format!("{name}: {value}\r\n"));
                    }
                    head.push_str(&format!(
                        "Content-Length: {}\r\nConnection: close\r\n\r\n",
                        reply.body.len()
                    ));
                    let _ = sock.write_all(head.as_bytes()).await;
                    let _ = sock.write_all(&reply.body).await;
                    let _ = sock.shutdown().await;
                });
            }
        });
        Self { addr, seen }
    }
}

async fn read_request(sock: &mut tokio::net::TcpStream) -> Seen {
    let mut buf = Vec::new();
    let mut chunk = [0u8; 4096];
    let header_end = loop {
        let Ok(n) = sock.read(&mut chunk).await else {
            return Seen::default();
        };
        if n == 0 {
            return Seen::default();
        }
        buf.extend_from_slice(&chunk[..n]);
        if let Some(pos) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
            break pos;
        }
    };
    let head = String::from_utf8_lossy(&buf[..header_end]).to_string();
    let mut lines = head.split("\r\n");
    let mut parts = lines.next().unwrap_or_default().split_whitespace();
    let method = parts.next().unwrap_or_default().to_string();
    let path_and_query = parts.next().unwrap_or_default().to_string();
    let headers: Vec<(String, String)> = lines
        .filter_map(|l| l.split_once(':'))
        .map(|(n, v)| (n.trim().to_string(), v.trim().to_string()))
        .collect();
    let length: usize = headers
        .iter()
        .find(|(n, _)| n.eq_ignore_ascii_case("content-length"))
        .and_then(|(_, v)| v.parse().ok())
        .unwrap_or(0);
    let mut body = buf[header_end + 4..].to_vec();
    while body.len() < length {
        let Ok(n) = sock.read(&mut chunk).await else {
            break;
        };
        if n == 0 {
            break;
        }
        body.extend_from_slice(&chunk[..n]);
    }
    Seen {
        method,
        path_and_query,
        headers,
        body: String::from_utf8_lossy(&body).to_string(),
    }
}
