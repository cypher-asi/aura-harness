//! Linear trusted-integration handlers — `linear_list_teams` and
//! `linear_create_issue`, both wrapped around the bespoke
//! `linear_graphql` helper which adds a Linear-prefixed error on top of
//! the shared [`graphql_user_errors`] check.
//!
//! [`graphql_user_errors`]: super::super::guards::graphql_user_errors

use super::super::super::json_paths::{optional_string, required_string};
use super::super::guards::graphql_user_errors;
use super::super::ToolResolver;
use crate::error::ToolError;
use aura_core::{InstalledToolRuntimeIntegration, InstalledToolRuntimeProviderExecution};
use reqwest::Method;
use serde_json::{json, Value};

impl ToolResolver {
    pub(in super::super) async fn linear_list_teams(
        &self,
        provider: &InstalledToolRuntimeProviderExecution,
        integration: &InstalledToolRuntimeIntegration,
    ) -> Result<Value, ToolError> {
        let response = self
            .linear_graphql(
                provider,
                integration,
                "query AuraLinearTeams { teams { nodes { id name key } } }",
                json!({}),
            )
            .await?;
        let teams = response
            .pointer("/data/teams/nodes")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        Ok(json!({ "teams": teams }))
    }

    pub(in super::super) async fn linear_create_issue(
        &self,
        provider: &InstalledToolRuntimeProviderExecution,
        integration: &InstalledToolRuntimeIntegration,
        args: &Value,
    ) -> Result<Value, ToolError> {
        let team_id = required_string(args, &["team_id", "teamId"])?;
        let title = required_string(args, &["title"])?;
        let description = optional_string(
            args,
            &[
                "description",
                "body",
                "markdown_contents",
                "markdownContents",
            ],
        );
        let response = self
            .linear_graphql(
                provider,
                integration,
                "mutation AuraLinearCreateIssue($input: IssueCreateInput!) { issueCreate(input: $input) { success issue { id identifier title url state { name } team { id name key } } } }",
                json!({
                    "input": {
                        "teamId": team_id,
                        "title": title,
                        "description": description,
                    }
                }),
            )
            .await?;
        Ok(json!({
            "issue": response.pointer("/data/issueCreate/issue").cloned().unwrap_or_else(|| json!({}))
        }))
    }

    /// Linear-specific GraphQL POST: posts to `provider.base_url`, then
    /// surfaces any `errors[]` entries with a `"linear graphql error: "`
    /// prefix. Distinct from the generic `GraphqlErrors` success guard
    /// (which uses a plain `"graphql error: "` prefix); both share the
    /// underlying [`graphql_user_errors`] joiner so the rendered messages
    /// stay byte-identical.
    async fn linear_graphql(
        &self,
        provider: &InstalledToolRuntimeProviderExecution,
        integration: &InstalledToolRuntimeIntegration,
        query: &str,
        variables: Value,
    ) -> Result<Value, ToolError> {
        let response = self
            .provider_json_request(
                Method::POST,
                &provider.base_url,
                provider,
                integration,
                Some(json!({
                    "query": query,
                    "variables": variables,
                })),
            )
            .await?;
        if let Some(message) = graphql_user_errors(&response) {
            return Err(ToolError::ExternalToolError(format!(
                "linear graphql error: {message}"
            )));
        }
        Ok(response)
    }
}
