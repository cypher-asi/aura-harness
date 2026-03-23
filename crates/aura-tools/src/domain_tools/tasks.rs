//! Task domain tool handlers.

use serde_json::{json, Value};
use tracing::debug;

use super::api::{DomainApi, TaskUpdate};
use super::helpers::{require_str, str_array, str_field};

pub async fn list_tasks(api: &dyn DomainApi, project_id: &str, input: &Value) -> String {
    debug!(project_id, "domain_tools: list_tasks");
    let spec_id = str_field(input, "spec_id");

    match api.list_tasks(project_id, spec_id.as_deref()).await {
        Ok(tasks) => {
            let summaries: Vec<Value> = tasks
                .iter()
                .map(|t| {
                    json!({
                        "task_id": t.id,
                        "spec_id": t.spec_id,
                        "title": t.title,
                        "status": t.status,
                    })
                })
                .collect();
            json!({ "ok": true, "tasks": summaries }).to_string()
        }
        Err(e) => json!({ "ok": false, "error": e.to_string() }).to_string(),
    }
}

pub async fn create_task(api: &dyn DomainApi, _project_id: &str, input: &Value) -> String {
    debug!("domain_tools: create_task");
    let spec_id = match require_str(input, "spec_id") {
        Ok(id) => id,
        Err(e) => return json!({ "ok": false, "error": e }).to_string(),
    };
    let title = str_field(input, "title").unwrap_or_default();
    let description = str_field(input, "description").unwrap_or_default();
    let deps = str_array(input, "dependency_ids");

    match api.create_task(&spec_id, &title, &description, &deps).await {
        Ok(t) => json!({ "ok": true, "task": t }).to_string(),
        Err(e) => json!({ "ok": false, "error": e.to_string() }).to_string(),
    }
}

pub async fn update_task(api: &dyn DomainApi, _project_id: &str, input: &Value) -> String {
    debug!("domain_tools: update_task");
    let task_id = match require_str(input, "task_id") {
        Ok(id) => id,
        Err(e) => return json!({ "ok": false, "error": e }).to_string(),
    };
    let updates = TaskUpdate {
        title: str_field(input, "title"),
        description: str_field(input, "description"),
        status: str_field(input, "status"),
    };
    match api.update_task(&task_id, updates).await {
        Ok(t) => json!({ "ok": true, "task": t }).to_string(),
        Err(e) => json!({ "ok": false, "error": e.to_string() }).to_string(),
    }
}

pub async fn delete_task(api: &dyn DomainApi, _project_id: &str, input: &Value) -> String {
    debug!("domain_tools: delete_task");
    let task_id = match require_str(input, "task_id") {
        Ok(id) => id,
        Err(e) => return json!({ "ok": false, "error": e }).to_string(),
    };
    match api
        .update_task(
            &task_id,
            TaskUpdate {
                status: Some("deleted".to_string()),
                ..Default::default()
            },
        )
        .await
    {
        Ok(_) => json!({ "ok": true, "deleted": task_id }).to_string(),
        Err(e) => json!({ "ok": false, "error": e.to_string() }).to_string(),
    }
}

pub async fn transition_task(api: &dyn DomainApi, _project_id: &str, input: &Value) -> String {
    debug!("domain_tools: transition_task");
    let task_id = match require_str(input, "task_id") {
        Ok(id) => id,
        Err(e) => return json!({ "ok": false, "error": e }).to_string(),
    };
    let status = match require_str(input, "status") {
        Ok(s) => s,
        Err(e) => return json!({ "ok": false, "error": e }).to_string(),
    };
    match api.transition_task(&task_id, &status).await {
        Ok(t) => json!({ "ok": true, "task": t }).to_string(),
        Err(e) => json!({ "ok": false, "error": e.to_string() }).to_string(),
    }
}
