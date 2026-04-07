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
    Action, ActionKind, Effect, EffectKind, EffectStatus, InstalledToolDefinition, ToolAuth,
    ToolCall, ToolResult,
};
use aura_kernel::{ExecuteContext, Executor, ExecutorError};
use bytes::Bytes;
use reqwest::header::{HeaderMap, HeaderName, HeaderValue, AUTHORIZATION, CONTENT_TYPE};
use reqwest::Client;
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
        args: &serde_json::Value,
    ) -> Result<ToolResult, ToolError> {
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
