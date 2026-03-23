//! Tool installation and management endpoints.

use aura_core::InstalledToolDefinition;
use aura_tools::ToolInstaller;
use axum::{
    extract::{Path, State},
    http::StatusCode,
    response::IntoResponse,
    Json,
};
use serde::Serialize;
use std::sync::Arc;
use tracing::info;

/// Response for GET /tools -- tool info without auth secrets.
#[derive(Debug, Serialize)]
struct ToolListEntry {
    name: String,
    description: String,
    endpoint: String,
    namespace: Option<String>,
}

/// POST /tools/install -- install or replace a tool definition.
pub async fn install_tool_handler(
    State(installer): State<Arc<ToolInstaller>>,
    Json(def): Json<InstalledToolDefinition>,
) -> impl IntoResponse {
    let name = def.name.clone();
    installer.install(def);
    info!(tool = %name, "Tool installed via API");
    (StatusCode::OK, Json(serde_json::json!({ "installed": name })))
}

/// DELETE /tools/:name -- uninstall a tool by name.
pub async fn delete_tool_handler(
    State(installer): State<Arc<ToolInstaller>>,
    Path(name): Path<String>,
) -> impl IntoResponse {
    if installer.uninstall(&name) {
        info!(tool = %name, "Tool uninstalled via API");
        (
            StatusCode::OK,
            Json(serde_json::json!({ "uninstalled": name })),
        )
    } else {
        (
            StatusCode::NOT_FOUND,
            Json(serde_json::json!({ "error": format!("Tool '{name}' not found") })),
        )
    }
}

/// GET /tools -- list all installed tools (no auth secrets).
pub async fn get_tools_handler(
    State(installer): State<Arc<ToolInstaller>>,
) -> impl IntoResponse {
    let tools: Vec<ToolListEntry> = installer
        .snapshot()
        .into_iter()
        .map(|def| ToolListEntry {
            name: def.name,
            description: def.description,
            endpoint: def.endpoint,
            namespace: def.namespace,
        })
        .collect();
    Json(tools)
}
