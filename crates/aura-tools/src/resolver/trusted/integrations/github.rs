//! GitHub trusted-integration handlers — `github_list_repos` and
//! `github_create_issue`. Both go through the canonical
//! [`ToolResolver::provider_json_request`] path.

use super::super::super::json_paths::{optional_string, required_string};
use super::super::ToolResolver;
use crate::error::ToolError;
use aura_core::{InstalledToolRuntimeIntegration, InstalledToolRuntimeProviderExecution};
use reqwest::Method;
use serde_json::{json, Value};

impl ToolResolver {
    pub(in super::super) async fn github_list_repos(
        &self,
        provider: &InstalledToolRuntimeProviderExecution,
        integration: &InstalledToolRuntimeIntegration,
    ) -> Result<Value, ToolError> {
        let url = format!("{}/user/repos?per_page=20&sort=updated", provider.base_url);
        let response = self
            .provider_json_request(Method::GET, &url, provider, integration, None)
            .await?;
        let repos = response
            .as_array()
            .cloned()
            .unwrap_or_default()
            .into_iter()
            .map(|repo| {
                json!({
                    "name": repo.get("name").and_then(Value::as_str).unwrap_or_default(),
                    "full_name": repo.get("full_name").and_then(Value::as_str).unwrap_or_default(),
                    "private": repo.get("private").and_then(Value::as_bool).unwrap_or(false),
                    "html_url": repo.get("html_url").and_then(Value::as_str).unwrap_or_default(),
                    "default_branch": repo.get("default_branch").and_then(Value::as_str).unwrap_or_default(),
                    "description": repo.get("description").and_then(Value::as_str),
                })
            })
            .collect::<Vec<_>>();
        Ok(json!({ "repos": repos }))
    }

    pub(in super::super) async fn github_create_issue(
        &self,
        provider: &InstalledToolRuntimeProviderExecution,
        integration: &InstalledToolRuntimeIntegration,
        args: &Value,
    ) -> Result<Value, ToolError> {
        let owner = required_string(args, &["owner"])?;
        let repo = required_string(args, &["repo"])?;
        let title = required_string(args, &["title"])?;
        let body = optional_string(args, &["body", "markdown_contents", "markdownContents"]);
        let url = format!("{}/repos/{owner}/{repo}/issues", provider.base_url);
        let response = self
            .provider_json_request(
                Method::POST,
                &url,
                provider,
                integration,
                Some(json!({
                    "title": title,
                    "body": body,
                })),
            )
            .await?;
        Ok(json!({
            "issue": {
                "number": response.get("number").and_then(Value::as_u64),
                "title": response.get("title").and_then(Value::as_str).unwrap_or_default(),
                "state": response.get("state").and_then(Value::as_str).unwrap_or_default(),
                "html_url": response.get("html_url").and_then(Value::as_str).unwrap_or_default(),
            }
        }))
    }
}
