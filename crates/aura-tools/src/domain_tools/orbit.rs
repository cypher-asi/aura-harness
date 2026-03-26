//! Orbit domain tool handlers.

use serde_json::{json, Value};
use tracing::debug;

use super::api::DomainApi;
use super::helpers::str_field;

pub async fn orbit_list_repos(api: &dyn DomainApi, _project_id: &str, input: &Value) -> String {
    debug!("domain_tools: orbit_list_repos");
    let jwt = str_field(input, "jwt").unwrap_or_default();
    match api.orbit_api_call("GET", "/repos", None, Some(&jwt)).await {
        Ok(body) => json!({ "ok": true, "result": serde_json::from_str::<Value>(&body).unwrap_or(Value::String(body.clone())) }).to_string(),
        Err(e) => json!({ "ok": false, "error": e.to_string() }).to_string(),
    }
}

pub async fn orbit_create_repo(api: &dyn DomainApi, _project_id: &str, input: &Value) -> String {
    debug!("domain_tools: orbit_create_repo");
    let body = json!({
        "orgId": input["org_id"].as_str().unwrap_or_default(),
        "projectId": input["project_id"].as_str().unwrap_or_default(),
        "ownerId": input["owner_id"].as_str().unwrap_or_default(),
        "name": input["name"].as_str().unwrap_or_default(),
        "visibility": input["visibility"].as_str().unwrap_or("private"),
    });
    let jwt = str_field(input, "jwt");
    match api.orbit_api_call("POST", "/internal/repos", Some(&body), jwt.as_deref()).await {
        Ok(r) => json!({ "ok": true, "result": serde_json::from_str::<Value>(&r).unwrap_or(Value::String(r.clone())) }).to_string(),
        Err(e) => json!({ "ok": false, "error": e.to_string() }).to_string(),
    }
}

pub async fn orbit_list_branches(api: &dyn DomainApi, _project_id: &str, input: &Value) -> String {
    debug!("domain_tools: orbit_list_branches");
    let org_id = input["org_id"].as_str().unwrap_or_default();
    let repo = input["repo"].as_str().unwrap_or_default();
    let jwt = str_field(input, "jwt").unwrap_or_default();
    let path = format!("/repos/{org_id}/{repo}/branches");
    match api.orbit_api_call("GET", &path, None, Some(&jwt)).await {
        Ok(r) => json!({ "ok": true, "result": serde_json::from_str::<Value>(&r).unwrap_or(Value::String(r.clone())) }).to_string(),
        Err(e) => json!({ "ok": false, "error": e.to_string() }).to_string(),
    }
}

pub async fn orbit_create_branch(api: &dyn DomainApi, _project_id: &str, input: &Value) -> String {
    debug!("domain_tools: orbit_create_branch");
    let org_id = input["org_id"].as_str().unwrap_or_default();
    let repo = input["repo"].as_str().unwrap_or_default();
    let jwt = str_field(input, "jwt").unwrap_or_default();
    let body = json!({
        "name": input["name"].as_str().unwrap_or_default(),
        "source": input["source"].as_str().unwrap_or("main"),
    });
    let path = format!("/repos/{org_id}/{repo}/branches");
    match api.orbit_api_call("POST", &path, Some(&body), Some(&jwt)).await {
        Ok(r) => json!({ "ok": true, "result": serde_json::from_str::<Value>(&r).unwrap_or(Value::String(r.clone())) }).to_string(),
        Err(e) => json!({ "ok": false, "error": e.to_string() }).to_string(),
    }
}

pub async fn orbit_list_commits(api: &dyn DomainApi, _project_id: &str, input: &Value) -> String {
    debug!("domain_tools: orbit_list_commits");
    let org_id = input["org_id"].as_str().unwrap_or_default();
    let repo = input["repo"].as_str().unwrap_or_default();
    let git_ref = input["ref"].as_str().unwrap_or_default();
    let limit = input["limit"].as_u64().unwrap_or(20);
    let jwt = str_field(input, "jwt").unwrap_or_default();
    let mut path = format!("/repos/{org_id}/{repo}/commits?limit={limit}");
    if !git_ref.is_empty() {
        path.push_str(&format!("&ref={git_ref}"));
    }
    match api.orbit_api_call("GET", &path, None, Some(&jwt)).await {
        Ok(r) => json!({ "ok": true, "result": serde_json::from_str::<Value>(&r).unwrap_or(Value::String(r.clone())) }).to_string(),
        Err(e) => json!({ "ok": false, "error": e.to_string() }).to_string(),
    }
}

pub async fn orbit_get_diff(api: &dyn DomainApi, _project_id: &str, input: &Value) -> String {
    debug!("domain_tools: orbit_get_diff");
    let org_id = input["org_id"].as_str().unwrap_or_default();
    let repo = input["repo"].as_str().unwrap_or_default();
    let sha = input["sha"].as_str().unwrap_or_default();
    let jwt = str_field(input, "jwt").unwrap_or_default();
    let path = format!("/repos/{org_id}/{repo}/commits/{sha}/diff");
    match api.orbit_api_call("GET", &path, None, Some(&jwt)).await {
        Ok(r) => json!({ "ok": true, "result": serde_json::from_str::<Value>(&r).unwrap_or(Value::String(r.clone())) }).to_string(),
        Err(e) => json!({ "ok": false, "error": e.to_string() }).to_string(),
    }
}

pub async fn orbit_create_pr(api: &dyn DomainApi, _project_id: &str, input: &Value) -> String {
    debug!("domain_tools: orbit_create_pr");
    let org_id = input["org_id"].as_str().unwrap_or_default();
    let repo = input["repo"].as_str().unwrap_or_default();
    let jwt = str_field(input, "jwt").unwrap_or_default();
    let body = json!({
        "sourceBranch": input["source_branch"].as_str().unwrap_or_default(),
        "targetBranch": input["target_branch"].as_str().unwrap_or_default(),
        "title": input["title"].as_str().unwrap_or_default(),
        "description": input["description"].as_str().unwrap_or_default(),
    });
    let path = format!("/repos/{org_id}/{repo}/pulls");
    match api.orbit_api_call("POST", &path, Some(&body), Some(&jwt)).await {
        Ok(r) => json!({ "ok": true, "result": serde_json::from_str::<Value>(&r).unwrap_or(Value::String(r.clone())) }).to_string(),
        Err(e) => json!({ "ok": false, "error": e.to_string() }).to_string(),
    }
}

pub async fn orbit_list_prs(api: &dyn DomainApi, _project_id: &str, input: &Value) -> String {
    debug!("domain_tools: orbit_list_prs");
    let org_id = input["org_id"].as_str().unwrap_or_default();
    let repo = input["repo"].as_str().unwrap_or_default();
    let status = input["status"].as_str().unwrap_or_default();
    let jwt = str_field(input, "jwt").unwrap_or_default();
    let mut path = format!("/repos/{org_id}/{repo}/pulls");
    if !status.is_empty() {
        path.push_str(&format!("?status={status}"));
    }
    match api.orbit_api_call("GET", &path, None, Some(&jwt)).await {
        Ok(r) => json!({ "ok": true, "result": serde_json::from_str::<Value>(&r).unwrap_or(Value::String(r.clone())) }).to_string(),
        Err(e) => json!({ "ok": false, "error": e.to_string() }).to_string(),
    }
}

pub async fn orbit_merge_pr(api: &dyn DomainApi, _project_id: &str, input: &Value) -> String {
    debug!("domain_tools: orbit_merge_pr");
    let org_id = input["org_id"].as_str().unwrap_or_default();
    let repo = input["repo"].as_str().unwrap_or_default();
    let pr_id = input["pr_id"].as_str().unwrap_or_default();
    let jwt = str_field(input, "jwt").unwrap_or_default();
    let body = json!({
        "strategy": input["strategy"].as_str().unwrap_or("merge"),
    });
    let path = format!("/repos/{org_id}/{repo}/pulls/{pr_id}/merge");
    match api.orbit_api_call("POST", &path, Some(&body), Some(&jwt)).await {
        Ok(r) => json!({ "ok": true, "result": serde_json::from_str::<Value>(&r).unwrap_or(Value::String(r.clone())) }).to_string(),
        Err(e) => json!({ "ok": false, "error": e.to_string() }).to_string(),
    }
}

pub async fn orbit_push(api: &dyn DomainApi, _project_id: &str, input: &Value) -> String {
    debug!("domain_tools: orbit_push");
    let org_id = input["org_id"].as_str().unwrap_or_default();
    let repo = input["repo"].as_str().unwrap_or_default();
    let branch = input["branch"].as_str().unwrap_or_default();
    let force = input["force"].as_bool().unwrap_or(false);
    let jwt = str_field(input, "jwt").unwrap_or_default();
    let workspace = str_field(input, "workspace").unwrap_or_default();

    if org_id.is_empty() || repo.is_empty() || branch.is_empty() {
        return json!({ "ok": false, "error": "org_id, repo, and branch are required" }).to_string();
    }
    if workspace.is_empty() {
        return json!({ "ok": false, "error": "No workspace context" }).to_string();
    }

    let orbit_host = api.orbit_url()
        .trim_start_matches("https://")
        .trim_start_matches("http://");
    let remote_url = format!("https://x-token:{jwt}@{orbit_host}/{org_id}/{repo}.git");

    let mut cmd = tokio::process::Command::new("git");
    cmd.arg("push").arg(&remote_url);
    if force {
        cmd.arg("--force");
    }
    cmd.arg(format!("HEAD:refs/heads/{branch}"));
    cmd.current_dir(&workspace);
    cmd.env("GIT_TERMINAL_PROMPT", "0");

    match cmd.output().await {
        Ok(output) => {
            let stdout = String::from_utf8_lossy(&output.stdout);
            let stderr = String::from_utf8_lossy(&output.stderr);
            if output.status.success() {
                json!({ "ok": true, "result": format!("{stdout}{stderr}").trim().to_string() }).to_string()
            } else {
                json!({ "ok": false, "error": format!("git push failed: {stderr}").trim().to_string() }).to_string()
            }
        }
        Err(e) => json!({ "ok": false, "error": format!("Failed to run git push: {e}") }).to_string(),
    }
}
