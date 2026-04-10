//! Tool resolver — unified dispatch layer for tool execution.
//!
//! The resolver adds catalog-based visibility and domain tool dispatch on top
//! of [`ToolExecutor`](crate::ToolExecutor), which owns the internal built-in
//! tool implementations and permission checks.

use crate::catalog::ToolCatalog;
use crate::catalog::ToolProfile;
use crate::domain_tools::DomainToolExecutor;
use crate::error::ToolError;
use crate::tool::Tool;
use crate::ToolConfig;
use crate::ToolExecutor;
use async_trait::async_trait;
use aura_core::ToolDefinition;
use aura_core::{
    Action, ActionKind, Effect, EffectKind, EffectStatus, InstalledToolDefinition,
    InstalledToolRuntimeAuth, InstalledToolRuntimeExecution, InstalledToolRuntimeIntegration,
    InstalledToolRuntimeProviderExecution, ToolAuth, ToolCall, ToolResult,
};
use aura_kernel::{ExecuteContext, Executor, ExecutorError};
use bytes::Bytes;
use reqwest::header::{
    HeaderMap, HeaderName, HeaderValue, ACCEPT, AUTHORIZATION, CONTENT_TYPE, USER_AGENT,
};
use reqwest::{Client, Method, RequestBuilder, Url};
use serde::Deserialize;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::sync::Arc;
use tracing::{debug, error, instrument};

const TRUSTED_INTEGRATION_RUNTIME_METADATA_KEY: &str = "trusted_integration_runtime";

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "snake_case")]
enum TrustedIntegrationHttpMethod {
    Get,
    Post,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "snake_case")]
enum TrustedIntegrationArgValueType {
    String,
    StringList,
    PositiveNumber,
    Json,
}

#[derive(Debug, Clone, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
enum TrustedIntegrationArgSource {
    #[default]
    InputArgs,
    ProviderConfig,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
struct TrustedIntegrationArgBinding {
    arg_names: Vec<String>,
    target: String,
    #[serde(default)]
    source: TrustedIntegrationArgSource,
    value_type: TrustedIntegrationArgValueType,
    #[serde(default)]
    required: bool,
    #[serde(default)]
    default_value: Option<Value>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "snake_case")]
enum TrustedIntegrationSuccessGuard {
    None,
    SlackOk,
    GraphqlErrors,
}

impl Default for TrustedIntegrationSuccessGuard {
    fn default() -> Self {
        Self::None
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
struct TrustedIntegrationResultField {
    output: String,
    pointer: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
struct TrustedIntegrationResultExtraField {
    output: String,
    pointer: String,
    #[serde(default)]
    default_value: Option<Value>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum TrustedIntegrationResultTransform {
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
enum TrustedIntegrationRuntimeSpec {
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

/// Unified tool resolver providing both visibility and execution dispatch.
///
/// Composes [`ToolExecutor`](crate::ToolExecutor) for built-in tool execution
/// and adds domain tool routing (specs, tasks, project) on top.
///
/// Implements [`Executor`] so it can be plugged into the kernel layer
/// (scheduler, `ExecutorRouter`) as a drop-in replacement for `ToolExecutor`.
pub struct ToolResolver {
    catalog: Arc<ToolCatalog>,
    inner: ToolExecutor,
    domain_executor: Option<Arc<DomainToolExecutor>>,
    installed_tools: HashMap<String, InstalledToolDefinition>,
    http_client: Client,
}

impl ToolResolver {
    /// Create a resolver pre-loaded with all built-in tool handlers.
    #[must_use]
    pub fn new(catalog: Arc<ToolCatalog>, config: ToolConfig) -> Self {
        Self {
            catalog,
            inner: ToolExecutor::new(config),
            domain_executor: None,
            installed_tools: HashMap::new(),
            http_client: Client::new(),
        }
    }

    /// Attach a domain tool executor for specs/tasks/project dispatch.
    #[must_use]
    pub fn with_domain_executor(mut self, exec: Arc<DomainToolExecutor>) -> Self {
        self.domain_executor = Some(exec);
        self
    }

    /// Attach installed tools that should execute via HTTP callbacks.
    #[must_use]
    pub fn with_installed_tools(mut self, tools: Vec<InstalledToolDefinition>) -> Self {
        self.installed_tools = tools
            .into_iter()
            .map(|tool| (tool.name.clone(), tool))
            .collect();
        self
    }

    /// Visible tools for a profile (delegates to the catalog + config).
    #[must_use]
    pub fn visible_tools(&self, profile: ToolProfile) -> Vec<ToolDefinition> {
        self.catalog.visible_tools(profile, self.inner.config())
    }

    /// Register an additional internal tool at runtime.
    pub fn register(&mut self, tool: Box<dyn Tool>) {
        self.inner.register(tool);
    }

    /// Execute a tool call:
    /// 1. Domain executor when attached (pure HTTP — no sandbox needed).
    /// 2. Delegate to the inner [`ToolExecutor`] for built-in tools.
    #[instrument(skip(self, ctx), fields(tool = %tool_call.tool))]
    async fn execute_tool(
        &self,
        ctx: &ExecuteContext,
        tool_call: &ToolCall,
    ) -> Result<ToolResult, ToolError> {
        let tool_name = &tool_call.tool;

        if let Some(tool) = self.installed_tools.get(tool_name) {
            return self
                .execute_installed_tool(ctx, tool, &tool_call.args)
                .await;
        }

        // Domain tools (specs, tasks, project) — pure HTTP calls that
        // never touch the filesystem, so they must be dispatched before
        // Sandbox::new to avoid failing when the workspace dir is
        // inaccessible (e.g. remote agent on a different OS).
        if let Some(ref domain) = self.domain_executor {
            if domain.handles(tool_name) {
                let project_id = tool_call.args["project_id"].as_str().unwrap_or_default();
                let result_json = domain.execute(tool_name, project_id, &tool_call.args).await;
                let is_error = serde_json::from_str::<serde_json::Value>(&result_json)
                    .ok()
                    .and_then(|v| v.get("ok")?.as_bool())
                    .is_some_and(|ok| !ok);
                if is_error {
                    return Ok(ToolResult::failure(tool_name, result_json));
                }
                return Ok(ToolResult::success(tool_name, result_json));
            }
        }

        // Built-in tools — delegates permission checks, sandbox, and dispatch
        // to ToolExecutor so the logic is not duplicated.
        self.inner.execute_tool(ctx, tool_call).await
    }

    async fn execute_installed_tool(
        &self,
        ctx: &ExecuteContext,
        tool: &InstalledToolDefinition,
        args: &Value,
    ) -> Result<ToolResult, ToolError> {
        if let Some(runtime_execution) = &tool.runtime_execution {
            return self
                .execute_runtime_installed_tool(ctx, tool, args, runtime_execution)
                .await;
        }

        let mut headers = HeaderMap::new();
        headers.insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));
        match &tool.auth {
            ToolAuth::None => {}
            ToolAuth::Bearer { token } => {
                let value = HeaderValue::from_str(&format!("Bearer {token}")).map_err(|e| {
                    ToolError::ExternalToolError(format!("invalid bearer auth header: {e}"))
                })?;
                headers.insert(AUTHORIZATION, value);
            }
            ToolAuth::ApiKey { header, key } => {
                let name = HeaderName::from_bytes(header.as_bytes()).map_err(|e| {
                    ToolError::ExternalToolError(format!("invalid auth header name: {e}"))
                })?;
                let value = HeaderValue::from_str(key).map_err(|e| {
                    ToolError::ExternalToolError(format!("invalid api key header value: {e}"))
                })?;
                headers.insert(name, value);
            }
            ToolAuth::Headers { headers: extra } => {
                for (name, value) in extra {
                    let header_name = HeaderName::from_bytes(name.as_bytes()).map_err(|e| {
                        ToolError::ExternalToolError(format!(
                            "invalid auth header name `{name}`: {e}"
                        ))
                    })?;
                    let header_value = HeaderValue::from_str(value).map_err(|e| {
                        ToolError::ExternalToolError(format!(
                            "invalid auth header value for `{name}`: {e}"
                        ))
                    })?;
                    headers.insert(header_name, header_value);
                }
            }
        }

        headers.insert(
            HeaderName::from_static("x-aura-agent-id"),
            HeaderValue::from_str(&ctx.agent_id.to_string()).map_err(|e| {
                ToolError::ExternalToolError(format!("invalid x-aura-agent-id header: {e}"))
            })?,
        );

        let request = self
            .http_client
            .post(&tool.endpoint)
            .headers(headers)
            .json(args)
            .timeout(std::time::Duration::from_millis(
                tool.timeout_ms.unwrap_or(30_000),
            ));

        let response =
            request
                .send()
                .await
                .map_err(|e| ToolError::ExternalToolCallbackUnreachable {
                    url: tool.endpoint.clone(),
                    reason: e.to_string(),
                })?;
        let status = response.status();
        let body = response.text().await.map_err(|e| {
            ToolError::ExternalToolError(format!("reading installed tool response failed: {e}"))
        })?;

        if status.is_success() {
            Ok(ToolResult::success(&tool.name, body))
        } else {
            Err(ToolError::ExternalToolCallbackFailed {
                url: tool.endpoint.clone(),
                status: status.as_u16(),
                body,
            })
        }
    }

    async fn execute_runtime_installed_tool(
        &self,
        _ctx: &ExecuteContext,
        tool: &InstalledToolDefinition,
        args: &Value,
        execution: &InstalledToolRuntimeExecution,
    ) -> Result<ToolResult, ToolError> {
        let result = match execution {
            InstalledToolRuntimeExecution::AppProvider(provider) => {
                if let Some(spec) = trusted_runtime_spec(tool)? {
                    let integration = select_runtime_integration(provider, args)?;
                    self.execute_trusted_runtime_app_provider(provider, integration, args, &spec)
                        .await?
                } else {
                    self.execute_runtime_app_provider(tool, args, provider)
                        .await?
                }
            }
        };
        Ok(ToolResult::success(&tool.name, result.to_string()))
    }

    async fn execute_trusted_runtime_app_provider(
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

    async fn execute_runtime_app_provider(
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
            "more_results_available": response.pointer("/query/more_results_available").and_then(Value::as_bool).unwrap_or(false),
        }))
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
        if let Some(errors) = response.get("errors").and_then(Value::as_array) {
            if !errors.is_empty() {
                let message = errors
                    .iter()
                    .filter_map(|error| error.get("message").and_then(Value::as_str))
                    .collect::<Vec<_>>()
                    .join("; ");
                return Err(ToolError::ExternalToolError(format!(
                    "linear graphql error: {message}"
                )));
            }
        }
        Ok(response)
    }

    async fn provider_json_request(
        &self,
        method: Method,
        url: &str,
        provider: &InstalledToolRuntimeProviderExecution,
        integration: &InstalledToolRuntimeIntegration,
        body: Option<Value>,
    ) -> Result<Value, ToolError> {
        let final_url = runtime_url_with_auth(url, integration)?;
        let mut request = self.http_client.request(method, final_url);
        request = apply_runtime_headers(request, &provider.static_headers)?;
        request = apply_runtime_auth(request, integration)?;
        if let Some(body) = body {
            request = request.json(&body);
        }
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

    async fn provider_form_request(
        &self,
        method: Method,
        url: &str,
        provider: &InstalledToolRuntimeProviderExecution,
        integration: &InstalledToolRuntimeIntegration,
        body: Vec<(String, String)>,
    ) -> Result<Value, ToolError> {
        let final_url = runtime_url_with_auth(url, integration)?;
        let mut request = self.http_client.request(method, final_url);
        request = apply_runtime_headers(request, &provider.static_headers)?;
        request = apply_runtime_auth(request, integration)?;
        request = request.form(&body);
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

fn trusted_runtime_spec(
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
            while let Some(next) = chars.next() {
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

fn insert_json_path(target: &mut Value, path: &str, value: Value) -> Result<(), ToolError> {
    let parts = path
        .split('.')
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>();
    if parts.is_empty() {
        return Err(ToolError::ExternalToolError(
            "trusted integration metadata declared an empty target path".into(),
        ));
    }

    let mut current = target;
    for part in &parts[..parts.len() - 1] {
        if !current.is_object() {
            *current = json!({});
        }
        current = current
            .as_object_mut()
            .expect("object ensured above")
            .entry((*part).to_string())
            .or_insert_with(|| json!({}));
    }

    current
        .as_object_mut()
        .ok_or_else(|| {
            ToolError::ExternalToolError(format!(
                "trusted integration target path `{path}` does not resolve to an object"
            ))
        })?
        .insert(parts[parts.len() - 1].to_string(), value);
    Ok(())
}

fn apply_success_guard(
    response: &Value,
    guard: &TrustedIntegrationSuccessGuard,
) -> Result<(), ToolError> {
    match guard {
        TrustedIntegrationSuccessGuard::None => Ok(()),
        TrustedIntegrationSuccessGuard::SlackOk => ensure_slack_ok(response),
        TrustedIntegrationSuccessGuard::GraphqlErrors => {
            if let Some(errors) = response.get("errors").and_then(Value::as_array) {
                if !errors.is_empty() {
                    let message = errors
                        .iter()
                        .filter_map(|error| error.get("message").and_then(Value::as_str))
                        .collect::<Vec<_>>()
                        .join("; ");
                    return Err(ToolError::ExternalToolError(format!(
                        "graphql error: {message}"
                    )));
                }
            }
            Ok(())
        }
    }
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
                "more_results_available": response.pointer("/query/more_results_available").and_then(Value::as_bool).unwrap_or(false),
            }))
        }
    }
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

fn select_runtime_integration<'a>(
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

fn apply_runtime_headers(
    mut request: RequestBuilder,
    headers: &HashMap<String, String>,
) -> Result<RequestBuilder, ToolError> {
    request = request.header(ACCEPT, "application/json");
    request = request.header(CONTENT_TYPE, "application/json");
    request = request.header(USER_AGENT, "aura-harness");
    for (name, value) in headers {
        let header_name = HeaderName::from_bytes(name.as_bytes()).map_err(|e| {
            ToolError::ExternalToolError(format!("invalid runtime header name `{name}`: {e}"))
        })?;
        let header_value = HeaderValue::from_str(value).map_err(|e| {
            ToolError::ExternalToolError(format!("invalid runtime header value for `{name}`: {e}"))
        })?;
        request = request.header(header_name, header_value);
    }
    Ok(request)
}

fn apply_runtime_auth(
    mut request: RequestBuilder,
    integration: &InstalledToolRuntimeIntegration,
) -> Result<RequestBuilder, ToolError> {
    match &integration.auth {
        InstalledToolRuntimeAuth::None => {}
        InstalledToolRuntimeAuth::AuthorizationBearer { token } => {
            request = request.bearer_auth(token);
        }
        InstalledToolRuntimeAuth::AuthorizationRaw { value } => {
            request = request.header(AUTHORIZATION, value);
        }
        InstalledToolRuntimeAuth::Header { name, value } => {
            request = request.header(name, value);
        }
        InstalledToolRuntimeAuth::QueryParam { .. } => {}
        InstalledToolRuntimeAuth::Basic { username, password } => {
            request = request.basic_auth(username, Some(password));
        }
    }
    Ok(request)
}

fn runtime_url_with_auth(
    url: &str,
    integration: &InstalledToolRuntimeIntegration,
) -> Result<String, ToolError> {
    match &integration.auth {
        InstalledToolRuntimeAuth::QueryParam { name, value } => {
            let mut parsed = Url::parse(url).map_err(|e| {
                ToolError::ExternalToolError(format!("invalid runtime url for query auth: {e}"))
            })?;
            parsed.query_pairs_mut().append_pair(name, value);
            Ok(parsed.to_string())
        }
        _ => Ok(url.to_string()),
    }
}

fn required_string(args: &Value, keys: &[&str]) -> Result<String, ToolError> {
    optional_string(args, keys).ok_or_else(|| {
        ToolError::ExternalToolError(format!("missing required field `{}`", keys[0]))
    })
}

fn optional_string(args: &Value, keys: &[&str]) -> Option<String> {
    optional_string_from_names(
        args,
        &keys
            .iter()
            .map(|key| (*key).to_string())
            .collect::<Vec<_>>(),
    )
}

fn optional_string_from_names(args: &Value, keys: &[String]) -> Option<String> {
    keys.iter().find_map(|key| {
        args.get(key.as_str())
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_string)
    })
}

fn optional_string_from_names_map(
    values: &HashMap<String, Value>,
    keys: &[String],
) -> Option<String> {
    keys.iter().find_map(|key| {
        values
            .get(key.as_str())
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_string)
    })
}

fn required_string_list(args: &Value, keys: &[&str]) -> Result<Vec<String>, ToolError> {
    optional_string_list(args, keys).ok_or_else(|| {
        ToolError::ExternalToolError(format!("missing required field `{}`", keys[0]))
    })
}

fn optional_string_list(args: &Value, keys: &[&str]) -> Option<Vec<String>> {
    optional_string_list_from_names(
        args,
        &keys
            .iter()
            .map(|key| (*key).to_string())
            .collect::<Vec<_>>(),
    )
}

fn optional_string_list_from_names(args: &Value, keys: &[String]) -> Option<Vec<String>> {
    keys.iter().find_map(|key| {
        let value = args.get(key.as_str())?;
        if let Some(single) = value
            .as_str()
            .map(str::trim)
            .filter(|value| !value.is_empty())
        {
            return Some(vec![single.to_string()]);
        }
        value
            .as_array()
            .map(|items| {
                items
                    .iter()
                    .filter_map(Value::as_str)
                    .map(str::trim)
                    .filter(|value| !value.is_empty())
                    .map(str::to_string)
                    .collect::<Vec<_>>()
            })
            .filter(|items| !items.is_empty())
    })
}

fn optional_string_list_from_names_map(
    values: &HashMap<String, Value>,
    keys: &[String],
) -> Option<Vec<String>> {
    keys.iter().find_map(|key| {
        let value = values.get(key.as_str())?;
        if let Some(single) = value
            .as_str()
            .map(str::trim)
            .filter(|value| !value.is_empty())
        {
            return Some(vec![single.to_string()]);
        }
        value
            .as_array()
            .map(|items| {
                items
                    .iter()
                    .filter_map(Value::as_str)
                    .map(str::trim)
                    .filter(|value| !value.is_empty())
                    .map(str::to_string)
                    .collect::<Vec<_>>()
            })
            .filter(|items| !items.is_empty())
    })
}

fn optional_positive_number(args: &Value, keys: &[&str]) -> Option<u64> {
    optional_positive_number_from_names(
        args,
        &keys
            .iter()
            .map(|key| (*key).to_string())
            .collect::<Vec<_>>(),
    )
}

fn optional_positive_number_from_names(args: &Value, keys: &[String]) -> Option<u64> {
    keys.iter()
        .find_map(|key| args.get(key.as_str()).and_then(Value::as_u64))
}

fn optional_positive_number_from_names_map(
    values: &HashMap<String, Value>,
    keys: &[String],
) -> Option<u64> {
    keys.iter()
        .find_map(|key| values.get(key.as_str()).and_then(Value::as_u64))
}

fn optional_json_from_names(args: &Value, keys: &[String]) -> Option<Value> {
    keys.iter().find_map(|key| args.get(key.as_str()).cloned())
}

fn optional_json_from_names_map(values: &HashMap<String, Value>, keys: &[String]) -> Option<Value> {
    keys.iter()
        .find_map(|key| values.get(key.as_str()).cloned())
}

fn ensure_slack_ok(response: &Value) -> Result<(), ToolError> {
    if response.get("ok").and_then(Value::as_bool).unwrap_or(false) {
        return Ok(());
    }
    let error = response
        .get("error")
        .and_then(Value::as_str)
        .unwrap_or("unknown slack error");
    Err(ToolError::ExternalToolError(format!(
        "slack api error: {error}"
    )))
}

// ---------------------------------------------------------------------------
// Executor trait impl  — allows the resolver to be used in ExecutorRouter
// ---------------------------------------------------------------------------

#[async_trait]
impl Executor for ToolResolver {
    #[instrument(skip(self, ctx, action), fields(action_id = %action.action_id))]
    async fn execute(
        &self,
        ctx: &ExecuteContext,
        action: &Action,
    ) -> Result<Effect, ExecutorError> {
        let tool_call: ToolCall = serde_json::from_slice(&action.payload).map_err(|e| {
            ExecutorError::ExecutionFailed(format!("Failed to parse tool call: {e}"))
        })?;

        debug!(tool = %tool_call.tool, "Executing tool via resolver");

        match self.execute_tool(ctx, &tool_call).await {
            Ok(result) => {
                let payload = serde_json::to_vec(&result).map_err(|e| {
                    ExecutorError::ExecutionFailed(format!("Failed to serialize tool result: {e}"))
                })?;
                Ok(Effect::new(
                    action.action_id,
                    EffectKind::Agreement,
                    EffectStatus::Committed,
                    Bytes::from(payload),
                ))
            }
            Err(e) => {
                error!(error = %e, "Tool execution failed");
                let result = ToolResult::failure(&tool_call.tool, e.to_string());
                let payload = serde_json::to_vec(&result).map_err(|e| {
                    ExecutorError::ExecutionFailed(format!("Failed to serialize error result: {e}"))
                })?;
                Ok(Effect::new(
                    action.action_id,
                    EffectKind::Agreement,
                    EffectStatus::Failed,
                    Bytes::from(payload),
                ))
            }
        }
    }

    fn can_handle(&self, action: &Action) -> bool {
        if action.kind != ActionKind::Delegate {
            return false;
        }
        serde_json::from_slice::<ToolCall>(&action.payload).is_ok()
    }

    fn name(&self) -> &'static str {
        "tool_resolver"
    }
}

#[cfg(test)]
#[path = "resolver_tests.rs"]
mod tests;
