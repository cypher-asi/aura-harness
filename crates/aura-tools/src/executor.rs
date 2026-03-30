//! Tool executor implementation.

use crate::error::ToolError;
use crate::sandbox::Sandbox;
use crate::tool::{builtin_tools, Tool, ToolContext};
use crate::ToolConfig;
use async_trait::async_trait;
use aura_core::{Action, ActionKind, Effect, EffectKind, EffectStatus, ToolCall, ToolResult};
use aura_core::{ExecuteContext, Executor};
use bytes::Bytes;
use std::collections::HashMap;
use tracing::{debug, error, instrument};

/// Tool executor for filesystem and command operations.
///
/// Holds a `HashMap<String, Box<dyn Tool>>` for trait-based dispatch
/// instead of a hardcoded match block.
pub struct ToolExecutor {
    config: ToolConfig,
    tools: HashMap<String, Box<dyn Tool>>,
}

impl ToolExecutor {
    /// Create a new tool executor with the given config and all builtin tools.
    #[must_use]
    pub fn new(config: ToolConfig) -> Self {
        let mut tools = HashMap::new();
        for tool in builtin_tools() {
            tools.insert(tool.name().to_string(), tool);
        }
        Self { config, tools }
    }

    /// Create a tool executor with default config.
    #[must_use]
    pub fn with_defaults() -> Self {
        Self::new(ToolConfig::default())
    }

    /// Register an additional tool at runtime.
    pub fn register(&mut self, tool: Box<dyn Tool>) {
        self.tools.insert(tool.name().to_string(), tool);
    }

    /// Execute a tool call.
    #[instrument(skip(self, ctx), fields(tool = %tool_call.tool))]
    async fn execute_tool(
        &self,
        ctx: &ExecuteContext,
        tool_call: &ToolCall,
    ) -> Result<ToolResult, ToolError> {
        let tool_name = &tool_call.tool;

        // Category-level permission checks
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

        match self.tools.get(tool_name.as_str()) {
            Some(tool) => tool.execute(&tool_ctx, tool_call.args.clone()).await,
            None => Err(ToolError::UnknownTool(tool_name.clone())),
        }
    }
}

#[async_trait]
impl Executor for ToolExecutor {
    #[instrument(skip(self, ctx, action), fields(action_id = %action.action_id))]
    async fn execute(&self, ctx: &ExecuteContext, action: &Action) -> Result<Effect, aura_core::ExecutorError> {
        let tool_call: ToolCall = serde_json::from_slice(&action.payload)
            .map_err(|e| aura_core::ExecutorError::ExecutionFailed(format!("Failed to parse tool call: {e}")))?;

        debug!(tool = %tool_call.tool, "Executing tool");

        match self.execute_tool(ctx, &tool_call).await {
            Ok(result) => {
                let payload = serde_json::to_vec(&result)
                    .map_err(|e| aura_core::ExecutorError::ExecutionFailed(format!("Failed to serialize tool result: {e}")))?;
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
                let payload = serde_json::to_vec(&result)
                    .map_err(|e| aura_core::ExecutorError::ExecutionFailed(format!("Failed to serialize error result: {e}")))?;
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
        "tool"
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use aura_core::{ActionId, AgentId};
    use tempfile::TempDir;

    fn create_test_context() -> (ExecuteContext, TempDir) {
        let dir = TempDir::new().unwrap();
        let ctx = ExecuteContext::new(
            AgentId::generate(),
            ActionId::generate(),
            dir.path().to_path_buf(),
        );
        (ctx, dir)
    }

    #[tokio::test]
    async fn test_fs_ls_tool() {
        let (ctx, dir) = create_test_context();
        std::fs::write(dir.path().join("test.txt"), "hello").unwrap();

        let executor = ToolExecutor::with_defaults();
        let tool_call = ToolCall::fs_ls(".");
        let action = Action::delegate_tool(&tool_call).unwrap();

        let effect = executor.execute(&ctx, &action).await.unwrap();
        assert_eq!(effect.status, EffectStatus::Committed);

        let result: ToolResult = serde_json::from_slice(&effect.payload).unwrap();
        assert!(result.ok);
        let output = String::from_utf8_lossy(&result.stdout);
        assert!(output.contains("test.txt"));
    }

    #[tokio::test]
    async fn test_fs_read_tool() {
        let (ctx, dir) = create_test_context();
        std::fs::write(dir.path().join("test.txt"), "Hello, Aura!").unwrap();

        let executor = ToolExecutor::with_defaults();
        let tool_call = ToolCall::fs_read("test.txt", None);
        let action = Action::delegate_tool(&tool_call).unwrap();

        let effect = executor.execute(&ctx, &action).await.unwrap();
        assert_eq!(effect.status, EffectStatus::Committed);

        let result: ToolResult = serde_json::from_slice(&effect.payload).unwrap();
        assert!(result.ok);
        assert_eq!(&result.stdout[..], b"Hello, Aura!");
    }

    #[tokio::test]
    async fn test_sandbox_violation() {
        let (ctx, _dir) = create_test_context();

        let executor = ToolExecutor::with_defaults();
        let tool_call = ToolCall::fs_read("../../../etc/passwd", None);
        let action = Action::delegate_tool(&tool_call).unwrap();

        let effect = executor.execute(&ctx, &action).await.unwrap();
        assert_eq!(effect.status, EffectStatus::Failed);

        let result: ToolResult = serde_json::from_slice(&effect.payload).unwrap();
        assert!(!result.ok);
    }

    #[tokio::test]
    async fn test_cmd_disabled() {
        let (ctx, _dir) = create_test_context();

        let mut config = ToolConfig::default();
        config.enable_commands = false;
        let executor = ToolExecutor::new(config);
        let tool_call = ToolCall::new("run_command", serde_json::json!({"program": "ls"}));
        let action = Action::delegate_tool(&tool_call).unwrap();

        let effect = executor.execute(&ctx, &action).await.unwrap();
        assert_eq!(effect.status, EffectStatus::Failed);
    }

    #[tokio::test]
    async fn test_unknown_tool() {
        let (ctx, _dir) = create_test_context();

        let executor = ToolExecutor::with_defaults();
        let tool_call = ToolCall::new("nonexistent_tool", serde_json::json!({}));
        let action = Action::delegate_tool(&tool_call).unwrap();

        let effect = executor.execute(&ctx, &action).await.unwrap();
        assert_eq!(effect.status, EffectStatus::Failed);

        let result: ToolResult = serde_json::from_slice(&effect.payload).unwrap();
        assert!(!result.ok);
    }

    #[tokio::test]
    async fn test_register_custom_tool() {
        let mut executor = ToolExecutor::with_defaults();
        assert!(executor.tools.contains_key("list_files"));
        assert!(!executor.tools.contains_key("custom_tool"));

        // Custom tools can be registered at runtime
        struct DummyTool;

        #[async_trait]
        impl Tool for DummyTool {
            fn name(&self) -> &str {
                "custom_tool"
            }
            fn definition(&self) -> aura_core::ToolDefinition {
                aura_core::ToolDefinition::new(
                    "custom_tool",
                    "A test tool",
                    serde_json::json!({"type": "object"}),
                )
            }
            async fn execute(
                &self,
                _ctx: &ToolContext,
                _args: serde_json::Value,
            ) -> Result<ToolResult, ToolError> {
                Ok(ToolResult::success("custom_tool", "ok"))
            }
        }

        executor.register(Box::new(DummyTool));
        assert!(executor.tools.contains_key("custom_tool"));
    }
}
