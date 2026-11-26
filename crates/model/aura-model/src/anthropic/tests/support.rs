//! Fixtures shared by the Anthropic provider unit tests: model names,
//! API block builders, a local one-shot HTTP server and the
//! `aura::console` transcript capture.

use crate::anthropic::api_types::{ApiContent, ApiMessage};
use crate::anthropic::AnthropicConfig;
use crate::types::ModelRequestBuilder;
use crate::{Message, ModelRequest};
use std::net::SocketAddr;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::task::JoinHandle;

pub(super) const TEST_DEFAULT_MODEL: &str = "claude-opus-4-6";
pub(super) const TEST_FALLBACK_MODEL: &str = "claude-sonnet-4-6";

// ============================================================================
// API block builders
// ============================================================================

pub(super) fn tool_result(id: &str, body: &str, is_error: bool) -> ApiContent {
    ApiContent::ToolResult {
        tool_use_id: id.to_string(),
        content: body.to_string(),
        is_error: Some(is_error),
        cache_control: None,
    }
}

pub(super) fn text_block(text: &str) -> ApiContent {
    ApiContent::Text {
        text: text.to_string(),
        cache_control: None,
    }
}

pub(super) fn count_tool_results_with_id<'a>(
    api_messages: &'a [ApiMessage],
    id: &str,
) -> Vec<(&'a str, Option<bool>)> {
    api_messages
        .iter()
        .flat_map(|m| &m.content)
        .filter_map(|b| match b {
            ApiContent::ToolResult {
                tool_use_id,
                content,
                is_error,
                ..
            } if tool_use_id == id => Some((content.as_str(), *is_error)),
            _ => None,
        })
        .collect()
}

// ============================================================================
// Local proxy server
// ============================================================================

/// Proxy-mode config for `default_model` pointed at a local test server:
/// no retries, a 5s timeout and prompt caching on.
pub(super) fn proxy_config(default_model: &str, addr: SocketAddr) -> AnthropicConfig {
    AnthropicConfig {
        default_model: default_model.to_string(),
        timeout_ms: 5000,
        max_retries: 0,
        backoff_initial_ms: 250,
        backoff_cap_ms: 30_000,
        min_request_interval_ms: 0,
        base_url: format!("http://127.0.0.1:{}", addr.port()),
        fallback_model: None,
        prompt_caching_enabled: true,
        emergency_body_cap_bytes: 0,
        cloudflare_max_retries: 3,
    }
}

/// A request for `model` carrying one user message and a proxy JWT.
pub(super) fn proxy_request(model: &str) -> ModelRequestBuilder {
    ModelRequest::builder(model, "system")
        .message(Message::user("test"))
        .auth_token(Some("test-jwt-token".to_string()))
}

/// A raw HTTP/1.1 response; `headers` are CRLF-separated and
/// `Content-Length` is appended for `body`.
pub(super) fn http_response(status: &str, headers: &str, body: &str) -> String {
    format!(
        "HTTP/1.1 {status}\r\n{headers}\r\nContent-Length: {}\r\n\r\n{body}",
        body.len()
    )
}

/// A `200 OK` response with a JSON `body`.
pub(super) fn json_ok(body: &str) -> String {
    http_response("200 OK", "Content-Type: application/json", body)
}

/// Read one chunk (up to `capacity` bytes) of the inbound request as text.
pub(super) async fn read_request(socket: &mut TcpStream, capacity: usize) -> String {
    let mut buf = vec![0u8; capacity];
    let n = socket.read(&mut buf).await.unwrap();
    String::from_utf8_lossy(&buf[..n]).to_string()
}

/// Accept one connection on a fresh local port, answer it with
/// `response` and hand back the captured request text.
pub(super) async fn serve_once(
    response: String,
    capacity: usize,
) -> (SocketAddr, JoinHandle<String>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let server = tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.unwrap();
        let request_text = read_request(&mut socket, capacity).await;
        socket.write_all(response.as_bytes()).await.unwrap();
        request_text
    });
    (addr, server)
}

/// [`serve_once`] with a `200 OK` JSON `body` and an 8 KiB read.
pub(super) async fn serve_json_once(body: &str) -> (SocketAddr, JoinHandle<String>) {
    serve_once(json_ok(body), 8192).await
}

/// [`serve_once`] with a Cloudflare WAF `403` HTML page.
pub(super) async fn serve_cloudflare_block_once(body: &str) -> (SocketAddr, JoinHandle<String>) {
    let headers = "Content-Type: text/html\r\ncf-ray: ray-test";
    serve_once(http_response("403 Forbidden", headers, body), 8192).await
}

// ============================================================================
// `aura::console` capture
// ============================================================================

/// Process-wide buffer that the global `aura::console` subscriber
/// writes into. Tests acquire [`CONSOLE_CAPTURE_LOCK`] to take
/// exclusive ownership, clear, run their scenario, snapshot, then
/// release. Writes from other parallel tests are filtered out via
/// [`CAPTURE_THREAD_ID`] so they don't pollute the snapshot.
static CONSOLE_CAPTURE_BUF: std::sync::Mutex<Vec<u8>> = std::sync::Mutex::new(Vec::new());

/// Process-wide lock for tests that capture the `aura::console`
/// transcript. Serializes capture so concurrent `#[test]`s don't
/// interleave their event traffic into the shared buffer.
static CONSOLE_CAPTURE_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

static CONSOLE_CAPTURE_INIT: std::sync::Once = std::sync::Once::new();

/// Thread ID of the test currently holding [`CONSOLE_CAPTURE_LOCK`].
/// `0` means "no test is capturing" — the writer drops the event in
/// that state, so we don't accumulate events from unrelated tests.
static CAPTURE_THREAD_ID: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

fn current_thread_id_u64() -> u64 {
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    std::thread::current().id().hash(&mut hasher);
    hasher.finish()
}

struct CaptureWriter;
impl std::io::Write for CaptureWriter {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        let target = CAPTURE_THREAD_ID.load(std::sync::atomic::Ordering::Acquire);
        if target != 0 && target == current_thread_id_u64() {
            CONSOLE_CAPTURE_BUF.lock().unwrap().extend_from_slice(buf);
        }
        Ok(buf.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

/// Lock-guarded handle on the shared capture buffer. Dropping it
/// releases the lock and unforces ANSI colors. Constructed via
/// [`install_console_capture`].
pub(super) struct ConsoleCapture {
    _lock: std::sync::MutexGuard<'static, ()>,
}

impl ConsoleCapture {
    pub(super) fn snapshot(&self) -> String {
        String::from_utf8_lossy(&CONSOLE_CAPTURE_BUF.lock().unwrap()).to_string()
    }
}

impl Drop for ConsoleCapture {
    fn drop(&mut self) {
        CAPTURE_THREAD_ID.store(0, std::sync::atomic::Ordering::Release);
        colored::control::unset_override();
    }
}

/// Install — exactly once per process — a global tracing subscriber
/// that funnels every event into [`CONSOLE_CAPTURE_BUF`]. Acquire the
/// process-wide capture lock, clear the buffer, force `colored` off,
/// and return a guard. Subsequent capturing tests will block on the
/// lock until this guard drops.
///
/// ## Why a single global subscriber
///
/// Per-test thread-local subscribers (`set_default` / `with_default`)
/// don't survive `tokio` task hops or `tracing`'s callsite-interest
/// cache when other tests run in parallel — events from
/// `info!(target: "aura::console", …)` race against other subscribers
/// installed on other threads, and the cached `Interest` decides
/// whether the event reaches us or them. Owning the dispatcher
/// process-wide and serializing capture sidesteps the race.
pub(super) fn install_console_capture() -> ConsoleCapture {
    use tracing_subscriber::fmt::layer;
    use tracing_subscriber::layer::SubscriberExt;
    use tracing_subscriber::util::SubscriberInitExt;

    CONSOLE_CAPTURE_INIT.call_once(|| {
        let subscriber = tracing_subscriber::registry().with(
            layer()
                .with_ansi(false)
                .with_writer(|| CaptureWriter)
                .with_target(true),
        );
        let _ = subscriber.try_init();
    });

    let lock = CONSOLE_CAPTURE_LOCK
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    CONSOLE_CAPTURE_BUF.lock().unwrap().clear();
    CAPTURE_THREAD_ID.store(
        current_thread_id_u64(),
        std::sync::atomic::Ordering::Release,
    );
    colored::control::set_override(false);
    ConsoleCapture { _lock: lock }
}
