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
    extract::{ws::WebSocketUpgrade, Path, Query, State},
    http::{HeaderMap, StatusCode},
    response::IntoResponse,
    routing::{get, post},
    Json, Router,
};
use bytes::Bytes;
use dashmap::DashMap;
use serde::{Deserialize, Serialize};
use std::sync::{Arc, RwLock};
use tower_http::trace::TraceLayer;
use tracing::{error, info, instrument};

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
pub struct RouterState {
    pub store: Arc<dyn Store>,
    pub scheduler: Arc<Scheduler>,
    pub config: NodeConfig,
    /// Model provider for WebSocket sessions (type-erased).
    pub provider: Arc<dyn ModelProvider + Send + Sync>,
    /// Tool configuration for WebSocket sessions.
    pub tool_config: ToolConfig,
    /// Canonical tool catalog (shared across sessions).
    pub catalog: Arc<ToolCatalog>,
    /// Domain API for specs/tasks/project/orbit/network (None if no internal token).
    pub domain_api: Option<Arc<dyn DomainApi>>,
    /// Automaton controller for dev-loop lifecycle (None when domain API unavailable).
    pub automaton_controller: Option<Arc<dyn AutomatonController>>,
    /// Concrete bridge for event subscription (same object as automaton_controller).
    pub automaton_bridge: Option<Arc<AutomatonBridge>>,
    /// tx_id (hex) -> error message for scheduling failures after 202 acceptance.
    pub failed_txs: Arc<DashMap<String, String>>,
    /// Optional memory manager for CRUD API and session injection.
    pub memory_manager: Option<Arc<aura_memory::MemoryManager>>,
    /// Optional skill manager for skill CRUD API and prompt injection.
    pub skill_manager: Option<Arc<RwLock<aura_skills::SkillManager>>>,
    /// Router URL for generation proxying (from `AURA_ROUTER_URL`).
    pub router_url: Option<String>,
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
pub fn create_router(state: RouterState) -> Router {
    Router::new()
        .route("/health", get(health_handler))
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
        .with_state(state)
        .layer(TraceLayer::new_for_http())
}

// === Terminal WebSocket ===

async fn terminal_ws_handler(ws: WebSocketUpgrade) -> impl IntoResponse {
    ws.on_upgrade(terminal::handle_terminal_ws)
}

// === Health ===

/// Return a simple health-check response with version info.
async fn health_handler() -> impl IntoResponse {
    Json(serde_json::json!({
        "status": "ok",
        "version": env!("CARGO_PKG_VERSION")
    }))
}
