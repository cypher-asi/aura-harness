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
use std::sync::{Arc, RwLock};
use std::time::Duration;
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
mod tx;
mod ws;

use automaton::*;
use files::*;
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
}

/// Input bundle for [`RouterState::new`].
///
/// Grouped into a single parameter struct so we don't have to thread 13
/// positional arguments through every test and binary. Optional fields
/// mirror the ones that default to `None` on the router state.
pub struct RouterStateConfig {
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
pub fn create_router(state: RouterState) -> Router {
    let public = Router::new().route("/health", get(health_handler));

    let protected = Router::new()
        .route("/api/files", get(list_files_handler))
        .route("/api/read-file", get(read_file_handler))
        .route("/workspace/resolve", get(resolve_workspace_handler))
        .route("/tx", post(submit_tx_handler))
        .route("/tx/status/:agent_id/:tx_id", get(tx_status_handler))
        .route("/agents/:agent_id/head", get(get_head_handler))
        .route("/agents/:agent_id/record", get(scan_record_handler))
        .route("/ws/terminal", get(terminal_ws_handler))
        .route("/stream", get(ws_upgrade_handler))
        .route("/stream/automaton/:automaton_id", get(automaton_ws_handler))
        .route("/automaton/start", post(automaton_start_handler))
        .route("/automaton/list", get(automaton_list_handler))
        .route(
            "/automaton/:automaton_id/status",
            get(automaton_status_handler),
        )
        .route(
            "/automaton/:automaton_id/pause",
            post(automaton_pause_handler),
        )
        .route(
            "/automaton/:automaton_id/stop",
            post(automaton_stop_handler),
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
        .route_layer(axum::middleware::from_fn_with_state(
            state.clone(),
            auth::require_bearer_mw,
        ));

    Router::new()
        .merge(public)
        .merge(protected)
        .with_state(state)
        // Security + observability layers (Wave 5 / T1).
        //
        // Order matters: outermost first. DefaultBodyLimit caps request
        // bodies BEFORE they reach handlers so malicious clients can't
        // blow up memory via `/tx`. Anonymous routes (GET /health,
        // GET /api/files, GET /workspace/resolve) stay anonymous — see
        // per-handler comments — but they still inherit the body limit,
        // CORS allow-list, request timeout, and trace span.
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

async fn terminal_ws_handler(ws: WebSocketUpgrade) -> impl IntoResponse {
    ws.max_frame_size(ws::WS_MAX_FRAME_BYTES)
        .max_message_size(ws::WS_MAX_MESSAGE_BYTES)
        .on_upgrade(terminal::handle_terminal_ws)
}

// === Health ===

/// Return a simple health-check response with version info.
async fn health_handler() -> impl IntoResponse {
    Json(serde_json::json!({
        "status": "ok",
        "version": env!("CARGO_PKG_VERSION")
    }))
}
