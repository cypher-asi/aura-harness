//! Tool resolver — unified dispatch layer for tool execution.
//!
//! The resolver owns the internal `Tool` implementations (built-in handlers).
//! Domain tools are delegated when a [`DomainToolExecutor`] is attached.
//! Additional tools can be registered at runtime with [`ToolResolver::register`].

use crate::catalog::ToolCatalog;
use crate::catalog::ToolProfile;
use crate::domain_tools::DomainToolExecutor;
use crate::error::ToolError;
use crate::sandbox::Sandbox;
use crate::tool::{builtin_tools, Tool, ToolContext};
use crate::ToolConfig;
use async_trait::async_trait;
use aura_core::{Action, ActionKind, Effect, EffectKind, EffectStatus, ToolCall, ToolResult};
use aura_executor::{ExecuteContext, Executor};
use aura_reasoner::ToolDefinition;
use bytes::Bytes;
use std::collections::HashMap;
use std::sync::Arc;
use tracing::{debug, error, instrument};

/// Unified tool resolver providing both visibility and execution dispatch.
///
/// Implements [`Executor`] so it can be plugged into the kernel layer
/// (scheduler, `ExecutorRouter`) as a drop-in replacement for `ToolExecutor`.
pub struct ToolResolver {
    catalog: Arc<ToolCatalog>,
    tools: HashMap<String, Box<dyn Tool>>,
    domain_executor: Option<Arc<DomainToolExecutor>>,
    config: ToolConfig,
}

impl ToolResolver {
    /// Create a resolver pre-loaded with all built-in tool handlers.
    #[must_use]
    pub fn new(catalog: Arc<ToolCatalog>, config: ToolConfig) -> Self {
        let mut tools: HashMap<String, Box<dyn Tool>> = HashMap::new();
        for tool in builtin_tools() {
            tools.insert(tool.name().to_string(), tool);
        }
        Self {
            catalog,
            tools,
            domain_executor: None,
            config,
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
        self.catalog.visible_tools(profile, &self.config)
    }

    /// Register an additional internal tool at runtime.
    pub fn register(&mut self, tool: Box<dyn Tool>) {
        self.tools.insert(tool.name().to_string(), tool);
    }

    /// Execute a tool call (after FS/command permission gates):
    /// 1. Internal handler map (built-ins and [`register`](Self::register) tools).
    /// 2. Domain executor when attached.
    /// 3. [`ToolError::UnknownTool`] if nothing matches.
    #[instrument(skip(self, ctx), fields(tool = %tool_call.tool))]
    async fn execute_tool(
        &self,
        ctx: &ExecuteContext,
        tool_call: &ToolCall,
    ) -> Result<ToolResult, ToolError> {
        let tool_name = &tool_call.tool;

        const FS_TOOLS: &[&str] = &[
            "read_file",
            "write_file",
            "edit_file",
            "delete_file",
            "list_files",
            "find_files",
            "stat_file",
            "search_code",
        ];
        const CMD_TOOLS: &[&str] = &["run_command"];

        if FS_TOOLS.contains(&tool_name.as_str()) && !self.config.enable_fs {
            return Err(ToolError::ToolDisabled(tool_name.clone()));
        }
        if CMD_TOOLS.contains(&tool_name.as_str()) && !self.config.enable_commands {
            return Err(ToolError::ToolDisabled(tool_name.clone()));
        }

        let sandbox = Sandbox::new(&ctx.workspace_root)?;
        let tool_ctx = ToolContext {
            sandbox,
            config: self.config.clone(),
        };

        // 1. Internal handler (highest precedence)
        if let Some(tool) = self.tools.get(tool_name.as_str()) {
            return tool.execute(&tool_ctx, tool_call.args.clone()).await;
        }

        // 2. Domain tools (specs, tasks, project)
        if let Some(ref domain) = self.domain_executor {
            if domain.handles(tool_name) {
                let project_id = tool_call.args["project_id"]
                    .as_str()
                    .unwrap_or_default();
                let result_json = domain
                    .execute(tool_name, project_id, &tool_call.args)
                    .await;
                let is_error = serde_json::from_str::<serde_json::Value>(&result_json)
                    .ok()
                    .and_then(|v| v.get("ok")?.as_bool())
                    .map_or(false, |ok| !ok);
                if is_error {
                    return Ok(ToolResult::failure(tool_name, result_json));
                }
                return Ok(ToolResult::success(tool_name, result_json));
            }
        } else if crate::domain_tools::is_domain_tool(tool_name) {
            return Err(ToolError::DomainUnavailable(tool_name.clone()));
        }

        Err(ToolError::UnknownTool(tool_name.clone()))
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
    ) -> Result<Effect, aura_executor::ExecutorError> {
        let tool_call: ToolCall = serde_json::from_slice(&action.payload).map_err(|e| {
            aura_executor::ExecutorError::ExecutionFailed(format!(
                "Failed to parse tool call: {e}"
            ))
        })?;

        debug!(tool = %tool_call.tool, "Executing tool via resolver");

        match self.execute_tool(ctx, &tool_call).await {
            Ok(result) => {
                let payload = serde_json::to_vec(&result).map_err(|e| {
                    aura_executor::ExecutorError::ExecutionFailed(format!(
                        "Failed to serialize tool result: {e}"
                    ))
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
                    aura_executor::ExecutorError::ExecutionFailed(format!(
                        "Failed to serialize error result: {e}"
                    ))
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

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use aura_core::{ActionId, AgentId};
    use tempfile::TempDir;

    fn make_catalog_and_resolver() -> (Arc<ToolCatalog>, ToolResolver) {
        let cat = Arc::new(ToolCatalog::new());
        let resolver = ToolResolver::new(cat.clone(), ToolConfig::default());
        (cat, resolver)
    }

    fn test_context() -> (ExecuteContext, TempDir) {
        let dir = TempDir::new().unwrap();
        let ctx = ExecuteContext::new(
            AgentId::generate(),
            ActionId::generate(),
            dir.path().to_path_buf(),
        );
        (ctx, dir)
    }

    #[test]
    fn resolver_has_builtin_tools() {
        let (_cat, resolver) = make_catalog_and_resolver();
        assert!(resolver.tools.contains_key("read_file"));
        assert!(resolver.tools.contains_key("run_command"));
    }

    #[test]
    fn visible_tools_returns_core() {
        let (_cat, resolver) = make_catalog_and_resolver();
        let tools = resolver.visible_tools(ToolProfile::Core);
        let names: std::collections::HashSet<_> =
            tools.iter().map(|t| t.name.as_str()).collect();
        assert!(names.contains("read_file"));
    }

    #[tokio::test]
    async fn execute_builtin_tool() {
        let (_cat, resolver) = make_catalog_and_resolver();
        let (ctx, dir) = test_context();
        std::fs::write(dir.path().join("hello.txt"), "world").unwrap();

        let tc = ToolCall::fs_ls(".");
        let action = Action::delegate_tool(&tc).unwrap();
        let effect = resolver.execute(&ctx, &action).await.unwrap();
        assert_eq!(effect.status, EffectStatus::Committed);
    }

    #[tokio::test]
    async fn unknown_tool_returns_failed_effect() {
        let (_cat, resolver) = make_catalog_and_resolver();
        let (ctx, _dir) = test_context();

        let tc = ToolCall::new("no_such_tool", serde_json::json!({}));
        let action = Action::delegate_tool(&tc).unwrap();
        let effect = resolver.execute(&ctx, &action).await.unwrap();
        assert_eq!(effect.status, EffectStatus::Failed);
        let result: ToolResult = serde_json::from_slice(&effect.payload).unwrap();
        let err_msg = std::str::from_utf8(&result.stderr).unwrap();
        assert!(
            err_msg.contains("unknown tool"),
            "truly unknown tool should say 'unknown tool', got: {err_msg}",
        );
    }

    #[tokio::test]
    async fn domain_tool_without_executor_returns_domain_unavailable() {
        let (_cat, resolver) = make_catalog_and_resolver();
        let (ctx, _dir) = test_context();

        let tc = ToolCall::new("create_spec", serde_json::json!({"project_id": "p1"}));
        let action = Action::delegate_tool(&tc).unwrap();
        let effect = resolver.execute(&ctx, &action).await.unwrap();
        assert_eq!(effect.status, EffectStatus::Failed);
        let result: ToolResult = serde_json::from_slice(&effect.payload).unwrap();
        let err_msg = std::str::from_utf8(&result.stderr).unwrap();
        assert!(
            err_msg.contains("unavailable") && err_msg.contains("INTERNAL_SERVICE_TOKEN"),
            "domain tool without executor should mention config, got: {err_msg}",
        );
    }

    #[tokio::test]
    async fn fs_disabled_returns_failed() {
        let cat = Arc::new(ToolCatalog::new());
        let mut config = ToolConfig::default();
        config.enable_fs = false;
        let resolver = ToolResolver::new(cat, config);
        let (ctx, _dir) = test_context();

        let tc = ToolCall::fs_read("test.txt", None);
        let action = Action::delegate_tool(&tc).unwrap();
        let effect = resolver.execute(&ctx, &action).await.unwrap();
        assert_eq!(effect.status, EffectStatus::Failed);
    }

    #[test]
    fn every_exposed_core_tool_has_handler() {
        let (_cat, resolver) = make_catalog_and_resolver();
        let core = _cat.tools_for_profile(ToolProfile::Core);
        for t in &core {
            let has_handler = resolver.tools.contains_key(t.name.as_str());
            assert!(
                has_handler,
                "core tool '{}' has no built-in handler",
                t.name,
            );
        }
    }
}
