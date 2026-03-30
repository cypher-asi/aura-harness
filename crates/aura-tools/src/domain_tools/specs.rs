//! Spec domain tool handlers.

use serde_json::{json, Value};
use tracing::debug;

use super::api::DomainApi;
use super::helpers::{domain_err, domain_ok, require_str, str_field};

pub async fn list_specs(api: &dyn DomainApi, project_id: &str, input: &Value) -> String {
    debug!(project_id, "domain_tools: list_specs");
    let jwt = str_field(input, "jwt");
    match api.list_specs(project_id, jwt.as_deref()).await {
        Ok(specs) => {
            let summaries: Vec<Value> = specs
                .iter()
                .map(|s| {
                    json!({
                        "spec_id": s.id,
                        "title": s.title,
                        "order": s.order,
                    })
                })
                .collect();
            domain_ok(json!({ "specs": summaries }))
        }
        Err(e) => domain_err(e),
    }
}

pub async fn get_spec(api: &dyn DomainApi, project_id: &str, input: &Value) -> String {
    debug!(project_id, "domain_tools: get_spec");
    let spec_id = match require_str(input, "spec_id") {
        Ok(id) => id,
        Err(e) => return domain_err(&e),
    };
    let jwt = str_field(input, "jwt");
    match api.get_spec(&spec_id, jwt.as_deref()).await {
        Ok(s) => domain_ok(json!({ "spec": s })),
        Err(e) => domain_err(e),
    }
}

pub async fn create_spec(api: &dyn DomainApi, project_id: &str, input: &Value) -> String {
    debug!(project_id, "domain_tools: create_spec");
    let title = str_field(input, "title").unwrap_or_default();
    let content = str_field(input, "markdown_contents")
        .or_else(|| str_field(input, "content"))
        .unwrap_or_default();
    let jwt = str_field(input, "jwt");

    // Auto-derive orderIndex from existing spec count so specs are
    // numbered in creation order without the caller needing to track it.
    let order = match api.list_specs(project_id, jwt.as_deref()).await {
        Ok(specs) => specs.len() as u32,
        Err(_) => 0,
    };

    match api.create_spec(project_id, &title, &content, order, jwt.as_deref()).await {
        Ok(s) => domain_ok(json!({ "spec": s })),
        Err(e) => domain_err(e),
    }
}

pub async fn update_spec(api: &dyn DomainApi, _project_id: &str, input: &Value) -> String {
    debug!("domain_tools: update_spec");
    let spec_id = match require_str(input, "spec_id") {
        Ok(id) => id,
        Err(e) => return domain_err(&e),
    };
    let title = str_field(input, "title");
    let content = str_field(input, "markdown_contents").or_else(|| str_field(input, "content"));
    let jwt = str_field(input, "jwt");

    match api
        .update_spec(&spec_id, title.as_deref(), content.as_deref(), jwt.as_deref())
        .await
    {
        Ok(s) => domain_ok(json!({ "spec": s })),
        Err(e) => domain_err(e),
    }
}

pub async fn delete_spec(api: &dyn DomainApi, _project_id: &str, input: &Value) -> String {
    debug!("domain_tools: delete_spec");
    let spec_id = match require_str(input, "spec_id") {
        Ok(id) => id,
        Err(e) => return domain_err(&e),
    };
    let jwt = str_field(input, "jwt");
    match api.delete_spec(&spec_id, jwt.as_deref()).await {
        Ok(()) => domain_ok(json!({ "deleted": spec_id })),
        Err(e) => domain_err(e),
    }
}
