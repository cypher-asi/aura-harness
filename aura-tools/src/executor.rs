//! Tool executor implementation.

use crate::error::ToolError;
use crate::fs_tools;
use crate::sandbox::Sandbox;
use crate::ToolConfig;
use async_trait::async_trait;
use aura_core::{Action, ActionKind, Effect, EffectKind, EffectStatus, ToolCall, ToolResult};
use aura_executor::{ExecuteContext, Executor};
use bytes::Bytes;
use tracing::{debug, error, instrument, warn};

/// Tool executor for filesystem and command operations.
pub struct ToolExecutor {
    config: ToolConfig,
}

impl ToolExecutor {
    /// Create a new tool executor with the given config.
    #[must_use]
    pub const fn new(config: ToolConfig) -> Self {
        Self { config }
    }

    /// Create a tool executor with default config.
    #[must_use]
    pub fn with_defaults() -> Self {
        Self::new(ToolConfig::default())
    }

    /// Execute a tool call.
    #[instrument(skip(self, ctx), fields(tool = %tool_call.tool))]
    fn execute_tool(
        &self,
        ctx: &ExecuteContext,
        tool_call: &ToolCall,
    ) -> Result<ToolResult, ToolError> {
        let tool = &tool_call.tool;

        // Check if tool is enabled
        if tool.starts_with("fs_") && !self.config.enable_fs {
            return Err(ToolError::ToolDisabled(tool.clone()));
        }
        if tool.starts_with("cmd_") && !self.config.enable_commands {
            return Err(ToolError::ToolDisabled(tool.clone()));
        }

        // Create sandbox for this execution
        let sandbox = Sandbox::new(&ctx.workspace_root)?;

        match tool.as_str() {
            "fs_ls" => {
                let path = tool_call.args["path"]
                    .as_str()
                    .ok_or_else(|| ToolError::InvalidArguments("missing 'path' argument".into()))?;
                fs_tools::fs_ls(&sandbox, path)
            }
            "fs_read" => {
                let path = tool_call.args["path"]
                    .as_str()
                    .ok_or_else(|| ToolError::InvalidArguments("missing 'path' argument".into()))?;
                let max_bytes = tool_call.args["max_bytes"]
                    .as_u64()
                    .map_or(self.config.max_read_bytes, |n| {
                        usize::try_from(n).unwrap_or(usize::MAX)
                    });
                let max_bytes = max_bytes.min(self.config.max_read_bytes);
                fs_tools::fs_read(&sandbox, path, max_bytes)
            }
            "fs_stat" => {
                let path = tool_call.args["path"]
                    .as_str()
                    .ok_or_else(|| ToolError::InvalidArguments("missing 'path' argument".into()))?;
                fs_tools::fs_stat(&sandbox, path)
            }
            "fs_write" => {
                let path = tool_call.args["path"]
                    .as_str()
                    .ok_or_else(|| ToolError::InvalidArguments("missing 'path' argument".into()))?;
                let content = tool_call.args["content"]
                    .as_str()
                    .ok_or_else(|| ToolError::InvalidArguments("missing 'content' argument".into()))?;
                let create_dirs = tool_call.args["create_dirs"].as_bool().unwrap_or(false);
                fs_tools::fs_write(&sandbox, path, content, create_dirs)
            }
            "fs_edit" => {
                let path = tool_call.args["path"]
                    .as_str()
                    .ok_or_else(|| ToolError::InvalidArguments("missing 'path' argument".into()))?;
                let old_text = tool_call.args["old_text"]
                    .as_str()
                    .ok_or_else(|| ToolError::InvalidArguments("missing 'old_text' argument".into()))?;
                let new_text = tool_call.args["new_text"]
                    .as_str()
                    .ok_or_else(|| ToolError::InvalidArguments("missing 'new_text' argument".into()))?;
                fs_tools::fs_edit(&sandbox, path, old_text, new_text)
            }
            "search_code" => {
                let pattern = tool_call.args["pattern"]
                    .as_str()
                    .ok_or_else(|| ToolError::InvalidArguments("missing 'pattern' argument".into()))?;
                let path = tool_call.args["path"].as_str();
                let file_pattern = tool_call.args["file_pattern"].as_str();
                let max_results = tool_call.args["max_results"]
                    .as_u64()
                    .map_or(100, |n| usize::try_from(n).unwrap_or(100));
                fs_tools::search_code(&sandbox, pattern, path, file_pattern, max_results)
            }
            "cmd_run" => {
                // Commands are disabled by default and require explicit allowlisting
                if !self.config.enable_commands {
                    return Err(ToolError::ToolDisabled("cmd_run".into()));
                }

                let program = tool_call.args["program"].as_str().ok_or_else(|| {
                    ToolError::InvalidArguments("missing 'program' argument".into())
                })?;

                // Check allowlist if not empty
                if !self.config.command_allowlist.is_empty()
                    && !self.config.command_allowlist.contains(&program.to_string())
                {
                    return Err(ToolError::CommandNotAllowed(program.into()));
                }

                // Parse arguments
                let args: Vec<String> = tool_call.args["args"]
                    .as_array()
                    .map(|arr| {
                        arr.iter()
                            .filter_map(|v| v.as_str().map(String::from))
                            .collect()
                    })
                    .unwrap_or_default();

                // Optional working directory
                let cwd = tool_call.args["cwd"].as_str();

                // For backwards compatibility, use sync_threshold for the default timeout
                // The executor will use threshold-based execution in the future
                let timeout_ms = tool_call.args["timeout_ms"]
                    .as_u64()
                    .unwrap_or(self.config.sync_threshold_ms);

                fs_tools::cmd_run(&sandbox, program, &args, cwd, timeout_ms)
            }
            _ => Err(ToolError::UnknownTool(tool.clone())),
        }
    }
}

#[async_trait]
impl Executor for ToolExecutor {
    #[instrument(skip(self, ctx, action), fields(action_id = %action.action_id))]
    async fn execute(&self, ctx: &ExecuteContext, action: &Action) -> anyhow::Result<Effect> {
        // Parse tool call from action payload
        let tool_call: ToolCall = serde_json::from_slice(&action.payload)
            .map_err(|e| anyhow::anyhow!("Failed to parse tool call: {e}"))?;

        debug!(?tool_call, "Executing tool");

        match self.execute_tool(ctx, &tool_call) {
            Ok(result) => {
                let payload = serde_json::to_vec(&result)?;
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
                let payload = serde_json::to_vec(&result)?;
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
        // We handle Delegate actions with tool_call payloads
        if action.kind != ActionKind::Delegate {
            return false;
        }

        // Try to parse as ToolCall
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
        let action = Action::delegate_tool(&tool_call);

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
        let action = Action::delegate_tool(&tool_call);

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
        let action = Action::delegate_tool(&tool_call);

        let effect = executor.execute(&ctx, &action).await.unwrap();
        assert_eq!(effect.status, EffectStatus::Failed);

        let result: ToolResult = serde_json::from_slice(&effect.payload).unwrap();
        assert!(!result.ok);
    }

    #[tokio::test]
    async fn test_cmd_disabled() {
        let (ctx, _dir) = create_test_context();

        let executor = ToolExecutor::with_defaults();
        let tool_call = ToolCall::new("cmd_run", serde_json::json!({"program": "ls"}));
        let action = Action::delegate_tool(&tool_call);

        let effect = executor.execute(&ctx, &action).await.unwrap();
        assert_eq!(effect.status, EffectStatus::Failed);
    }
}
