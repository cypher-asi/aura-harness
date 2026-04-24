//! HTTP and WebSocket router for the node API.

use crate::automaton_bridge::AutomatonBridge;
use crate::config::NodeConfig;
use crate::scheduler::Scheduler;
use crate::session::{handle_ws_connection, WsContext};
use crate::terminal;
use aura_core::{AgentId, Hash, Transaction, TransactionType};
use aura_reasoner::ModelProvider;
use aura_store::Store;
use aura_tools::automaton_tools::AutomatonController;
use aura_tools::domain_tools::DomainApi;
use aura_tools::{ToolCatalog, ToolConfig};
use axum::{
    extract::{ws::WebSocketUpgrade, DefaultBodyLimit, Path, Query, State},
    http::{
        header::{self, HeaderName},
        HeaderMap, HeaderValue, Method, StatusCode,
    },
    response::IntoResponse,
    routing::{get, post},
    Json, Router,
};
use bytes::Bytes;
use dashmap::DashMap;
use serde::{Deserialize, Serialize};
use std::net::SocketAddr;
use std::sync::{Arc, RwLock};
use std::time::Duration;
use tokio::sync::Semaphore;
use tower::limit::GlobalConcurrencyLimitLayer;
use tower_governor::{governor::GovernorConfigBuilder, GovernorLayer};
use tower_http::{
    cors::{AllowOrigin, CorsLayer},
    timeout::TimeoutLayer,
    trace::{DefaultMakeSpan, DefaultOnRequest, DefaultOnResponse, TraceLayer},
};
use tracing::{error, info, instrument, warn, Level};

mod auth;
mod automaton;
mod files;
mod memory;
mod skills;
mod tool_permissions;
mod tx;
mod ws;

use automaton::*;
use files::*;
use tool_permissions::*;
use tx::*;
use ws::*;

#[cfg(test)]
mod tests;

/// Shared state for the router.
///
/// Fields are `pub(crate)` — external callers (including the `test_support`
/// feature and harness binaries) must go through [`RouterState::new`]. This
/// keeps the wire-up in one place instead of scattering struct literals
/// across test fixtures. (Wave 3 — T2.3.)
pub struct RouterState {
    // TODO(phase2-followup): Invariant §10 wants this bound to
    // `Arc<dyn ReadStore>`. The router itself only needs `enqueue_tx`
    // / `get_head_seq` / `has_pending_tx` (all on `ReadStore`), but it
    // also hands the store to `WsContext`, which in turn hands it to
    // `Kernel::new` — and `Kernel::new` takes `Arc<dyn Store>`.
    // Resolving this requires either (a) teaching `Kernel::new` to
    // accept a `(ReadStore, WriteHook)` pair or (b) splitting this
    // field into a `ReadStore` for the HTTP surface and a separate
    // `Store` scoped to the session/kernel construction path. Punted
    // to a follow-up phase.
    pub(crate) store: Arc<dyn Store>,
    pub(crate) scheduler: Arc<Scheduler>,
    pub(crate) config: NodeConfig,
    /// Model provider for WebSocket sessions (type-erased).
    pub(crate) provider: Arc<dyn ModelProvider + Send + Sync>,
    /// Tool configuration for WebSocket sessions.
    pub(crate) tool_config: ToolConfig,
    /// Canonical tool catalog (shared across sessions).
    pub(crate) catalog: Arc<ToolCatalog>,
    /// Domain API for specs/tasks/project/orbit/network (None if no internal token).
    pub(crate) domain_api: Option<Arc<dyn DomainApi>>,
    /// Automaton controller for dev-loop lifecycle (None when domain API unavailable).
    pub(crate) automaton_controller: Option<Arc<dyn AutomatonController>>,
    /// Concrete bridge for event subscription (same object as automaton_controller).
    pub(crate) automaton_bridge: Option<Arc<AutomatonBridge>>,
    /// tx_id (hex) -> error message for scheduling failures after 202 acceptance.
    pub(crate) failed_txs: Arc<DashMap<String, String>>,
    /// Optional memory manager for CRUD API and session injection.
    pub(crate) memory_manager: Option<Arc<aura_memory::MemoryManager>>,
    /// Optional skill manager for skill CRUD API and prompt injection.
    pub(crate) skill_manager: Option<Arc<RwLock<aura_skills::SkillManager>>>,
    /// Router URL for generation proxying (from `AURA_ROUTER_URL`).
    pub(crate) router_url: Option<String>,
    /// Bounded pool of WebSocket connection slots.
    ///
    /// Every upgrade handler (`/ws/terminal`, `/stream`,
    /// `/stream/automaton/:id`) must call
    /// [`ws::try_acquire_ws_slot`] and attach the returned permit to the
    /// spawned socket task. When the semaphore is empty, the handler
    /// short-circuits with `503 Service Unavailable` instead of tying
    /// up another tokio task (the H5 "unbounded WS frames + slow-client
    /// task exhaustion" finding — phase 9 of the audit remediation).
    ///
    /// A strict per-IP cap would need to plumb the peer socket address
    /// through every upgrade handler; tower_governor can't rate-limit
    /// long-lived WS sessions because it only inspects the upgrade
    /// request, so we leave per-IP for a future iteration and bound
    /// the global count here.
    pub(crate) ws_slots: Arc<Semaphore>,
}

/// Input bundle for [`RouterState::new`].
///
/// Grouped into a single parameter struct so we don't have to thread 13
/// positional arguments through every test and binary. Optional fields
/// mirror the ones that default to `None` on the router state.
pub struct RouterStateConfig {
    /// Store handle for HTTP/WS endpoints. See the TODO on
    /// [`RouterState::store`] — the type is conceptually
    /// `Arc<dyn ReadStore>` (Invariant §10) but is still `Arc<dyn Store>`
    /// while the kernel constructor expects the combined trait.
    pub store: Arc<dyn Store>,
    pub scheduler: Arc<Scheduler>,
    pub config: NodeConfig,
    pub provider: Arc<dyn ModelProvider + Send + Sync>,
    pub tool_config: ToolConfig,
    pub catalog: Arc<ToolCatalog>,
    pub domain_api: Option<Arc<dyn DomainApi>>,
    pub automaton_controller: Option<Arc<dyn AutomatonController>>,
    pub automaton_bridge: Option<Arc<AutomatonBridge>>,
    pub memory_manager: Option<Arc<aura_memory::MemoryManager>>,
    pub skill_manager: Option<Arc<RwLock<aura_skills::SkillManager>>>,
    pub router_url: Option<String>,
}

impl RouterState {
    /// Build a router state from the given configuration.
    ///
    /// `failed_txs` is always initialized fresh — there is no legitimate
    /// reason to share that map across `RouterState` instances.
    #[must_use]
    pub fn new(cfg: RouterStateConfig) -> Self {
        Self {
            store: cfg.store,
            scheduler: cfg.scheduler,
            config: cfg.config,
            provider: cfg.provider,
            tool_config: cfg.tool_config,
            catalog: cfg.catalog,
            domain_api: cfg.domain_api,
            automaton_controller: cfg.automaton_controller,
            automaton_bridge: cfg.automaton_bridge,
            failed_txs: Arc::new(DashMap::new()),
            memory_manager: cfg.memory_manager,
            skill_manager: cfg.skill_manager,
            router_url: cfg.router_url,
            ws_slots: Arc::new(Semaphore::new(ws::MAX_WS_CONNS_PER_NODE)),
        }
    }
}

impl Clone for RouterState {
    fn clone(&self) -> Self {
        Self {
            store: self.store.clone(),
            scheduler: self.scheduler.clone(),
            config: self.config.clone(),
            provider: self.provider.clone(),
            tool_config: self.tool_config.clone(),
            catalog: self.catalog.clone(),
            domain_api: self.domain_api.clone(),
            automaton_controller: self.automaton_controller.clone(),
            automaton_bridge: self.automaton_bridge.clone(),
            failed_txs: self.failed_txs.clone(),
            memory_manager: self.memory_manager.clone(),
            skill_manager: self.skill_manager.clone(),
            router_url: self.router_url.clone(),
            ws_slots: self.ws_slots.clone(),
        }
    }
}

/// Create the router.
///
/// The router is split into two halves:
///
/// - A **public** sub-router that currently only exposes `GET /health`
///   for liveness probes.
/// - A **protected** sub-router that layers every other route behind the
///   [`auth::require_bearer_mw`] middleware via `.route_layer(...)` so
///   unauthenticated callers are rejected with `401` before any handler
///   logic runs. Using `route_layer` (not `layer`) keeps the middleware
///   scoped to the matched routes and lets `.fallback` still apply
///   uniformly across both halves. (Security audit — phase 1.)
///
/// The auth layer is only attached when [`NodeConfig::require_auth`] is
/// `true` (driven by `AURA_NODE_REQUIRE_AUTH`). When auth is disabled
/// the protected sub-router is still structurally separate — matching
/// the public / protected split for ordering and body-limit purposes —
/// but every request is allowed through without a token check.
pub fn create_router(state: RouterState) -> Router {
    // Per-route body limits — tighter ceilings for endpoints that have
    // no legitimate reason to accept a large body. Each one is a
    // *layer* so it overrides the 1 MiB global default applied at the
    // bottom of this function. Phase 9 of the security audit.
    let body_limit_1k = DefaultBodyLimit::max(1024);
    let body_limit_16k = DefaultBodyLimit::max(16 * 1024);
    let body_limit_4k = DefaultBodyLimit::max(4 * 1024);

    let public = Router::new().route("/health", get(health_handler).route_layer(body_limit_1k));

    // Mutating JSON endpoints get a stricter per-IP governor (5/sec,
    // burst 10) so a misbehaving client can't flood writes even if
    // they stay under the global 30/sec cap. See `build_strict_governor`.
    let strict_governor_layer = GovernorLayer {
        config: build_strict_governor(),
    };

    // Strict-rate-limit sub-router: `/tx`, `/automaton/start`, and the
    // `:id/pause` + `:id/stop` path params. Pause/stop use a 4 KiB
    // body limit for tiny JSON payloads; `/tx` and `/automaton/start`
    // keep the 1 MiB default because legitimate requests can be large.
    let strict_small_body = Router::new()
        .route(
            "/automaton/:automaton_id/pause",
            post(automaton_pause_handler),
        )
        .route(
            "/automaton/:automaton_id/stop",
            post(automaton_stop_handler),
        )
        .route_layer(body_limit_4k);

    let strict_default_body = Router::new()
        .route("/tx", post(submit_tx_handler))
        .route("/automaton/start", post(automaton_start_handler));

    let strict = strict_small_body
        .merge(strict_default_body)
        .route_layer(strict_governor_layer);

    let protected = Router::new()
        .route(
            "/api/files",
            get(list_files_handler).route_layer(body_limit_16k),
        )
        .route(
            "/api/read-file",
            get(read_file_handler).route_layer(body_limit_16k),
        )
        .route(
            "/workspace/resolve",
            get(resolve_workspace_handler).route_layer(body_limit_16k),
        )
        .route(
            "/tx/status/:agent_id/:tx_id",
            get(tx_status_handler).route_layer(body_limit_1k),
        )
        .route(
            "/agents/:agent_id/head",
            get(get_head_handler).route_layer(body_limit_1k),
        )
        .route(
            "/agents/:agent_id/record",
            get(scan_record_handler).route_layer(body_limit_16k),
        )
        .route(
            "/users/:user_id/tool-defaults",
            get(get_user_tool_defaults_handler).put(put_user_tool_defaults_handler),
        )
        .route(
            "/agents/:agent_id/tool-permissions",
            get(get_agent_tool_permissions_handler).put(put_agent_tool_permissions_handler),
        )
        .route("/agents/:agent_id/tools", get(get_agent_tools_handler))
        .route("/ws/terminal", get(terminal_ws_handler))
        .route("/stream", get(ws_upgrade_handler))
        .route("/stream/automaton/:automaton_id", get(automaton_ws_handler))
        .route(
            "/automaton/list",
            get(automaton_list_handler).route_layer(body_limit_1k),
        )
        .route(
            "/automaton/:automaton_id/status",
            get(automaton_status_handler).route_layer(body_limit_1k),
        )
        // Memory CRUD (canonical paths)
        .route(
            "/memory/:agent_id/facts",
            get(memory::list_facts).post(memory::create_fact),
        )
        .route(
            "/memory/:agent_id/facts/:id",
            get(memory::get_fact)
                .put(memory::update_fact)
                .delete(memory::delete_fact),
        )
        .route(
            "/memory/:agent_id/facts/by-key/:key",
            get(memory::get_fact_by_key),
        )
        .route(
            "/memory/:agent_id/events",
            get(memory::list_events).post(memory::create_event),
        )
        .route(
            "/memory/:agent_id/events/:id",
            axum::routing::delete(memory::delete_event),
        )
        .route(
            "/memory/:agent_id/events/bulk-delete",
            post(memory::bulk_delete_events),
        )
        .route(
            "/memory/:agent_id/procedures",
            get(memory::list_procedures).post(memory::create_procedure),
        )
        .route(
            "/memory/:agent_id/procedures/:id",
            get(memory::get_procedure)
                .put(memory::update_procedure)
                .delete(memory::delete_procedure),
        )
        .route("/memory/:agent_id/snapshot", get(memory::snapshot))
        .route("/memory/:agent_id/wipe", post(memory::wipe))
        .route("/memory/:agent_id/stats", get(memory::stats))
        .route("/memory/:agent_id/consolidate", post(memory::consolidate))
        // Memory aliases (aura-os proxy sends /api/agents/:id/memory/...)
        .route(
            "/api/agents/:agent_id/memory",
            get(memory::snapshot).delete(memory::wipe),
        )
        .route(
            "/api/agents/:agent_id/memory/facts",
            get(memory::list_facts).post(memory::create_fact),
        )
        .route(
            "/api/agents/:agent_id/memory/facts/:id",
            get(memory::get_fact)
                .put(memory::update_fact)
                .delete(memory::delete_fact),
        )
        .route(
            "/api/agents/:agent_id/memory/facts/by-key/:key",
            get(memory::get_fact_by_key),
        )
        .route(
            "/api/agents/:agent_id/memory/events",
            get(memory::list_events).post(memory::create_event),
        )
        .route(
            "/api/agents/:agent_id/memory/events/:id",
            axum::routing::delete(memory::delete_event),
        )
        .route(
            "/api/agents/:agent_id/memory/events/bulk-delete",
            post(memory::bulk_delete_events),
        )
        .route(
            "/api/agents/:agent_id/memory/procedures",
            get(memory::list_procedures).post(memory::create_procedure),
        )
        .route(
            "/api/agents/:agent_id/memory/procedures/:id",
            get(memory::get_procedure)
                .put(memory::update_procedure)
                .delete(memory::delete_procedure),
        )
        .route("/api/agents/:agent_id/memory/stats", get(memory::stats))
        .route(
            "/api/agents/:agent_id/memory/consolidate",
            post(memory::consolidate),
        )
        // Skills CRUD
        .route(
            "/api/skills",
            get(skills::list_skills).post(skills::create_skill),
        )
        .route("/api/skills/:name", get(skills::get_skill))
        .route("/api/skills/:name/activate", post(skills::activate_skill))
        // Per-agent skill installations
        .route(
            "/api/agents/:agent_id/skills",
            get(skills::list_agent_skills).post(skills::install_agent_skill),
        )
        .route(
            "/api/agents/:agent_id/skills/:name",
            axum::routing::delete(skills::uninstall_agent_skill),
        )
        // Legacy compatibility aliases for older harness callers.
        .route(
            "/api/harness/agents/:agent_id/skills",
            get(skills::list_agent_skills).post(skills::install_agent_skill),
        )
        .route(
            "/api/harness/agents/:agent_id/skills/:name",
            axum::routing::delete(skills::uninstall_agent_skill),
        )
        .merge(strict);

    let protected = if state.config.require_auth {
        protected.route_layer(axum::middleware::from_fn_with_state(
            state.clone(),
            auth::require_bearer_mw,
        ))
    } else {
        protected
    };

    Router::new()
        .merge(public)
        .merge(protected)
        .with_state(state)
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
        // endpoints (`/tx`, `/automaton/start`) while throwing 413
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

/// Build the CORS layer from the `AURA_ALLOWED_ORIGINS` environment variable.
///
/// If set, parses a comma-separated list of exact origin values (e.g.
/// `https://aura.example,https://console.aura.example`). If unset, defaults
/// to a loopback predicate accepting `http://localhost:*` and
/// `http://127.0.0.1:*`, which is the conservative choice for local dev.
///
/// Non-loopback origins are denied by default — operators must opt in via
/// the env var.
fn build_cors_layer() -> CorsLayer {
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

// === Terminal WebSocket ===

async fn terminal_ws_handler(
    ws: WebSocketUpgrade,
    State(state): State<RouterState>,
) -> axum::response::Response {
    let Some(permit) = ws::try_acquire_ws_slot(&state.ws_slots) else {
        warn!(
            cap = ws::MAX_WS_CONNS_PER_NODE,
            "Refusing /ws/terminal upgrade: WS connection cap reached"
        );
        return StatusCode::SERVICE_UNAVAILABLE.into_response();
    };

    ws.max_frame_size(ws::WS_MAX_FRAME_BYTES)
        .max_message_size(ws::WS_MAX_MESSAGE_BYTES)
        .on_upgrade(move |socket| async move {
            // `permit` is moved into the per-socket task so the slot
            // is only released when the socket task actually exits.
            terminal::handle_terminal_ws(socket).await;
            drop(permit);
        })
        .into_response()
}

// === Health ===

/// Return a liveness/readiness response with version + tool policy.
///
/// The tool-policy fields (`run_command_enabled`, `shell_enabled`,
/// `allowed_commands`) expose the effective executor config so
/// external consumers can diff the running harness's policy against
/// what they need. Historically the `aura-os-desktop` external-harness
/// probe relied on `run_command_enabled` to fail fast when the
/// operator forgot `AURA_AUTONOMOUS_DEV_LOOP=1`; `run_command` is now
/// on by default, so the field is mainly a diagnostic aid for
/// operators who deliberately locked the harness down via
/// `ENABLE_CMD_TOOLS=false` or `AURA_STRICT_MODE=1`.
///
/// The response is deliberately unauthenticated (matches the old
/// minimal-health behaviour) because the information is non-sensitive:
/// anyone who can already reach the health port can trivially discover
/// the same facts by sending any tool invocation and observing the
/// denial. Fields are additive — a missing field in older harness
/// versions means "unknown", and the desktop treats unknown as a warn
/// (not a hard-fail) so mixed-version fleets keep working.
async fn health_handler(State(state): State<RouterState>) -> impl IntoResponse {
    Json(serde_json::json!({
        "status": "ok",
        "version": env!("CARGO_PKG_VERSION"),
        "run_command_enabled": state.tool_config.enable_commands,
        "shell_enabled": state.tool_config.allow_shell,
        "allowed_commands": state.tool_config.command_allowlist,
        "fs_enabled": state.tool_config.enable_fs,
    }))
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
/// cannot fail here; the `.expect(...)` below is a
/// provably-infallible-at-compile-time assertion. Covered by
/// `router::tests::test_global_governor_config_is_valid`.
fn build_global_governor() -> Arc<GovernorCfg> {
    Arc::new(
        GovernorConfigBuilder::default()
            .per_millisecond(1000 / 30) // ≈30 req/sec sustained
            .burst_size(60)
            .finish()
            .expect("global governor config should be valid"),
    )
}

/// Stricter rate limit for mutating endpoints (`/tx`, `/automaton/start`,
/// `:id/pause`, `:id/stop`). 5/sec burst 10.
///
/// INVARIANT: same reasoning as [`build_global_governor`] — hard-coded
/// non-zero integer literals make the `.expect(...)` infallible by
/// construction.
fn build_strict_governor() -> Arc<GovernorCfg> {
    Arc::new(
        GovernorConfigBuilder::default()
            .per_millisecond(200) // 5 req/sec sustained
            .burst_size(10)
            .finish()
            .expect("strict governor config should be valid"),
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
async fn ensure_connect_info(
    mut req: axum::extract::Request,
    next: axum::middleware::Next,
) -> axum::response::Response {
    use axum::extract::ConnectInfo;
    if req.extensions().get::<ConnectInfo<SocketAddr>>().is_none() {
        req.extensions_mut()
            .insert(ConnectInfo(SocketAddr::from(([127, 0, 0, 1], 0))));
    }
    next.run(req).await
}
