use super::support::*;
use crate::gateway::*;
use axum::body::Body;
use axum::http::Request;
use axum::http::StatusCode;
use tower::util::ServiceExt;

/// Shared buffer the global tracing subscriber writes events into,
/// gated by [`CAPTURE_THREAD_ID`]: writes only land in the buffer when
/// the emitting thread matches the capturing test's thread, so events
/// from other tests running in parallel can't pollute the snapshot.
static CONSOLE_CAPTURE_BUF: std::sync::Mutex<Vec<u8>> = std::sync::Mutex::new(Vec::new());
static CONSOLE_CAPTURE_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
static CONSOLE_CAPTURE_INIT: std::sync::Once = std::sync::Once::new();
/// Thread ID of the test currently holding [`CONSOLE_CAPTURE_LOCK`].
/// `0` means "no test is capturing right now" — the writer drops the
/// event in that state, so we don't accidentally accumulate events
/// from unrelated parts of the test suite.
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

/// Lock-guarded handle on the shared capture buffer. Constructed via
/// [`ConsoleCapture::install`]; drop to release the lock.
struct ConsoleCapture {
    _lock: std::sync::MutexGuard<'static, ()>,
}

impl ConsoleCapture {
    /// Install — exactly once per process — a global tracing subscriber
    /// that writes every event into [`CONSOLE_CAPTURE_BUF`], acquire
    /// the process-wide capture lock, and reset the buffer. Per-test
    /// thread-local subscribers don't survive `tokio` task hops or
    /// `tracing`'s callsite-interest cache when other tests run in
    /// parallel — owning the dispatcher process-wide and serializing
    /// capture sidesteps that race.
    fn install() -> Self {
        use tracing_subscriber::{fmt::layer, layer::SubscriberExt, util::SubscriberInitExt};

        CONSOLE_CAPTURE_INIT.call_once(|| {
            let subscriber = tracing_subscriber::registry().with(
                layer()
                    .with_ansi(false)
                    .with_writer(|| CaptureWriter)
                    .with_target(true),
            );
            let _ = subscriber.try_init();
        });

        let _lock = CONSOLE_CAPTURE_LOCK
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        CONSOLE_CAPTURE_BUF.lock().unwrap().clear();
        CAPTURE_THREAD_ID.store(
            current_thread_id_u64(),
            std::sync::atomic::Ordering::Release,
        );
        colored::control::set_override(false);
        Self { _lock }
    }

    fn snapshot(&self) -> String {
        String::from_utf8_lossy(&CONSOLE_CAPTURE_BUF.lock().unwrap()).to_string()
    }
}

impl Drop for ConsoleCapture {
    fn drop(&mut self) {
        CAPTURE_THREAD_ID.store(0, std::sync::atomic::Ordering::Release);
        colored::control::unset_override();
    }
}

/// Phase: the inbound failure observer middleware must surface
/// auth-middleware 401s through the visual `aura::console` transcript
/// the same way the outbound LLM call surfaces a 403 Cloudflare block.
/// Failure to do so means an operator scanning a single log file
/// won't see "request was rejected by the harness" — they'd have to
/// hunt through the structured `tower_http::trace` rows. Pair this
/// with `cloudflare_block_round_trip_emits_paired_request_and_failure_blocks`
/// in `aura-reasoner` to keep both halves of the contract honest.
#[tokio::test]
async fn unauthorized_inbound_request_emits_paired_inbound_blocks() {
    let capture = ConsoleCapture::install();
    let store = create_test_store();
    let state = test_router_state(store);
    let app = create_router(state);

    let mut req = Request::builder()
        .method("POST")
        .uri("/tx")
        .body(Body::empty())
        .unwrap();
    inject_fake_peer(&mut req, 0);

    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(
        resp.status(),
        StatusCode::UNAUTHORIZED,
        "missing bearer token should produce a 401 through the auth middleware"
    );

    let captured = capture.snapshot();
    assert!(
        captured.contains("→ POST /tx"),
        "expected paired inbound request block, got transcript:\n{captured}"
    );
    assert!(
        captured.contains("← 401 unauthorized"),
        "expected paired inbound failure block with 401 header + reason label, got transcript:\n{captured}"
    );
}

/// Health probes must NOT show up in the visual transcript, even
/// when they 4xx — kubelet / load-balancer probes hit `/health` on
/// a fixed cadence and would otherwise drown out real rejections.
/// Pair-tests `unauthorized_inbound_request_emits_paired_inbound_blocks`
/// (`/tx` → emits) against `/health` (→ silent) to pin the skip list.
#[tokio::test]
async fn inbound_failure_observer_skips_health_probes() {
    let capture = ConsoleCapture::install();
    let store = create_test_store();
    let state = test_router_state(store);
    let app = create_router(state);

    // `/health` is a public route, so a GET succeeds — but the
    // observer must stay quiet either way. Using GET keeps this test
    // resilient to public-route changes; the assertion is "no inbound
    // block was rendered for this path".
    let req = Request::builder()
        .method("GET")
        .uri("/health")
        .body(Body::empty())
        .unwrap();
    let _ = app.oneshot(req).await.unwrap();

    let captured = capture.snapshot();
    // The shared global subscriber captures events from every test
    // running in parallel — we only care that OUR observer didn't emit
    // its specific paired blocks for the health probe. Other concurrent
    // tests' debug noise is fine.
    assert!(
        !captured.contains("→ GET /health"),
        "/health probes must be excluded from the inbound observer's transcript, got:\n{captured}"
    );
    assert!(
        !captured.contains("← 404") && !captured.contains("← 405"),
        "inbound failure observer must not emit any failure block for /health, got:\n{captured}"
    );
}
