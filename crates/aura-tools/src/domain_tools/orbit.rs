//! Orbit domain tool handlers.

use serde_json::{json, Value};
use tracing::debug;

use super::api::DomainApi;
use super::helpers::{domain_err, domain_ok, str_field};

fn parse_orbit_result(body: &str) -> Value {
    serde_json::from_str::<Value>(body).unwrap_or_else(|_| Value::String(body.to_owned()))
}

pub async fn orbit_list_repos(api: &dyn DomainApi, _project_id: &str, input: &Value) -> String {
    debug!("domain_tools: orbit_list_repos");
    let jwt = str_field(input, "jwt").unwrap_or_default();
    match api.orbit_api_call("GET", "/repos", None, Some(&jwt)).await {
        Ok(body) => domain_ok(json!({ "result": parse_orbit_result(&body) })),
        Err(e) => domain_err(e),
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
    match api
        .orbit_api_call("POST", "/api/repos", Some(&body), jwt.as_deref())
        .await
    {
        Ok(r) => domain_ok(json!({ "result": parse_orbit_result(&r) })),
        Err(e) => domain_err(e),
    }
}

pub async fn orbit_list_branches(api: &dyn DomainApi, _project_id: &str, input: &Value) -> String {
    debug!("domain_tools: orbit_list_branches");
    let org_id = input["org_id"].as_str().unwrap_or_default();
    let repo = input["repo"].as_str().unwrap_or_default();
    let jwt = str_field(input, "jwt").unwrap_or_default();
    let path = format!("/repos/{org_id}/{repo}/branches");
    match api.orbit_api_call("GET", &path, None, Some(&jwt)).await {
        Ok(r) => domain_ok(json!({ "result": parse_orbit_result(&r) })),
        Err(e) => domain_err(e),
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
    match api
        .orbit_api_call("POST", &path, Some(&body), Some(&jwt))
        .await
    {
        Ok(r) => domain_ok(json!({ "result": parse_orbit_result(&r) })),
        Err(e) => domain_err(e),
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
        use std::fmt::Write;
        let _ = write!(path, "&ref={git_ref}");
    }
    match api.orbit_api_call("GET", &path, None, Some(&jwt)).await {
        Ok(r) => domain_ok(json!({ "result": parse_orbit_result(&r) })),
        Err(e) => domain_err(e),
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
        Ok(r) => domain_ok(json!({ "result": parse_orbit_result(&r) })),
        Err(e) => domain_err(e),
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
    match api
        .orbit_api_call("POST", &path, Some(&body), Some(&jwt))
        .await
    {
        Ok(r) => domain_ok(json!({ "result": parse_orbit_result(&r) })),
        Err(e) => domain_err(e),
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
        use std::fmt::Write;
        let _ = write!(path, "?status={status}");
    }
    match api.orbit_api_call("GET", &path, None, Some(&jwt)).await {
        Ok(r) => domain_ok(json!({ "result": parse_orbit_result(&r) })),
        Err(e) => domain_err(e),
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
    match api
        .orbit_api_call("POST", &path, Some(&body), Some(&jwt))
        .await
    {
        Ok(r) => domain_ok(json!({ "result": parse_orbit_result(&r) })),
        Err(e) => domain_err(e),
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
        return domain_err("org_id, repo, and branch are required");
    }
    if workspace.is_empty() {
        return domain_err("No workspace context");
    }

    let orbit_host = api
        .orbit_url()
        .trim_start_matches("https://")
        .trim_start_matches("http://");
    // `orbit_push` is itself a domain tool that runs inside the
    // kernel's executor pipeline (policy + recording already applied
    // at the outer `ToolCall` boundary). Re-entering the full tool
    // executor just to run `git push` would double-record the
    // mutation and risk recursive policy gating. Instead we call the
    // shared `git_push_impl` helper in `aura-tools/src/git_tool/` —
    // the same code path the dedicated `git_push` tool uses — so the
    // single git spawn call-site remains centralized in `git_tool`
    // (see `docs/invariants.md` §1).
    let remote_url = format!("https://{orbit_host}/{org_id}/{repo}.git");
    let workspace_path = std::path::Path::new(&workspace);
    let timeout = std::time::Duration::from_secs(120);

    match crate::git_tool::git_push_impl(workspace_path, &remote_url, branch, &jwt, force, timeout)
        .await
    {
        Ok(commits) => domain_ok(json!({
            "result": format!("pushed {} commit(s) to {branch}", commits.len()),
            "commits": commits
                .iter()
                .map(|c| json!({ "sha": c.sha, "message": c.message }))
                .collect::<Vec<_>>(),
        })),
        Err(e) => domain_err(format!("git push failed: {e}")),
    }
}
