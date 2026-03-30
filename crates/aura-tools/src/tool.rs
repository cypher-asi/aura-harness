//! Extensible tool trait for trait-based dispatch.
//!
//! Each tool is a struct implementing [`Tool`], providing its name,
//! JSON schema definition, and execution logic. The [`ToolExecutor`](crate::ToolExecutor)
//! dispatches to tools via `HashMap` lookup instead of a hardcoded match.

use crate::error::ToolError;
use crate::sandbox::Sandbox;
use crate::ToolConfig;
use async_trait::async_trait;
use aura_core::ToolDefinition;
use aura_core::ToolResult;

/// Context provided to tools during execution.
pub struct ToolContext {
    /// Sandbox for path validation and resolution.
    pub sandbox: Sandbox,
    /// Tool configuration (limits, permissions).
    pub config: ToolConfig,
}

/// Trait for extensible tool implementations.
///
/// The `ToolExecutor` holds a `HashMap<String, Box<dyn Tool>>` and dispatches
/// calls by name lookup. Built-in tools and external tools both implement
/// this trait.
#[async_trait]
pub trait Tool: Send + Sync {
    /// Unique tool name used for dispatch (e.g., "read_file", "run_command").
    fn name(&self) -> &str;

    /// JSON schema definition sent to the model.
    fn definition(&self) -> ToolDefinition;

    /// Execute the tool with parsed arguments.
    async fn execute(
        &self,
        ctx: &ToolContext,
        args: serde_json::Value,
    ) -> Result<ToolResult, ToolError>;
}

/// Returns all built-in tool instances.
pub fn builtin_tools() -> Vec<Box<dyn Tool>> {
    use crate::fs_tools::{
        CmdRunTool, FsDeleteTool, FsEditTool, FsFindTool, FsLsTool, FsReadTool, FsStatTool,
        FsWriteTool, SearchCodeTool,
    };

    vec![
        Box::new(FsLsTool),
        Box::new(FsReadTool),
        Box::new(FsStatTool),
        Box::new(FsWriteTool),
        Box::new(FsEditTool),
        Box::new(FsDeleteTool),
        Box::new(FsFindTool),
        Box::new(SearchCodeTool),
        Box::new(CmdRunTool),
    ]
}

/// Returns only read-only built-in tool instances.
pub fn read_only_builtin_tools() -> Vec<Box<dyn Tool>> {
    use crate::fs_tools::{FsLsTool, FsReadTool, FsStatTool, SearchCodeTool};

    vec![
        Box::new(FsLsTool),
        Box::new(FsReadTool),
        Box::new(FsStatTool),
        Box::new(SearchCodeTool),
    ]
}
