//! Trusted-integration runtime execution — GitHub, Linear, Slack,
//! Brave Search, Resend, and the generic `TrustedIntegrationRuntimeSpec`
//! interpreter.
//!
//! Split out of `resolver.rs` in Wave 6 / T4. The bespoke per-integration
//! methods (`github_list_repos`, `linear_create_issue`, etc.) live here
//! alongside the generic metadata-driven path so all "call a trusted
//! provider" logic is in one file.

use super::json_paths::{
    ensure_slack_ok, insert_json_path, optional_json_from_names, optional_json_from_names_map,
    optional_positive_number, optional_positive_number_from_names,
    optional_positive_number_from_names_map, optional_string, optional_string_from_names,
    optional_string_from_names_map, optional_string_list, optional_string_list_from_names,
    optional_string_list_from_names_map, required_string, required_string_list,
};
use super::runtime_headers::{apply_runtime_auth, apply_runtime_headers, runtime_url_with_auth};
use super::{
    ToolResolver, TRUSTED_INTEGRATION_RUNTIME_METADATA_KEY, TRUSTED_PROVIDER_REQUEST_TIMEOUT,
};
use crate::error::ToolError;
use aura_core::{
    InstalledToolDefinition, InstalledToolRuntimeIntegration, InstalledToolRuntimeProviderExecution,
};
use reqwest::{Method, Url};
use serde::Deserialize;
use serde_json::{json, Value};
use std::collections::HashMap;

// ============================================================================
// Trusted integration runtime metadata schema
// ============================================================================

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(super) enum TrustedIntegrationHttpMethod {
    Get,
    Post,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(super) enum TrustedIntegrationArgValueType {
    String,
    StringList,
    PositiveNumber,
    Json,
}

#[derive(Debug, Clone, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub(super) enum TrustedIntegrationArgSource {
    #[default]
    InputArgs,
    ProviderConfig,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct TrustedIntegrationArgBinding {
    pub(super) arg_names: Vec<String>,
    pub(super) target: String,
    #[serde(default)]
    pub(super) source: TrustedIntegrationArgSource,
    pub(super) value_type: TrustedIntegrationArgValueType,
    #[serde(default)]
    pub(super) required: bool,
    #[serde(default)]
    pub(super) default_value: Option<Value>,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(super) enum TrustedIntegrationSuccessGuard {
    #[default]
    None,
    SlackOk,
    GraphqlErrors,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct TrustedIntegrationResultField {
    pub(super) output: String,
    pub(super) pointer: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct TrustedIntegrationResultExtraField {
    pub(super) output: String,
    pub(super) pointer: String,
    #[serde(default)]
    pub(super) default_value: Option<Value>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub(super) enum TrustedIntegrationResultTransform {
    WrapPointer {
        key: String,
        pointer: String,
    },
    ProjectArray {
        key: String,
        #[serde(default)]
        pointer: Option<String>,
        fields: Vec<TrustedIntegrationResultField>,
        #[serde(default)]
        extras: Vec<TrustedIntegrationResultExtraField>,
    },
    ProjectObject {
        key: String,
        #[serde(default)]
        pointer: Option<String>,
        fields: Vec<TrustedIntegrationResultField>,
    },
    BraveSearch {
        vertical: String,
    },
}

#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub(super) enum TrustedIntegrationRuntimeSpec {
    RestJson {
        method: TrustedIntegrationHttpMethod,
        path: String,
        #[serde(default)]
        query: Vec<TrustedIntegrationArgBinding>,
        #[serde(default)]
        body: Vec<TrustedIntegrationArgBinding>,
        #[serde(default)]
        success_guard: TrustedIntegrationSuccessGuard,
        result: TrustedIntegrationResultTransform,
    },
    RestForm {
        method: TrustedIntegrationHttpMethod,
        path: String,
        #[serde(default)]
        query: Vec<TrustedIntegrationArgBinding>,
        #[serde(default)]
        body: Vec<TrustedIntegrationArgBinding>,
        #[serde(default)]
        success_guard: TrustedIntegrationSuccessGuard,
        result: TrustedIntegrationResultTransform,
    },
    Graphql {
        query: String,
        #[serde(default)]
        variables: Vec<TrustedIntegrationArgBinding>,
        #[serde(default)]
        success_guard: TrustedIntegrationSuccessGuard,
        result: TrustedIntegrationResultTransform,
    },
    BraveSearch {
        vertical: String,
    },
    ResendSendEmail,
}

// ============================================================================
// Impl ToolResolver — trusted runtime dispatch
// ============================================================================

impl ToolResolver {
    pub(super) async fn execute_trusted_runtime_app_provider(
        &self,
        provider: &InstalledToolRuntimeProviderExecution,
        integration: &InstalledToolRuntimeIntegration,
        args: &Value,
        spec: &TrustedIntegrationRuntimeSpec,
    ) -> Result<Value, ToolError> {
        match spec {
            TrustedIntegrationRuntimeSpec::RestJson {
                method,
                path,
                query,
                body,
                success_guard,
                result,
            } => {
                let url = build_runtime_url(provider, integration, path, query, args)?;
                let body = build_object_from_bindings(body, args, &integration.provider_config)?;
                let response = self
                    .provider_json_request(
                        trusted_http_method(method),
                        &url,
                        provider,
                        integration,
                        body,
                    )
                    .await?;
                apply_success_guard(&response, success_guard)?;
                apply_result_transform(&response, result, args)
            }
            TrustedIntegrationRuntimeSpec::Graphql {
                query,
                variables,
                success_guard,
                result,
            } => {
                let variables =
                    build_object_from_bindings(variables, args, &integration.provider_config)?
                        .unwrap_or_else(|| json!({}));
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
                apply_success_guard(&response, success_guard)?;
                apply_result_transform(&response, result, args)
            }
            TrustedIntegrationRuntimeSpec::RestForm {
                method,
                path,
                query,
                body,
                success_guard,
                result,
            } => {
                let url = build_runtime_url(provider, integration, path, query, args)?;
                let body =
                    build_form_fields_from_bindings(body, args, &integration.provider_config)?;
                let response = self
                    .provider_form_request(
                        trusted_http_method(method),
                        &url,
                        provider,
                        integration,
                        body,
                    )
                    .await?;
                apply_success_guard(&response, success_guard)?;
                apply_result_transform(&response, result, args)
            }
            TrustedIntegrationRuntimeSpec::BraveSearch { vertical } => {
                self.brave_search(provider, integration, args, vertical)
                    .await
            }
            TrustedIntegrationRuntimeSpec::ResendSendEmail => {
                self.resend_send_email(provider, integration, args).await
            }
        }
    }

    pub(super) async fn execute_runtime_app_provider(
        &self,
        tool: &InstalledToolDefinition,
        args: &Value,
        provider: &InstalledToolRuntimeProviderExecution,
    ) -> Result<Value, ToolError> {
        let integration = select_runtime_integration(provider, args)?;
        match tool.name.as_str() {
            "github_list_repos" => self.github_list_repos(provider, integration).await,
            "github_create_issue" => self.github_create_issue(provider, integration, args).await,
            "linear_list_teams" => self.linear_list_teams(provider, integration).await,
            "linear_create_issue" => self.linear_create_issue(provider, integration, args).await,
            "slack_list_channels" => self.slack_list_channels(provider, integration).await,
            "slack_post_message" => self.slack_post_message(provider, integration, args).await,
            "brave_search_web" => self.brave_search(provider, integration, args, "web").await,
            "brave_search_news" => self.brave_search(provider, integration, args, "news").await,
            "resend_list_domains" => self.resend_list_domains(provider, integration).await,
            "resend_send_email" => self.resend_send_email(provider, integration, args).await,
            other => Err(ToolError::ExternalToolError(format!(
                "runtime execution is not implemented for installed tool `{other}`"
            ))),
        }
    }

    async fn github_list_repos(
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

    async fn github_create_issue(
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

    async fn linear_list_teams(
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

    async fn linear_create_issue(
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

    async fn brave_search(
        &self,
        provider: &InstalledToolRuntimeProviderExecution,
        integration: &InstalledToolRuntimeIntegration,
        args: &Value,
        vertical: &str,
    ) -> Result<Value, ToolError> {
        let query = required_string(args, &["query", "q"])?;
        let mut url = Url::parse(&format!("{}/res/v1/{vertical}/search", provider.base_url))
            .map_err(|e| ToolError::ExternalToolError(format!("invalid brave base url: {e}")))?;
        {
            let mut params = url.query_pairs_mut();
            params.append_pair("q", &query);
            params.append_pair(
                "count",
                &optional_positive_number(args, &["count"])
                    .unwrap_or(10)
                    .to_string(),
            );
            if let Some(freshness) = optional_string(args, &["freshness"]) {
                params.append_pair("freshness", &freshness);
            }
            if let Some(country) = optional_string(args, &["country"]) {
                params.append_pair("country", &country);
            }
            if let Some(search_lang) = optional_string(args, &["search_lang", "searchLang"]) {
                params.append_pair("search_lang", &search_lang);
            }
        }
        let response = self
            .provider_json_request(Method::GET, url.as_str(), provider, integration, None)
            .await?;
        brave_search_results(&response, vertical, args)
    }

    async fn slack_list_channels(
        &self,
        provider: &InstalledToolRuntimeProviderExecution,
        integration: &InstalledToolRuntimeIntegration,
    ) -> Result<Value, ToolError> {
        let url = format!(
            "{}/conversations.list?types=public_channel,private_channel&exclude_archived=true&limit=100",
            provider.base_url
        );
        let response = self
            .provider_json_request(Method::GET, &url, provider, integration, None)
            .await?;
        ensure_slack_ok(&response)?;
        let channels = response
            .get("channels")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default()
            .into_iter()
            .map(|channel| {
                json!({
                    "id": channel.get("id").and_then(Value::as_str).unwrap_or_default(),
                    "name": channel.get("name").and_then(Value::as_str).unwrap_or_default(),
                    "is_private": channel.get("is_private").and_then(Value::as_bool).unwrap_or(false),
                })
            })
            .collect::<Vec<_>>();
        Ok(json!({ "channels": channels }))
    }

    async fn slack_post_message(
        &self,
        provider: &InstalledToolRuntimeProviderExecution,
        integration: &InstalledToolRuntimeIntegration,
        args: &Value,
    ) -> Result<Value, ToolError> {
        let channel_id = required_string(args, &["channel_id", "channelId"])?;
        let text = required_string(args, &["text", "message"])?;
        let response = self
            .provider_json_request(
                Method::POST,
                &format!("{}/chat.postMessage", provider.base_url),
                provider,
                integration,
                Some(json!({
                    "channel": channel_id,
                    "text": text,
                })),
            )
            .await?;
        ensure_slack_ok(&response)?;
        Ok(json!({
            "message": {
                "channel": response.get("channel").and_then(Value::as_str).unwrap_or_default(),
                "ts": response.get("ts").and_then(Value::as_str).unwrap_or_default(),
            }
        }))
    }

    async fn resend_list_domains(
        &self,
        provider: &InstalledToolRuntimeProviderExecution,
        integration: &InstalledToolRuntimeIntegration,
    ) -> Result<Value, ToolError> {
        let response = self
            .provider_json_request(
                Method::GET,
                &format!("{}/domains", provider.base_url),
                provider,
                integration,
                None,
            )
            .await?;
        let domains = response
            .get("data")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default()
            .into_iter()
            .map(|domain| {
                json!({
                    "id": domain.get("id").and_then(Value::as_str).unwrap_or_default(),
                    "name": domain.get("name").and_then(Value::as_str).unwrap_or_default(),
                    "status": domain.get("status").and_then(Value::as_str).unwrap_or_default(),
                    "created_at": domain.get("created_at").and_then(Value::as_str),
                    "region": domain.get("region").and_then(Value::as_str),
                    "capabilities": domain.get("capabilities").cloned().unwrap_or_else(|| json!({})),
                })
            })
            .collect::<Vec<_>>();
        Ok(json!({
            "domains": domains,
            "has_more": response.get("has_more").and_then(Value::as_bool).unwrap_or(false),
        }))
    }

    async fn resend_send_email(
        &self,
        provider: &InstalledToolRuntimeProviderExecution,
        integration: &InstalledToolRuntimeIntegration,
        args: &Value,
    ) -> Result<Value, ToolError> {
        let from = required_string(args, &["from"])?;
        let to = required_string_list(args, &["to"])?;
        let subject = required_string(args, &["subject"])?;
        let html = optional_string(args, &["html"]);
        let text = optional_string(args, &["text"]);
        let cc = optional_string_list(args, &["cc"]);
        let bcc = optional_string_list(args, &["bcc"]);

        if html.is_none() && text.is_none() {
            return Err(ToolError::ExternalToolError(
                "resend_send_email requires at least one of `html` or `text`".into(),
            ));
        }

        let mut payload = json!({
            "from": from,
            "to": to,
            "subject": subject,
        });
        if let Some(html) = html {
            payload["html"] = Value::String(html);
        }
        if let Some(text) = text {
            payload["text"] = Value::String(text);
        }
        if let Some(cc) = cc {
            payload["cc"] = json!(cc);
        }
        if let Some(bcc) = bcc {
            payload["bcc"] = json!(bcc);
        }

        let response = self
            .provider_json_request(
                Method::POST,
                &format!("{}/emails", provider.base_url),
                provider,
                integration,
                Some(payload),
            )
            .await?;
        Ok(json!({
            "email": {
                "id": response.get("id").and_then(Value::as_str).unwrap_or_default(),
            }
        }))
    }

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

    pub(super) async fn provider_json_request(
        &self,
        method: Method,
        url: &str,
        provider: &InstalledToolRuntimeProviderExecution,
        integration: &InstalledToolRuntimeIntegration,
        body: Option<Value>,
    ) -> Result<Value, ToolError> {
        self.send_provider_request(
            method,
            url,
            provider,
            integration,
            body.map_or(ProviderRequestBody::None, ProviderRequestBody::Json),
        )
        .await
    }

    async fn provider_form_request(
        &self,
        method: Method,
        url: &str,
        provider: &InstalledToolRuntimeProviderExecution,
        integration: &InstalledToolRuntimeIntegration,
        body: Vec<(String, String)>,
    ) -> Result<Value, ToolError> {
        self.send_provider_request(
            method,
            url,
            provider,
            integration,
            ProviderRequestBody::Form(body),
        )
        .await
    }

    /// Unified trusted-provider HTTP send.
    ///
    /// Builds the URL, applies runtime headers + auth, attaches the
    /// requested body (JSON, form-encoded, or none), and decodes the
    /// response as JSON. Identical timeout/error handling for both
    /// JSON and form bodies — extracted from `provider_json_request`
    /// and `provider_form_request` (Phase 1 dedup).
    async fn send_provider_request(
        &self,
        method: Method,
        url: &str,
        provider: &InstalledToolRuntimeProviderExecution,
        integration: &InstalledToolRuntimeIntegration,
        body: ProviderRequestBody,
    ) -> Result<Value, ToolError> {
        let final_url = runtime_url_with_auth(url, integration)?;
        let mut request = self
            .http_client
            .request(method, final_url)
            // Per-request ceiling on trusted-provider calls so a slow
            // integration can't stall the whole turn. (Wave 5 / T2.3.)
            .timeout(TRUSTED_PROVIDER_REQUEST_TIMEOUT);
        request = apply_runtime_headers(request, &provider.static_headers)?;
        request = apply_runtime_auth(request, integration);
        request = match body {
            ProviderRequestBody::None => request,
            ProviderRequestBody::Json(value) => request.json(&value),
            ProviderRequestBody::Form(fields) => request.form(&fields),
        };
        let response = request
            .send()
            .await
            .map_err(|e| ToolError::ExternalToolError(format!("provider request failed: {e}")))?;
        let status = response.status();
        let text = response.text().await.map_err(|e| {
            ToolError::ExternalToolError(format!("reading provider response failed: {e}"))
        })?;
        if !status.is_success() {
            return Err(ToolError::ExternalToolError(format!(
                "provider request failed with {}: {}",
                status, text
            )));
        }
        serde_json::from_str(&text).map_err(|e| {
            ToolError::ExternalToolError(format!("provider returned invalid JSON: {e}"))
        })
    }
}

/// Body variants accepted by [`TrustedIntegrationResolver::send_provider_request`].
///
/// Collapses what used to be two parallel helpers (`provider_json_request`
/// / `provider_form_request`) into one — the body shape is the only thing
/// that differs between them.
enum ProviderRequestBody {
    None,
    Json(Value),
    Form(Vec<(String, String)>),
}

// ============================================================================
// Free helpers — runtime spec parsing, binding resolution, transforms
// ============================================================================

pub(super) fn trusted_runtime_spec(
    tool: &InstalledToolDefinition,
) -> Result<Option<TrustedIntegrationRuntimeSpec>, ToolError> {
    let Some(raw) = tool.metadata.get(TRUSTED_INTEGRATION_RUNTIME_METADATA_KEY) else {
        return Ok(None);
    };
    serde_json::from_value(raw.clone()).map(Some).map_err(|e| {
        ToolError::ExternalToolError(format!(
            "invalid trusted integration runtime metadata for `{}`: {e}",
            tool.name
        ))
    })
}

fn trusted_http_method(method: &TrustedIntegrationHttpMethod) -> Method {
    match method {
        TrustedIntegrationHttpMethod::Get => Method::GET,
        TrustedIntegrationHttpMethod::Post => Method::POST,
    }
}

fn build_runtime_url(
    provider: &InstalledToolRuntimeProviderExecution,
    integration: &InstalledToolRuntimeIntegration,
    path: &str,
    query_bindings: &[TrustedIntegrationArgBinding],
    args: &Value,
) -> Result<String, ToolError> {
    let expanded_path = expand_path_template(path, args)?;
    let resolved_base_url = integration
        .base_url
        .as_deref()
        .unwrap_or(&provider.base_url);
    let base = format!(
        "{}{}",
        resolved_base_url.trim_end_matches('/'),
        expanded_path
    );
    let mut url = Url::parse(&base)
        .map_err(|e| ToolError::ExternalToolError(format!("invalid trusted runtime url: {e}")))?;
    for binding in query_bindings {
        if let Some(value) = resolve_binding_value(args, &integration.provider_config, binding)? {
            append_query_value(&mut url, &binding.target, value);
        }
    }
    Ok(url.to_string())
}

fn expand_path_template(path: &str, args: &Value) -> Result<String, ToolError> {
    let mut expanded = String::new();
    let mut chars = path.chars().peekable();
    while let Some(ch) = chars.next() {
        if ch == '{' {
            let mut key = String::new();
            for next in chars.by_ref() {
                if next == '}' {
                    break;
                }
                key.push(next);
            }
            let value = required_string(args, &[key.as_str()])?;
            expanded.push_str(&value);
        } else {
            expanded.push(ch);
        }
    }
    Ok(expanded)
}

fn append_query_value(url: &mut Url, key: &str, value: Value) {
    let mut pairs = url.query_pairs_mut();
    match value {
        Value::Array(items) => {
            for item in items {
                if let Some(value) = item.as_str() {
                    pairs.append_pair(key, value);
                } else {
                    pairs.append_pair(key, &item.to_string());
                }
            }
        }
        Value::String(value) => {
            pairs.append_pair(key, &value);
        }
        other => {
            pairs.append_pair(key, &other.to_string());
        }
    }
}

fn build_object_from_bindings(
    bindings: &[TrustedIntegrationArgBinding],
    args: &Value,
    provider_config: &HashMap<String, Value>,
) -> Result<Option<Value>, ToolError> {
    if bindings.is_empty() {
        return Ok(None);
    }

    if bindings.len() == 1 && bindings[0].target == "$" {
        return resolve_binding_value(args, provider_config, &bindings[0]);
    }

    let mut body = json!({});
    let mut inserted = false;
    for binding in bindings {
        if binding.target == "$" {
            return Err(ToolError::ExternalToolError(
                "trusted integration metadata cannot mix root body bindings with object bindings"
                    .into(),
            ));
        }
        if let Some(value) = resolve_binding_value(args, provider_config, binding)? {
            insert_json_path(&mut body, &binding.target, value)?;
            inserted = true;
        }
    }
    Ok(inserted.then_some(body))
}

fn build_form_fields_from_bindings(
    bindings: &[TrustedIntegrationArgBinding],
    args: &Value,
    provider_config: &HashMap<String, Value>,
) -> Result<Vec<(String, String)>, ToolError> {
    let mut fields = Vec::new();
    for binding in bindings {
        if let Some(value) = resolve_binding_value(args, provider_config, binding)? {
            match value {
                Value::Array(items) => {
                    for item in items {
                        fields.push((binding.target.clone(), form_field_value(item)));
                    }
                }
                other => fields.push((binding.target.clone(), form_field_value(other))),
            }
        }
    }
    Ok(fields)
}

fn form_field_value(value: Value) -> String {
    match value {
        Value::String(value) => value,
        other => other.to_string(),
    }
}

fn resolve_binding_value(
    args: &Value,
    provider_config: &HashMap<String, Value>,
    binding: &TrustedIntegrationArgBinding,
) -> Result<Option<Value>, ToolError> {
    if binding.arg_names.is_empty() {
        return Ok(binding.default_value.clone());
    }

    let resolved = match binding.source {
        TrustedIntegrationArgSource::InputArgs => match binding.value_type {
            TrustedIntegrationArgValueType::String => {
                optional_string_from_names(args, &binding.arg_names).map(Value::String)
            }
            TrustedIntegrationArgValueType::StringList => {
                optional_string_list_from_names(args, &binding.arg_names).map(|items| json!(items))
            }
            TrustedIntegrationArgValueType::PositiveNumber => {
                optional_positive_number_from_names(args, &binding.arg_names)
                    .map(|value| json!(value))
            }
            TrustedIntegrationArgValueType::Json => {
                optional_json_from_names(args, &binding.arg_names)
            }
        },
        TrustedIntegrationArgSource::ProviderConfig => match binding.value_type {
            TrustedIntegrationArgValueType::String => {
                optional_string_from_names_map(provider_config, &binding.arg_names)
                    .map(Value::String)
            }
            TrustedIntegrationArgValueType::StringList => {
                optional_string_list_from_names_map(provider_config, &binding.arg_names)
                    .map(|items| json!(items))
            }
            TrustedIntegrationArgValueType::PositiveNumber => {
                optional_positive_number_from_names_map(provider_config, &binding.arg_names)
                    .map(|value| json!(value))
            }
            TrustedIntegrationArgValueType::Json => {
                optional_json_from_names_map(provider_config, &binding.arg_names)
            }
        },
    };

    if let Some(value) = resolved {
        return Ok(Some(value));
    }
    if let Some(default) = &binding.default_value {
        return Ok(Some(default.clone()));
    }
    if binding.required {
        return Err(ToolError::ExternalToolError(format!(
            "missing required field `{}`",
            binding.arg_names.first().map_or("", String::as_str)
        )));
    }
    Ok(None)
}

fn apply_success_guard(
    response: &Value,
    guard: &TrustedIntegrationSuccessGuard,
) -> Result<(), ToolError> {
    match guard {
        TrustedIntegrationSuccessGuard::None => Ok(()),
        TrustedIntegrationSuccessGuard::SlackOk => ensure_slack_ok(response),
        TrustedIntegrationSuccessGuard::GraphqlErrors => match graphql_user_errors(response) {
            Some(message) => Err(ToolError::ExternalToolError(format!(
                "graphql error: {message}"
            ))),
            None => Ok(()),
        },
    }
}

/// Extract a GraphQL `errors[].message` summary from `response`.
///
/// Returns `Some(joined)` when the response contains a non-empty
/// `errors` array; `None` otherwise. Messages are joined with `"; "`,
/// matching the legacy `linear_graphql` and `apply_success_guard`
/// implementations bit-for-bit. Each caller adds its own prefix
/// (`"linear graphql error: "` / `"graphql error: "`) so this helper
/// stays prefix-agnostic.
fn graphql_user_errors(response: &Value) -> Option<String> {
    let errors = response.get("errors").and_then(Value::as_array)?;
    if errors.is_empty() {
        return None;
    }
    Some(
        errors
            .iter()
            .filter_map(|error| error.get("message").and_then(Value::as_str))
            .collect::<Vec<_>>()
            .join("; "),
    )
}

fn apply_result_transform(
    response: &Value,
    transform: &TrustedIntegrationResultTransform,
    args: &Value,
) -> Result<Value, ToolError> {
    match transform {
        TrustedIntegrationResultTransform::WrapPointer { key, pointer } => Ok(object_with_entry(
            key,
            response
                .pointer(pointer)
                .cloned()
                .unwrap_or_else(|| json!({})),
        )),
        TrustedIntegrationResultTransform::ProjectArray {
            key,
            pointer,
            fields,
            extras,
        } => {
            let source = pointer
                .as_deref()
                .map_or(Some(response), |path| response.pointer(path));
            let items = source
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_default()
                .into_iter()
                .map(|item| project_fields(&item, fields))
                .collect::<Vec<_>>();
            let mut result = object_with_entry(key, Value::Array(items));
            for extra in extras {
                let value = response
                    .pointer(&extra.pointer)
                    .cloned()
                    .or_else(|| extra.default_value.clone())
                    .unwrap_or(Value::Null);
                result[&extra.output] = value;
            }
            Ok(result)
        }
        TrustedIntegrationResultTransform::ProjectObject {
            key,
            pointer,
            fields,
        } => {
            let source = pointer
                .as_deref()
                .map_or(Some(response), |path| response.pointer(path))
                .cloned()
                .unwrap_or_else(|| json!({}));
            Ok(object_with_entry(key, project_fields(&source, fields)))
        }
        TrustedIntegrationResultTransform::BraveSearch { vertical } => {
            brave_search_results(response, vertical, args)
        }
    }
}

/// Shape Brave Search responses into the canonical
/// `{ query, results: [...], more_results_available }` envelope.
///
/// Used both by the in-line `brave_search` runtime path and by the
/// declarative `apply_result_transform::BraveSearch` transform — keeping
/// one implementation guarantees both code paths produce bit-identical
/// output for the same Brave response.
fn brave_search_results(
    response: &Value,
    vertical: &str,
    args: &Value,
) -> Result<Value, ToolError> {
    let query = required_string(args, &["query", "q"])?;
    let items = response
        .pointer(&format!("/{vertical}/results"))
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
        .into_iter()
        .map(|item| {
            json!({
                "title": item.get("title").and_then(Value::as_str).unwrap_or_default(),
                "url": item
                    .get("url")
                    .or_else(|| item.get("profile"))
                    .and_then(Value::as_str)
                    .unwrap_or_default(),
                "description": item
                    .get("description")
                    .or_else(|| item.get("snippet"))
                    .and_then(Value::as_str),
                "age": item.get("age").and_then(Value::as_str),
                "source": item.get("source").and_then(Value::as_str),
            })
        })
        .collect::<Vec<_>>();
    Ok(json!({
        "query": query,
        "results": items,
        "more_results_available": response
            .pointer("/query/more_results_available")
            .and_then(Value::as_bool)
            .unwrap_or(false),
    }))
}

fn object_with_entry(key: &str, value: Value) -> Value {
    let mut map = serde_json::Map::new();
    map.insert(key.to_string(), value);
    Value::Object(map)
}

fn project_fields(source: &Value, fields: &[TrustedIntegrationResultField]) -> Value {
    let mut result = json!({});
    for field in fields {
        result[&field.output] = source
            .pointer(&field.pointer)
            .cloned()
            .unwrap_or(Value::Null);
    }
    result
}

pub(super) fn select_runtime_integration<'a>(
    provider: &'a InstalledToolRuntimeProviderExecution,
    args: &Value,
) -> Result<&'a InstalledToolRuntimeIntegration, ToolError> {
    let requested = optional_string(args, &["integration_id", "integrationId"]);
    if let Some(requested) = requested {
        return provider
            .integrations
            .iter()
            .find(|integration| integration.integration_id == requested)
            .ok_or_else(|| {
                ToolError::ExternalToolError(format!(
                    "requested integration `{requested}` is not installed for runtime execution"
                ))
            });
    }
    provider.integrations.first().ok_or_else(|| {
        ToolError::ExternalToolError("no runtime integration credentials are available".into())
    })
}
