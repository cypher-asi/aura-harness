//! Project domain tool handlers.

use serde_json::{json, Value};
use tracing::debug;

use super::api::{DomainApi, ProjectUpdate};
use super::helpers::{domain_err, domain_ok, str_field};

pub async fn get_project(api: &dyn DomainApi, project_id: &str, input: &Value) -> String {
    debug!(project_id, "domain_tools: get_project");
    let jwt = str_field(input, "jwt");
    match api.get_project(project_id, jwt.as_deref()).await {
        Ok(p) => domain_ok(json!({ "project": p })),
        Err(e) => domain_err(e),
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
    let jwt = str_field(input, "jwt");
    match api
        .update_project(project_id, updates, jwt.as_deref())
        .await
    {
        Ok(p) => domain_ok(json!({ "project": p })),
        Err(e) => domain_err(e),
    }
}
