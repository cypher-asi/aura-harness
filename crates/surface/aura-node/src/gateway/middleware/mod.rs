//! `create_router` + the middleware-stack assembly for the gateway.
//!
//! Renamed from `router/build.rs` in Phase C / Commit 4 so the
//! dispatch root [`super`] only owns module declarations and the
//! re-exports of [`super::RouterState`] / [`create_router`]. The
//! per-endpoint handler bundles live under
//! [`super::handlers`] and are pulled in here purely as imports —
//! no handler logic lives in this module.
//!
//! Co-located with the route-mounting layer:
//!
//! - [`create_router`] — splits the public sub-router (just
//!   `/health`) from the protected sub-router (everything else)
//!   so the auth middleware applies uniformly. The per-area
//!   sub-routers live in [`routes`].
//! - Middleware helpers in [`layers`]: `build_cors_layer`,
//!   `is_loopback_origin`, `build_global_governor`,
//!   `build_strict_governor`, `ensure_connect_info`,
//!   `inbound_failure_observer_mw`.
//! - The terminal-upgrade handler `terminal_ws_handler` (forwards
//!   to [`super::terminal_ws::handle_terminal_ws`]) and the `/health`
//!   handler, in [`handlers`].

mod handlers;
mod layers;
mod routes;

use std::time::Duration;

use axum::{extract::DefaultBodyLimit, http::StatusCode, Router};
use tower::limit::GlobalConcurrencyLimitLayer;
use tower_governor::GovernorLayer;
use tower_http::{
    timeout::TimeoutLayer,
    trace::{DefaultMakeSpan, DefaultOnRequest, DefaultOnResponse, TraceLayer},
};
use tracing::Level;

use super::auth_mw;
use super::RouterState;
use layers::{
    build_cors_layer, build_global_governor, ensure_connect_info, inbound_failure_observer_mw,
    MAX_IN_FLIGHT_REQUESTS,
};

/// Create the router.
///
/// The router is split into two halves:
///
/// - A **public** sub-router that currently only exposes `GET /health`
///   for liveness probes.
/// - A **protected** sub-router that layers every other route behind the
///   [`auth_mw::require_bearer_mw`] middleware via `.route_layer(...)` so
///   unauthenticated callers are rejected with `401` before any handler
///   logic runs. Using `route_layer` (not `layer`) keeps the middleware
///   scoped to the matched routes and lets `.fallback` still apply
///   uniformly across both halves. (Security audit — phase 1.)
///
/// The auth layer is only attached when [`crate::config::NodeConfig::require_auth`]
/// is `true` (driven by `AURA_NODE_REQUIRE_AUTH`). When auth is disabled
/// the protected sub-router is still structurally separate — matching
/// the public / protected split for ordering and body-limit purposes —
/// but every request is allowed through without a token check.
pub fn create_router(state: RouterState) -> Router {
    let protected = protected_routes(&state);
    let router = Router::new()
        .merge(routes::public_routes())
        .merge(protected)
        .with_state(state);
    apply_global_layers(router)
}

/// Every route except `/health`, behind the bearer-auth middleware when
/// auth is required.
fn protected_routes(state: &RouterState) -> Router<RouterState> {
    let protected = Router::new()
        .merge(routes::file_routes())
        .merge(routes::agent_routes())
        .merge(routes::stream_and_run_routes())
        .merge(routes::memory_routes())
        .merge(routes::skill_routes())
        .merge(routes::secret_routes())
        .merge(routes::process_routes())
        .merge(routes::strict_routes())
        .merge(routes::cloud_routes());

    // A cloud node always checks tokens: the hub's claim decides them.
    if state.config.require_auth || state.config.cloud_node {
        protected.route_layer(axum::middleware::from_fn_with_state(
            state.clone(),
            auth_mw::require_bearer_mw,
        ))
    } else {
        protected
    }
}

/// Wrap the stateful router in the router-wide observability, security and
/// limit layers.
fn apply_global_layers(router: Router) -> Router {
    router
        // Outermost observability layer: emit a paired `→ <method>
        // <path>` / `← <status> <reason>` block under the `aura::console`
        // target whenever an inbound request is rejected (non-2xx).
        // Sits outside `TraceLayer` so it observes the final response
        // status produced by every inner layer (auth, governor, body
        // limit, timeout, handler validation). Health probes are
        // suppressed inside the middleware to avoid kubelet-style noise.
        .layer(axum::middleware::from_fn(inbound_failure_observer_mw))
        // Security + observability layers (Wave 5 / T1 + phase 9).
        //
        // Order matters: `.layer(X)` wraps the existing stack, so the
        // *last* `.layer()` call runs first on an incoming request.
        // The stack from outermost (first seen) to innermost is:
        //   TraceLayer -> TimeoutLayer -> CorsLayer ->
        //   DefaultBodyLimit -> ConnectInfo-fallback ->
        //   GlobalConcurrencyLimitLayer -> GovernorLayer (global) ->
        //   (router + per-route strict governor + per-route body limits).
        //
        // Per-route body-limit layers on specific endpoints (e.g.
        // `/health` at 1 KiB, the GET query-param handlers at 16 KiB,
        // the small-JSON POSTs at 4 KiB) override the 1 MiB default
        // that applies to everything else. This keeps the 1 MiB
        // ceiling as a safety net for the few legitimately-large
        // endpoints (`/tx`, `/v1/run`) while throwing 413
        // early for everything that has no business seeing a megabyte
        // of body.
        //
        // `GlobalConcurrencyLimitLayer::new(MAX_IN_FLIGHT_REQUESTS)`
        // uses a shared `Arc<Semaphore>` — cloning the layer reuses
        // the same semaphore, which is what we need when axum clones
        // the router per connection. A plain `ConcurrencyLimitLayer`
        // would allocate a fresh semaphore per clone and defeat the
        // cap entirely.
        //
        // The `ensure_connect_info` fallback inserts
        // `ConnectInfo<SocketAddr>` into request extensions when it's
        // absent. Production serves with
        // `into_make_service_with_connect_info::<SocketAddr>()` so the
        // real peer is already there; this layer is a no-op in that
        // path. In `oneshot` tests we don't run through a listener,
        // so the fallback keeps the governor's `PeerIpKeyExtractor`
        // from rejecting requests with `UnableToExtractKey`.
        .layer(GovernorLayer {
            config: build_global_governor(),
        })
        .layer(GlobalConcurrencyLimitLayer::new(MAX_IN_FLIGHT_REQUESTS))
        .layer(axum::middleware::from_fn(ensure_connect_info))
        .layer(DefaultBodyLimit::max(1024 * 1024)) // 1 MiB
        .layer(build_cors_layer())
        .layer(TimeoutLayer::with_status_code(
            StatusCode::REQUEST_TIMEOUT,
            Duration::from_secs(30),
        ))
        // Phase 4 (security audit): explicit TraceLayer levels instead
        // of `TraceLayer::new_for_http()`. `tower_http`'s default span
        // already omits request headers — it only records method / uri
        // / version — so the `Authorization` header never enters our
        // log pipeline through this layer. The explicit level setters
        // make that intent auditable: if a future contributor swaps in
        // a custom `make_span_with`, they have to deliberately opt
        // into header logging (and redact Authorization) instead of
        // picking it up from the default.
        .layer(
            TraceLayer::new_for_http()
                .make_span_with(DefaultMakeSpan::new().level(Level::INFO))
                .on_request(DefaultOnRequest::new().level(Level::INFO))
                .on_response(DefaultOnResponse::new().level(Level::INFO)),
        )
}
