//! Storage domain tool handlers (logs, project stats).

use serde_json::{json, Value};
use tracing::debug;

use super::api::DomainApi;
use super::helpers::str_field;

pub async fn create_log(api: &dyn DomainApi, project_id: &str, input: &Value) -> String {
    debug!(project_id, "domain_tools: create_log");
    let message = input["message"].as_str().unwrap_or_default();
    let level = input["level"].as_str().unwrap_or("info");
    let metadata = input.get("metadata");
    let agent_id = str_field(input, "project_agent_id");
    let jwt = str_field(input, "jwt");

    match api.create_log(project_id, message, level, agent_id.as_deref(), metadata, jwt.as_deref()).await {
        Ok(result) => json!({ "ok": true, "result": result }).to_string(),
        Err(e) => json!({ "ok": false, "error": e.to_string() }).to_string(),
    }
}

/// list_logs uses the /api/ endpoint (JWT auth).
pub async fn list_logs(api: &dyn DomainApi, project_id: &str, input: &Value) -> String {
    debug!(project_id, "domain_tools: list_logs");
    let level = str_field(input, "level");
    let limit = input["limit"].as_u64();
    let jwt = str_field(input, "jwt");

    match api.list_logs(project_id, level.as_deref(), limit, jwt.as_deref()).await {
        Ok(result) => json!({ "ok": true, "result": result }).to_string(),
        Err(e) => json!({ "ok": false, "error": e.to_string() }).to_string(),
    }
}

/// get_project_stats uses the /api/ endpoint (JWT auth).
pub async fn get_project_stats(api: &dyn DomainApi, project_id: &str, input: &Value) -> String {
    debug!(project_id, "domain_tools: get_project_stats");
    let jwt = str_field(input, "jwt");

    match api.get_project_stats(project_id, jwt.as_deref()).await {
        Ok(result) => json!({ "ok": true, "result": result }).to_string(),
        Err(e) => json!({ "ok": false, "error": e.to_string() }).to_string(),
    }
}
