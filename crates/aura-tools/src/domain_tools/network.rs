//! Network domain tool handlers.

use serde_json::{json, Value};
use tracing::debug;

use super::api::DomainApi;
use super::helpers::{domain_err, domain_ok, str_field};

fn parse_api_result(body: &str) -> Value {
    serde_json::from_str::<Value>(body).unwrap_or(Value::String(body.to_owned()))
}

pub async fn post_to_feed(api: &dyn DomainApi, _project_id: &str, input: &Value) -> String {
    debug!("domain_tools: post_to_feed");
    let body = json!({
        "profileId": input["profile_id"].as_str().unwrap_or_default(),
        "title": input["title"].as_str().unwrap_or_default(),
        "summary": input["summary"].as_str(),
        "postType": input["post_type"].as_str().unwrap_or("post"),
        "agentId": input["agent_id"].as_str(),
        "userId": input["user_id"].as_str(),
        "metadata": input["metadata"],
    });
    let jwt = str_field(input, "jwt");
    match api
        .network_api_call("POST", "/api/posts", Some(&body), jwt.as_deref())
        .await
    {
        Ok(r) => domain_ok(json!({ "result": parse_api_result(&r) })),
        Err(e) => domain_err(e),
    }
}

pub async fn network_list_projects(
    api: &dyn DomainApi,
    _project_id: &str,
    input: &Value,
) -> String {
    debug!("domain_tools: network_list_projects");
    let org_id = input["org_id"].as_str().unwrap_or_default();
    let jwt = str_field(input, "jwt").unwrap_or_default();
    let path = format!("/api/projects?org_id={org_id}");
    match api.network_api_call("GET", &path, None, Some(&jwt)).await {
        Ok(r) => domain_ok(json!({ "result": parse_api_result(&r) })),
        Err(e) => domain_err(e),
    }
}

pub async fn network_get_project(api: &dyn DomainApi, _project_id: &str, input: &Value) -> String {
    debug!("domain_tools: network_get_project");
    let project_id = input["project_id"].as_str().unwrap_or_default();
    let jwt = str_field(input, "jwt").unwrap_or_default();
    let path = format!("/api/projects/{project_id}");
    match api.network_api_call("GET", &path, None, Some(&jwt)).await {
        Ok(r) => domain_ok(json!({ "result": parse_api_result(&r) })),
        Err(e) => domain_err(e),
    }
}

pub async fn check_budget(api: &dyn DomainApi, _project_id: &str, input: &Value) -> String {
    debug!("domain_tools: check_budget");
    let org_id = input["org_id"].as_str().unwrap_or_default();
    let jwt = str_field(input, "jwt");
    let path = format!("/api/orgs/{org_id}/budget");
    match api
        .network_api_call("GET", &path, None, jwt.as_deref())
        .await
    {
        Ok(r) => domain_ok(json!({ "result": parse_api_result(&r) })),
        Err(e) => domain_err(e),
    }
}

pub async fn record_usage(api: &dyn DomainApi, _project_id: &str, input: &Value) -> String {
    debug!("domain_tools: record_usage");
    let body = json!({
        "orgId": input["org_id"].as_str().unwrap_or_default(),
        "userId": input["user_id"].as_str().unwrap_or_default(),
        "inputTokens": input["input_tokens"].as_u64().unwrap_or(0),
        "outputTokens": input["output_tokens"].as_u64().unwrap_or(0),
        "agentId": input["agent_id"].as_str(),
        "model": input["model"].as_str(),
    });
    let jwt = str_field(input, "jwt");
    match api
        .network_api_call("POST", "/api/usage", Some(&body), jwt.as_deref())
        .await
    {
        Ok(r) => domain_ok(json!({ "result": parse_api_result(&r) })),
        Err(e) => domain_err(e),
    }
}
