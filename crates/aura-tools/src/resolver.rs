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
use aura_core::{Action, ActionKind, Effect, EffectKind, EffectStatus, ToolCall, ToolResult};
use aura_kernel::{ExecuteContext, Executor, ExecutorError};
use bytes::Bytes;
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
}

impl ToolResolver {
    /// Create a resolver pre-loaded with all built-in tool handlers.
    #[must_use]
    pub fn new(catalog: Arc<ToolCatalog>, config: ToolConfig) -> Self {
        Self {
            catalog,
            inner: ToolExecutor::new(config),
            domain_executor: None,
        }
    }

    /// Attach a domain tool executor for specs/tasks/project dispatch.
    #[must_use]
    pub fn with_domain_executor(mut self, exec: Arc<DomainToolExecutor>) -> Self {
        self.domain_executor = Some(exec);
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
