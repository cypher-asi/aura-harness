//! Middleware layers for the gateway: CORS, rate limiting, connect-info
//! fallback and the inbound-failure observer.

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Instant;

use axum::{
    body::Body,
    extract::ConnectInfo,
    http::{
        header::{self, HeaderName},
        HeaderValue, Method, Request,
    },
    middleware::Next,
    response::Response,
};
use tower_governor::governor::GovernorConfigBuilder;
use tower_http::cors::{AllowOrigin, CorsLayer};
use tracing::warn;

use crate::inbound_console::{self, reason_for_status, InboundFailureView, InboundRequestView};

/// Build the CORS layer from the `AURA_ALLOWED_ORIGINS` environment variable.
///
/// If set, parses a comma-separated list of exact origin values (e.g.
/// `https://aura.example,https://console.aura.example`). If unset, defaults
/// to a loopback predicate accepting `http://localhost:*` and
/// `http://127.0.0.1:*`, which is the conservative choice for local dev.
///
/// Non-loopback origins are denied by default — operators must opt in via
/// the env var.
pub(super) fn build_cors_layer() -> CorsLayer {
    let allow_origin = match std::env::var("AURA_ALLOWED_ORIGINS") {
        Ok(raw) if !raw.trim().is_empty() => {
            let values: Vec<HeaderValue> = raw
                .split(',')
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .filter_map(|origin| match HeaderValue::from_str(origin) {
                    Ok(v) => Some(v),
                    Err(e) => {
                        warn!(origin = %origin, error = %e, "ignoring invalid AURA_ALLOWED_ORIGINS entry");
                        None
                    }
                })
                .collect();
            if values.is_empty() {
                AllowOrigin::predicate(is_loopback_origin)
            } else {
                AllowOrigin::list(values)
            }
        }
        _ => AllowOrigin::predicate(is_loopback_origin),
    };

    CorsLayer::new()
        .allow_methods([Method::GET, Method::POST, Method::PUT, Method::DELETE])
        .allow_headers([
            header::AUTHORIZATION,
            header::CONTENT_TYPE,
            header::ACCEPT,
            HeaderName::from_static("x-requested-with"),
        ])
        .allow_origin(allow_origin)
}

/// Predicate that accepts only loopback origins (localhost / 127.0.0.1 / ::1)
/// on any port, using `http` or `https` scheme. Used as the default when
/// `AURA_ALLOWED_ORIGINS` is unset.
fn is_loopback_origin(origin: &HeaderValue, _req_parts: &axum::http::request::Parts) -> bool {
    let Ok(origin) = origin.to_str() else {
        return false;
    };
    let Some(rest) = origin
        .strip_prefix("http://")
        .or_else(|| origin.strip_prefix("https://"))
    else {
        return false;
    };
    // Strip the optional port segment so `localhost:3000` matches just as
    // well as bare `localhost`.
    let host = rest.split('/').next().unwrap_or(rest);
    let host = host.rsplit_once(':').map_or(host, |(h, _)| h);
    matches!(host, "localhost" | "127.0.0.1" | "[::1]" | "::1")
}

// === Rate limiting / concurrency helpers (phase 9) ===

/// Maximum number of in-flight HTTP requests the node will serve
/// concurrently before new requests wait on the
/// [`GlobalConcurrencyLimitLayer`] semaphore. Each pending request
/// occupies a tokio task plus its body buffer, so this caps worst-case
/// memory+task pressure on the runtime.
pub(crate) const MAX_IN_FLIGHT_REQUESTS: usize = 256;

/// Concrete type of the governor config we construct — spelled out so
/// helper builders can return something that the `GovernorLayer` field
/// accepts. `PeerIpKeyExtractor` is the default when the `axum`
/// feature is enabled, `NoOpMiddleware<QuantaInstant>` is the default
/// middleware `GovernorConfigBuilder` produces.
type GovernorCfg = tower_governor::governor::GovernorConfig<
    tower_governor::key_extractor::PeerIpKeyExtractor,
    governor::middleware::NoOpMiddleware<governor::clock::QuantaInstant>,
>;

/// Build the router-wide rate-limit config.
///
/// 30 requests/sec with a burst of 60, keyed on peer IP address.
/// Fresh per `create_router` call so different test routers don't
/// share a limiter — production only calls `create_router` once.
///
/// INVARIANT: both `per_millisecond` and `burst_size` are hard-coded
/// non-zero integer literals, so `GovernorConfigBuilder::finish()`
/// cannot return `None` here and the library-default fallback is never
/// taken.
pub(super) fn build_global_governor() -> Arc<GovernorCfg> {
    Arc::new(
        GovernorConfigBuilder::default()
            .per_millisecond(1000 / 30) // ≈30 req/sec sustained
            .burst_size(60)
            .finish()
            .unwrap_or_default(),
    )
}

/// Stricter rate limit for mutating endpoints (`/tx`, `/v1/run`,
/// `:id/pause`, `:id/stop`). 5/sec burst 10.
///
/// INVARIANT: same reasoning as [`build_global_governor`] — hard-coded
/// non-zero integer literals mean the fallback is never taken.
pub(super) fn build_strict_governor() -> Arc<GovernorCfg> {
    Arc::new(
        GovernorConfigBuilder::default()
            .per_millisecond(200) // 5 req/sec sustained
            .burst_size(10)
            .finish()
            .unwrap_or_default(),
    )
}

/// Inject a default `ConnectInfo<SocketAddr>` into request extensions
/// when one isn't already present.
///
/// Production uses `into_make_service_with_connect_info::<SocketAddr>()`
/// which inserts the real peer address before the request reaches this
/// layer, so this is a no-op in that code path. In `oneshot` tests
/// there is no listener, so without a fallback the governor's
/// `PeerIpKeyExtractor` would error out with `UnableToExtractKey`
/// (which tower_governor surfaces as `500 Internal Server Error`) on
/// every request. Injecting a loopback default means every oneshot
/// request is attributed to the same synthetic "client", which is
/// exactly what the rate-limit test wants.
pub(super) async fn ensure_connect_info(
    mut req: axum::extract::Request,
    next: axum::middleware::Next,
) -> axum::response::Response {
    if req.extensions().get::<ConnectInfo<SocketAddr>>().is_none() {
        req.extensions_mut()
            .insert(ConnectInfo(SocketAddr::from(([127, 0, 0, 1], 0))));
    }
    next.run(req).await
}

/// Path prefixes whose rejections are intentionally suppressed from the
/// `aura::console` visual transcript. Today this is just `/health` —
/// kubelet, load-balancer, and uptime probes hit it on a fixed cadence
/// and a non-2xx there (e.g. brief startup window) would otherwise
/// flood the log with paired blocks before the operator can see
/// anything else.
const INBOUND_OBSERVE_SKIP_PATHS: &[&str] = &["/health"];

/// Outermost axum middleware that surfaces blocked / rejected inbound
/// requests under the `aura::console` target.
///
/// Captures `method` / `path` / peer / `Content-Length` / start-instant
/// before forwarding the request, awaits the response, and — when the
/// final status is `>= 400` and the path is not in
/// [`INBOUND_OBSERVE_SKIP_PATHS`] — emits a paired
/// [`inbound_console::inbound_request_summary_block`] +
/// [`inbound_console::inbound_failure_block`] so the visual transcript
/// shows the rejection symmetric to the existing outbound `→ POST` /
/// `← <status>` LLM-call blocks.
///
/// Successful (2xx/3xx) responses stay quiet to keep transcript noise
/// low; the existing `TraceLayer` continues to record them through
/// the default formatter for grep-friendly per-request audit.
pub(super) async fn inbound_failure_observer_mw(req: Request<Body>, next: Next) -> Response {
    let started_at = Instant::now();
    let method = req.method().clone();
    let path = req.uri().path().to_string();
    let peer = req
        .extensions()
        .get::<ConnectInfo<SocketAddr>>()
        .map(|ConnectInfo(addr)| *addr);
    let body_bytes = req
        .headers()
        .get(header::CONTENT_LENGTH)
        .and_then(|v| v.to_str().ok())
        .and_then(|s| s.parse::<usize>().ok());

    let response = next.run(req).await;
    let status = response.status();
    if !status.is_client_error() && !status.is_server_error() {
        return response;
    }
    if INBOUND_OBSERVE_SKIP_PATHS
        .iter()
        .any(|prefix| path == *prefix || path.starts_with(&format!("{prefix}/")))
    {
        return response;
    }

    #[allow(clippy::cast_possible_truncation)]
    let elapsed_ms = started_at.elapsed().as_millis() as u64;
    let status_code = status.as_u16();
    let status_text = status
        .canonical_reason()
        .map_or_else(|| status.to_string(), str::to_string);
    let reason = reason_for_status(status_code);

    inbound_console::inbound_request_summary_block(&InboundRequestView {
        method: method.as_str(),
        path: &path,
        peer,
        body_bytes,
    });
    inbound_console::inbound_failure_block(&InboundFailureView {
        status_code,
        status_text: &status_text,
        reason,
        elapsed_ms,
        peer,
        body_preview: None,
    });
    response
}
