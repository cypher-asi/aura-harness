//! Project domain tool handlers.

use serde_json::{json, Value};
use tracing::debug;

use super::api::{DomainApi, ProjectUpdate};
use super::helpers::str_field;

pub async fn get_project(api: &dyn DomainApi, project_id: &str, _input: &Value) -> String {
    debug!(project_id, "domain_tools: get_project");
    match api.get_project(project_id).await {
        Ok(p) => json!({ "ok": true, "project": p }).to_string(),
        Err(e) => json!({ "ok": false, "error": e.to_string() }).to_string(),
    }
}

pub async fn update_project(api: &dyn DomainApi, project_id: &str, input: &Value) -> String {
    debug!(project_id, "domain_tools: update_project");
    let updates = ProjectUpdate {
        name: str_field(input, "name"),
        description: str_field(input, "description"),
        tech_stack: str_field(input, "tech_stack"),
        build_command: str_field(input, "build_command"),
        test_command: str_field(input, "test_command"),
    };
    match api.update_project(project_id, updates).await {
        Ok(p) => json!({ "ok": true, "project": p }).to_string(),
        Err(e) => json!({ "ok": false, "error": e.to_string() }).to_string(),
    }
}
