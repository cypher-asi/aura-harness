//! Per-area sub-routers composed by [`super::create_router`].
//!
//! Per-route body limits — tighter ceilings for endpoints that have
//! no legitimate reason to accept a large body. Each one is a
//! *layer* so it overrides the 1 MiB global default applied by
//! [`super::create_router`]. Phase 9 of the security audit.

use axum::{
    extract::DefaultBodyLimit,
    routing::{delete, get, post, put},
    Router,
};
use tower_governor::GovernorLayer;

use super::super::handlers::cloud;
use super::super::handlers::files::{
    delete_workspace_handler, import_workspace_handler, list_files_handler, read_file_handler,
    resolve_workspace_handler, write_file_handler,
};
use super::super::handlers::memory;
use super::super::handlers::processes::{
    create_process_handler, delete_process_handler, get_process_handler, list_process_runs_handler,
    list_processes_handler, trigger_process_handler, update_process_handler,
};
use super::super::handlers::run::{
    run_list_handler, run_pause_handler, run_start_handler, run_status_handler, run_stop_handler,
};
use super::super::handlers::run_ws::run_ws_handler;
use super::super::handlers::safe_workspace::{
    apply_safe_workspace_handler, prepare_safe_workspace_handler, restore_safe_workspace_handler,
    safe_workspace_diff_handler, safe_workspace_status_handler,
};
use super::super::handlers::secrets::{
    delete_secret_handler, get_secret_handler, list_secrets_handler, put_secret_handler,
};
use super::super::handlers::skills;
use super::super::handlers::tool_permissions::{
    get_agent_tool_permissions_handler, get_agent_tools_handler, get_user_tool_defaults_handler,
    put_agent_tool_permissions_handler, put_user_tool_defaults_handler,
};
use super::super::handlers::tx::{
    get_head_handler, scan_record_handler, submit_tx_handler, tool_call_audit_handler,
    tx_status_handler,
};
use super::super::RouterState;
use super::handlers::{health_handler, terminal_ws_handler};
use super::layers::build_strict_governor;

fn body_limit_1k() -> DefaultBodyLimit {
    DefaultBodyLimit::max(1024)
}

fn body_limit_4k() -> DefaultBodyLimit {
    DefaultBodyLimit::max(4 * 1024)
}

fn body_limit_16k() -> DefaultBodyLimit {
    DefaultBodyLimit::max(16 * 1024)
}

fn body_limit_12m() -> DefaultBodyLimit {
    DefaultBodyLimit::max(12 * 1024 * 1024)
}

/// The unauthenticated sub-router: just `GET /health`.
pub(super) fn public_routes() -> Router<RouterState> {
    Router::new().route("/health", get(health_handler).route_layer(body_limit_1k()))
}

/// Strict-rate-limit sub-router: `/tx`, `/v1/run`, `/api/write-file`, and the
/// `:id/pause` + `:id/stop` path params. Pause/stop use a 4 KiB
/// body limit for tiny JSON payloads; `/tx` and `/v1/run` keep
/// the 1 MiB default because legitimate requests can be large.
///
/// Mutating JSON endpoints get a stricter per-IP governor (5/sec,
/// burst 10) so a misbehaving client can't flood writes even if
/// they stay under the global 30/sec cap. See `build_strict_governor`.
pub(super) fn strict_routes() -> Router<RouterState> {
    let strict_governor_layer = GovernorLayer {
        config: build_strict_governor(),
    };

    let strict_small_body = Router::new()
        .route("/v1/run/{run_id}/pause", post(run_pause_handler))
        .route("/v1/run/{run_id}/stop", post(run_stop_handler))
        .route(
            "/workspace/{workspace_key}/safe/{session_id}",
            post(prepare_safe_workspace_handler),
        )
        .route(
            "/workspace/{workspace_key}/safe/{session_id}/checkpoints/{checkpoint_id}/restore",
            post(restore_safe_workspace_handler),
        )
        .route(
            "/workspace/{workspace_key}/safe/{session_id}/apply",
            post(apply_safe_workspace_handler),
        )
        .route_layer(body_limit_4k());

    let strict_default_body = Router::new()
        .route("/tx", post(submit_tx_handler))
        .route("/v1/run", post(run_start_handler))
        .route("/api/write-file", put(write_file_handler));

    strict_small_body
        .merge(strict_default_body)
        .route_layer(strict_governor_layer)
}

/// File browsing and workspace (incl. safe-workspace read) routes.
pub(super) fn file_routes() -> Router<RouterState> {
    Router::new()
        .route(
            "/api/files",
            get(list_files_handler).route_layer(body_limit_16k()),
        )
        .route(
            "/api/read-file",
            get(read_file_handler).route_layer(body_limit_16k()),
        )
        .route(
            "/workspace/resolve",
            get(resolve_workspace_handler).route_layer(body_limit_16k()),
        )
        .route(
            "/workspace/import",
            post(import_workspace_handler).route_layer(body_limit_12m()),
        )
        .route(
            "/workspace/{workspace_key}",
            delete(delete_workspace_handler).route_layer(body_limit_16k()),
        )
        .route(
            "/workspace/{workspace_key}/safe/{session_id}",
            get(safe_workspace_status_handler).route_layer(body_limit_1k()),
        )
        .route(
            "/workspace/{workspace_key}/safe/{session_id}/checkpoints/{checkpoint_id}/diff",
            get(safe_workspace_diff_handler).route_layer(body_limit_1k()),
        )
}

/// Transaction / record read routes and per-agent tool permissions.
pub(super) fn agent_routes() -> Router<RouterState> {
    Router::new()
        .route(
            "/tx/status/{agent_id}/{tx_id}",
            get(tx_status_handler).route_layer(body_limit_1k()),
        )
        .route(
            "/agents/{agent_id}/head",
            get(get_head_handler).route_layer(body_limit_1k()),
        )
        .route(
            "/agents/{agent_id}/record",
            get(scan_record_handler).route_layer(body_limit_16k()),
        )
        .route(
            "/agents/{agent_id}/tool_calls/{tool_use_id}/audit",
            get(tool_call_audit_handler).route_layer(body_limit_1k()),
        )
        .route(
            "/users/{user_id}/tool-defaults",
            get(get_user_tool_defaults_handler).put(put_user_tool_defaults_handler),
        )
        .route(
            "/agents/{agent_id}/tool-permissions",
            get(get_agent_tool_permissions_handler).put(put_agent_tool_permissions_handler),
        )
        .route("/agents/{agent_id}/tools", get(get_agent_tools_handler))
}

/// WebSocket streams and run read routes.
pub(super) fn stream_and_run_routes() -> Router<RouterState> {
    Router::new()
        .route("/ws/terminal", get(terminal_ws_handler))
        .route(
            "/ws/preview/tcp/{port}",
            get(crate::gateway::handlers::preview_tcp::preview_tcp_ws),
        )
        .route("/stream/{run_id}", get(run_ws_handler))
        .route(
            "/v1/run/list",
            get(run_list_handler).route_layer(body_limit_1k()),
        )
        .route(
            "/v1/run/{run_id}/status",
            get(run_status_handler).route_layer(body_limit_1k()),
        )
}

/// Memory CRUD routes shared by the canonical `/memory/:agent_id` paths and
/// the `/api/agents/:agent_id/memory` aliases.
fn memory_crud_routes(prefix: &str) -> Router<RouterState> {
    Router::new()
        .route(
            &format!("{prefix}/facts"),
            get(memory::list_facts).post(memory::create_fact),
        )
        .route(
            &format!("{prefix}/facts/{{id}}"),
            get(memory::get_fact)
                .put(memory::update_fact)
                .delete(memory::delete_fact),
        )
        .route(
            &format!("{prefix}/facts/by-key/{{key}}"),
            get(memory::get_fact_by_key),
        )
        .route(
            &format!("{prefix}/events"),
            get(memory::list_events).post(memory::create_event),
        )
        .route(
            &format!("{prefix}/events/{{id}}"),
            delete(memory::delete_event),
        )
        .route(
            &format!("{prefix}/events/bulk-delete"),
            post(memory::bulk_delete_events),
        )
        .route(
            &format!("{prefix}/procedures"),
            get(memory::list_procedures).post(memory::create_procedure),
        )
        .route(
            &format!("{prefix}/procedures/{{id}}"),
            get(memory::get_procedure)
                .put(memory::update_procedure)
                .delete(memory::delete_procedure),
        )
        .route(&format!("{prefix}/stats"), get(memory::stats))
        .route(
            &format!("{prefix}/continuity"),
            get(memory::get_continuity_config).put(memory::update_continuity_config),
        )
        .route(
            &format!("{prefix}/retrieval/latest"),
            get(memory::latest_retrieval_trace),
        )
        .route(&format!("{prefix}/consolidate"), post(memory::consolidate))
}

/// Memory CRUD: canonical paths plus the aliases the aura-os proxy uses
/// (`/api/agents/:id/memory/...`).
pub(super) fn memory_routes() -> Router<RouterState> {
    let canonical = memory_crud_routes("/memory/{agent_id}")
        .route("/memory/{agent_id}/snapshot", get(memory::snapshot))
        .route("/memory/{agent_id}/wipe", post(memory::wipe));
    let aliases = memory_crud_routes("/api/agents/{agent_id}/memory").route(
        "/api/agents/{agent_id}/memory",
        get(memory::snapshot).delete(memory::wipe),
    );
    canonical.merge(aliases)
}

/// Skills CRUD and per-agent skill installations.
pub(super) fn skill_routes() -> Router<RouterState> {
    Router::new()
        .route(
            "/api/skills",
            get(skills::list_skills).post(skills::create_skill),
        )
        .route("/api/skills/{name}", get(skills::get_skill))
        .route("/api/skills/{name}/activate", post(skills::activate_skill))
        .route(
            "/api/agents/{agent_id}/skills",
            get(skills::list_agent_skills).post(skills::install_agent_skill),
        )
        .route(
            "/api/agents/{agent_id}/skills/{name}",
            delete(skills::uninstall_agent_skill),
        )
        // Legacy compatibility aliases for older harness callers.
        .route(
            "/api/harness/agents/{agent_id}/skills",
            get(skills::list_agent_skills).post(skills::install_agent_skill),
        )
        .route(
            "/api/harness/agents/{agent_id}/skills/{name}",
            delete(skills::uninstall_agent_skill),
        )
}

/// In-TEE secrets vault (Swarm TEE phase 6). Proxied by the swarm
/// gateway as /v1/agents/:id/secrets[/:name]; values are sealed at
/// rest under the state DEK and only leave the vault on explicit
/// ?reveal=true reads.
pub(super) fn secret_routes() -> Router<RouterState> {
    Router::new()
        .route("/secrets", get(list_secrets_handler))
        .route(
            "/secrets/{name}",
            get(get_secret_handler)
                .put(put_secret_handler)
                .delete(delete_secret_handler)
                .route_layer(body_limit_16k()),
        )
}

/// In-TEE processes / automations (Swarm TEE phase 7). The
/// definitions (cron + prompt + config) and run history are
/// sealed at rest and only readable through this authenticated
/// in-VM API; the sole off-VM export is the trigger-metadata
/// seam (`ProcessStore::trigger_metadata`), wired up in the
/// next phase. `/v1` prefix matches the `/v1/run` route family.
pub(super) fn process_routes() -> Router<RouterState> {
    Router::new()
        .route(
            "/v1/processes",
            get(list_processes_handler).post(create_process_handler),
        )
        .route(
            "/v1/processes/{id}",
            get(get_process_handler)
                .put(update_process_handler)
                .delete(delete_process_handler),
        )
        .route("/v1/processes/{id}/trigger", post(trigger_process_handler))
        .route(
            "/v1/processes/{id}/runs",
            get(list_process_runs_handler).route_layer(body_limit_16k()),
        )
}

/// Routes a cloud node serves its hub and peers; they answer `404` on
/// other nodes. See [`cloud`].
pub(super) fn cloud_routes() -> Router<RouterState> {
    // Record pages and session checkpoints for an agent's hand-off can be
    // large.
    let import_limit = DefaultBodyLimit::max(64 * 1024 * 1024);
    Router::new()
        .route("/v1/host", get(cloud::host_handler))
        .route("/v1/cloud/processes", get(cloud::processes_handler))
        .route(
            "/v1/cloud/skills/{agent_id}",
            put(cloud::skills_put_handler).route_layer(DefaultBodyLimit::max(8 * 1024 * 1024)),
        )
        .route(
            "/v1/sync/import",
            post(cloud::sync_import_handler).route_layer(import_limit),
        )
        .route("/v1/sync/seal/{agent_id}", post(cloud::sync_seal_handler))
        .route(
            "/v1/sync/session/{agent_id}",
            put(cloud::sync_session_put_handler)
                .get(cloud::sync_session_get_handler)
                .route_layer(import_limit),
        )
        .route(
            "/v1/sync/session/{agent_id}/new",
            post(cloud::sync_session_new_handler),
        )
        .route("/v1/sync/traces", get(cloud::sync_traces_handler))
        .route("/v1/workspace/patch", get(cloud::workspace_patch_handler))
        .route("/v1/mesh/mail", post(cloud::mesh_mail_handler))
        .route("/v1/mesh", get(cloud::mesh_ws_handler))
}
