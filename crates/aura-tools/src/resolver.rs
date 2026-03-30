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
        self.catalog.visible_tools(profile, &self.inner.config())
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
                    .map_or(false, |ok| !ok);
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

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use aura_core::{ActionId, AgentId};
    use aura_kernel::ExecuteContext;
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
        let tools = resolver.visible_tools(ToolProfile::Core);
        let names: std::collections::HashSet<_> = tools.iter().map(|t| t.name.as_str()).collect();
        assert!(names.contains("read_file"));
    }

    #[test]
    fn visible_tools_returns_core() {
        let (_cat, resolver) = make_catalog_and_resolver();
        let tools = resolver.visible_tools(ToolProfile::Core);
        let names: std::collections::HashSet<_> = tools.iter().map(|t| t.name.as_str()).collect();
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
    async fn domain_tool_without_executor_falls_through_to_unknown() {
        let (_cat, resolver) = make_catalog_and_resolver();
        let (ctx, _dir) = test_context();

        let tc = ToolCall::new("create_spec", serde_json::json!({"project_id": "p1"}));
        let action = Action::delegate_tool(&tc).unwrap();
        let effect = resolver.execute(&ctx, &action).await.unwrap();
        assert_eq!(effect.status, EffectStatus::Failed);
        let result: ToolResult = serde_json::from_slice(&effect.payload).unwrap();
        let err_msg = std::str::from_utf8(&result.stderr).unwrap();
        assert!(
            err_msg.contains("unknown tool"),
            "domain tool without executor should now be 'unknown tool', got: {err_msg}",
        );
    }

    mod stub_domain {
        use async_trait::async_trait;
        use aura_tools_domain::*;

        pub struct StubDomainApi;

        #[async_trait]
        impl DomainApi for StubDomainApi {
            async fn list_specs(
                &self,
                _: &str,
                _: Option<&str>,
            ) -> anyhow::Result<Vec<SpecDescriptor>> {
                Ok(vec![])
            }
            async fn get_spec(&self, _: &str, _: Option<&str>) -> anyhow::Result<SpecDescriptor> {
                anyhow::bail!("stub")
            }
            async fn create_spec(
                &self,
                _: &str,
                title: &str,
                _: &str,
                _: u32,
                _: Option<&str>,
            ) -> anyhow::Result<SpecDescriptor> {
                Ok(SpecDescriptor {
                    id: "s1".into(),
                    project_id: "p1".into(),
                    title: title.into(),
                    content: String::new(),
                    order: 0,
                    parent_id: None,
                })
            }
            async fn update_spec(
                &self,
                _: &str,
                _: Option<&str>,
                _: Option<&str>,
                _: Option<&str>,
            ) -> anyhow::Result<SpecDescriptor> {
                anyhow::bail!("stub")
            }
            async fn delete_spec(&self, _: &str, _: Option<&str>) -> anyhow::Result<()> {
                Ok(())
            }
            async fn list_tasks(
                &self,
                _: &str,
                _: Option<&str>,
                _: Option<&str>,
            ) -> anyhow::Result<Vec<TaskDescriptor>> {
                Ok(vec![])
            }
            async fn create_task(
                &self,
                _: &str,
                _: &str,
                _: &str,
                _: &str,
                _: &[String],
                _: u32,
                _: Option<&str>,
            ) -> anyhow::Result<TaskDescriptor> {
                anyhow::bail!("stub")
            }
            async fn update_task(
                &self,
                _: &str,
                _: TaskUpdate,
                _: Option<&str>,
            ) -> anyhow::Result<TaskDescriptor> {
                anyhow::bail!("stub")
            }
            async fn delete_task(&self, _: &str, _: Option<&str>) -> anyhow::Result<()> {
                Ok(())
            }
            async fn transition_task(
                &self,
                _: &str,
                _: &str,
                _: Option<&str>,
            ) -> anyhow::Result<TaskDescriptor> {
                anyhow::bail!("stub")
            }
            async fn claim_next_task(
                &self,
                _: &str,
                _: &str,
                _: Option<&str>,
            ) -> anyhow::Result<Option<TaskDescriptor>> {
                Ok(None)
            }
            async fn get_task(&self, _: &str, _: Option<&str>) -> anyhow::Result<TaskDescriptor> {
                anyhow::bail!("stub")
            }
            async fn get_project(
                &self,
                project_id: &str,
                _: Option<&str>,
            ) -> anyhow::Result<ProjectDescriptor> {
                Ok(ProjectDescriptor {
                    id: project_id.into(),
                    name: "test".into(),
                    path: String::new(),
                    description: None,
                    tech_stack: None,
                    build_command: None,
                    test_command: None,
                })
            }
            async fn update_project(
                &self,
                _: &str,
                _: ProjectUpdate,
                _: Option<&str>,
            ) -> anyhow::Result<ProjectDescriptor> {
                anyhow::bail!("stub")
            }
            async fn create_log(
                &self,
                _: &str,
                _: &str,
                _: &str,
                _: Option<&str>,
                _: Option<&serde_json::Value>,
                _: Option<&str>,
            ) -> anyhow::Result<serde_json::Value> {
                Ok(serde_json::json!({}))
            }
            async fn list_logs(
                &self,
                _: &str,
                _: Option<&str>,
                _: Option<u64>,
                _: Option<&str>,
            ) -> anyhow::Result<serde_json::Value> {
                Ok(serde_json::json!([]))
            }
            async fn get_project_stats(
                &self,
                _: &str,
                _: Option<&str>,
            ) -> anyhow::Result<serde_json::Value> {
                Ok(serde_json::json!({}))
            }
            async fn list_messages(
                &self,
                _: &str,
                _: &str,
            ) -> anyhow::Result<Vec<MessageDescriptor>> {
                Ok(vec![])
            }
            async fn save_message(&self, _: SaveMessageParams) -> anyhow::Result<()> {
                Ok(())
            }
            async fn create_session(
                &self,
                _: CreateSessionParams,
            ) -> anyhow::Result<SessionDescriptor> {
                anyhow::bail!("stub")
            }
            async fn get_active_session(
                &self,
                _: &str,
            ) -> anyhow::Result<Option<SessionDescriptor>> {
                Ok(None)
            }
            async fn orbit_api_call(
                &self,
                _: &str,
                _: &str,
                _: Option<&serde_json::Value>,
                _: Option<&str>,
            ) -> anyhow::Result<String> {
                Ok("{}".into())
            }
            async fn network_api_call(
                &self,
                _: &str,
                _: &str,
                _: Option<&serde_json::Value>,
                _: Option<&str>,
            ) -> anyhow::Result<String> {
                Ok("{}".into())
            }
        }

        use crate::domain_tools as aura_tools_domain;
    }

    #[tokio::test]
    async fn domain_tool_succeeds_with_inaccessible_workspace() {
        use crate::domain_tools::DomainToolExecutor;

        let cat = Arc::new(ToolCatalog::new());
        let resolver =
            ToolResolver::new(cat, ToolConfig::default()).with_domain_executor(Arc::new(
                DomainToolExecutor::new(Arc::new(stub_domain::StubDomainApi)),
            ));

        // Use a workspace path that does not exist and cannot be created.
        let ctx = ExecuteContext::new(
            AgentId::generate(),
            ActionId::generate(),
            std::path::PathBuf::from("/nonexistent/impossible/workspace"),
        );

        let tc = ToolCall::new(
            "create_spec",
            serde_json::json!({
                "project_id": "p1",
                "title": "Hello World",
                "content": "# Hello"
            }),
        );
        let result = resolver.execute_tool(&ctx, &tc).await;
        assert!(
            result.is_ok(),
            "domain tool should succeed even with inaccessible workspace"
        );
        let tr = result.unwrap();
        let stdout = std::str::from_utf8(&tr.stdout).unwrap();
        assert!(
            stdout.contains("\"ok\":true"),
            "create_spec should return ok:true, got: {stdout}"
        );
    }

    #[tokio::test]
    async fn get_project_succeeds_with_inaccessible_workspace() {
        use crate::domain_tools::DomainToolExecutor;

        let cat = Arc::new(ToolCatalog::new());
        let resolver =
            ToolResolver::new(cat, ToolConfig::default()).with_domain_executor(Arc::new(
                DomainToolExecutor::new(Arc::new(stub_domain::StubDomainApi)),
            ));

        let ctx = ExecuteContext::new(
            AgentId::generate(),
            ActionId::generate(),
            std::path::PathBuf::from("/nonexistent/impossible/workspace"),
        );

        let tc = ToolCall::new("get_project", serde_json::json!({"project_id": "p1"}));
        let result = resolver.execute_tool(&ctx, &tc).await;
        assert!(
            result.is_ok(),
            "get_project should succeed even with inaccessible workspace"
        );
        let tr = result.unwrap();
        let stdout = std::str::from_utf8(&tr.stdout).unwrap();
        assert!(
            stdout.contains("\"ok\":true"),
            "get_project should return ok:true, got: {stdout}"
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
            let has_handler = resolver.inner.has_tool(&t.name);
            assert!(
                has_handler,
                "core tool '{}' has no built-in handler",
                t.name,
            );
        }
    }
}
