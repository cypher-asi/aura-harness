//! Task domain tool handlers.

use serde_json::{json, Value};
use tracing::debug;

use super::api::{DomainApi, TaskUpdate};
use super::helpers::{domain_err, domain_ok, require_str, str_array, str_field};

pub async fn get_task(api: &dyn DomainApi, _project_id: &str, input: &Value) -> String {
    debug!("domain_tools: get_task");
    let task_id = match require_str(input, "task_id") {
        Ok(id) => id,
        Err(e) => return domain_err(&e),
    };
    let jwt = str_field(input, "jwt");
    match api.get_task(&task_id, jwt.as_deref()).await {
        Ok(t) => domain_ok(json!({ "task": t })),
        Err(e) => domain_err(e),
    }
}

pub async fn list_tasks(api: &dyn DomainApi, project_id: &str, input: &Value) -> String {
    debug!(project_id, "domain_tools: list_tasks");
    let spec_id = str_field(input, "spec_id");
    let jwt = str_field(input, "jwt");

    match api
        .list_tasks(project_id, spec_id.as_deref(), jwt.as_deref())
        .await
    {
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
            domain_ok(json!({ "tasks": summaries }))
        }
        Err(e) => domain_err(e),
    }
}

pub async fn create_task(api: &dyn DomainApi, project_id: &str, input: &Value) -> String {
    debug!("domain_tools: create_task");
    let spec_id = match require_str(input, "spec_id") {
        Ok(id) => id,
        Err(e) => return domain_err(&e),
    };
    let title = str_field(input, "title").unwrap_or_default();
    let description = str_field(input, "description").unwrap_or_default();
    let deps = str_array(input, "dependency_ids");
    let jwt = str_field(input, "jwt");

    // Auto-derive orderIndex from existing task count within this spec.
    let order = match api
        .list_tasks(project_id, Some(&spec_id), jwt.as_deref())
        .await
    {
        #[allow(clippy::cast_possible_truncation)]
        Ok(tasks) => tasks.len() as u32,
        Err(_) => 0,
    };

    match api
        .create_task(
            project_id,
            &spec_id,
            &title,
            &description,
            &deps,
            order,
            jwt.as_deref(),
        )
        .await
    {
        Ok(t) => domain_ok(json!({ "task": t })),
        Err(e) => domain_err(e),
    }
}

pub async fn update_task(api: &dyn DomainApi, _project_id: &str, input: &Value) -> String {
    debug!("domain_tools: update_task");
    let task_id = match require_str(input, "task_id") {
        Ok(id) => id,
        Err(e) => return domain_err(&e),
    };
    let updates = TaskUpdate {
        title: str_field(input, "title"),
        description: str_field(input, "description"),
        status: str_field(input, "status"),
    };
    let jwt = str_field(input, "jwt");
    match api.update_task(&task_id, updates, jwt.as_deref()).await {
        Ok(t) => domain_ok(json!({ "task": t })),
        Err(e) => domain_err(e),
    }
}

pub async fn delete_task(api: &dyn DomainApi, _project_id: &str, input: &Value) -> String {
    debug!("domain_tools: delete_task");
    let task_id = match require_str(input, "task_id") {
        Ok(id) => id,
        Err(e) => return domain_err(&e),
    };
    let jwt = str_field(input, "jwt");
    match api.delete_task(&task_id, jwt.as_deref()).await {
        Ok(()) => domain_ok(json!({ "deleted": task_id })),
        Err(e) => domain_err(e),
    }
}

pub async fn transition_task(api: &dyn DomainApi, _project_id: &str, input: &Value) -> String {
    debug!("domain_tools: transition_task");
    let task_id = match require_str(input, "task_id") {
        Ok(id) => id,
        Err(e) => return domain_err(&e),
    };
    let status = match require_str(input, "status") {
        Ok(s) => s,
        Err(e) => return domain_err(&e),
    };
    let jwt = str_field(input, "jwt");
    match api.transition_task(&task_id, &status, jwt.as_deref()).await {
        Ok(t) => domain_ok(json!({ "task": t })),
        Err(e) => domain_err(e),
    }
}
