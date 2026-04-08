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
    InstalledToolRuntimeAuth, InstalledToolRuntimeExecution,
    InstalledToolRuntimeIntegration, InstalledToolRuntimeProviderExecution, ToolAuth, ToolCall,
    ToolResult,
};
use aura_kernel::{ExecuteContext, Executor, ExecutorError};
use bytes::Bytes;
use reqwest::header::{
    HeaderMap, HeaderName, HeaderValue, ACCEPT, AUTHORIZATION, CONTENT_TYPE, USER_AGENT,
};
use reqwest::{Client, Method, RequestBuilder, Url};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::sync::Arc;
use tracing::{debug, error, instrument};

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
            return self.execute_installed_tool(ctx, tool, &tool_call.args).await;
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
            HeaderValue::from_str(&ctx.agent_id.to_string())
                .map_err(|e| {
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

        let response = request
            .send()
            .await
            .map_err(|e| ToolError::ExternalToolCallbackUnreachable {
                url: tool.endpoint.clone(),
                reason: e.to_string(),
            })?;
        let status = response.status();
        let body = response
            .text()
            .await
            .map_err(|e| ToolError::ExternalToolError(format!(
                "reading installed tool response failed: {e}"
            )))?;

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
                self.execute_runtime_app_provider(tool, args, provider).await?
            }
        };
        Ok(ToolResult::success(&tool.name, result.to_string()))
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
            &["description", "body", "markdown_contents", "markdownContents"],
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
        let response = request.send().await.map_err(|e| {
            ToolError::ExternalToolError(format!("provider request failed: {e}"))
        })?;
        let status = response.status();
        let text = response
            .text()
            .await
            .map_err(|e| ToolError::ExternalToolError(format!(
                "reading provider response failed: {e}"
            )))?;
        if !status.is_success() {
            return Err(ToolError::ExternalToolError(format!(
                "provider request failed with {}: {}",
                status, text
            )));
        }
        serde_json::from_str(&text)
            .map_err(|e| ToolError::ExternalToolError(format!("provider returned invalid JSON: {e}")))
    }
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
            ToolError::ExternalToolError(format!(
                "invalid runtime header value for `{name}`: {e}"
            ))
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
    keys.iter().find_map(|key| {
        args.get(*key)
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
    keys.iter().find_map(|key| {
        let value = args.get(*key)?;
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
    keys.iter()
        .find_map(|key| args.get(*key).and_then(Value::as_u64))
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
