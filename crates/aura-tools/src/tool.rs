//! Extensible tool trait for trait-based dispatch.
//!
//! Each tool is a struct implementing [`Tool`], providing its name,
//! JSON schema definition, and execution logic. The [`ToolExecutor`](crate::ToolExecutor)
//! dispatches to tools via `HashMap` lookup instead of a hardcoded match.

use crate::error::ToolError;
use crate::sandbox::Sandbox;
use crate::ToolConfig;
use async_trait::async_trait;
use aura_core::{AgentId, AgentPermissions, ToolDefinition, ToolResult};

/// Context provided to tools during execution.
pub struct ToolContext {
    /// Sandbox for path validation and resolution.
    pub sandbox: Sandbox,
    /// Tool configuration (limits, permissions).
    pub config: ToolConfig,
    /// Phase 5: agent id of the caller that issued this tool call, when
    /// known. Cross-agent tools read this to populate parent-chain metadata
    /// on the resulting transaction.
    pub caller_agent_id: Option<AgentId>,
    /// Phase 5: caller's scope + capability grants. Cross-agent tools (e.g.
    /// `spawn_agent`) enforce strict-subset semantics against this bundle.
    pub caller_permissions: Option<AgentPermissions>,
    /// Phase 5: ancestor chain for the caller (immediate parent first, root
    /// last). Used for cycle prevention in `spawn_agent`.
    pub parent_chain: Vec<AgentId>,
    /// Phase 5: originating end-user id that began this delegate chain.
    /// Propagated onto every Delegate transaction for billing attribution.
    pub originating_user_id: Option<String>,
}

impl ToolContext {
    /// Construct a minimal context with only the fields required pre-phase-5.
    /// All new cross-agent fields default to `None` / empty.
    #[must_use]
    pub fn new(sandbox: Sandbox, config: ToolConfig) -> Self {
        Self {
            sandbox,
            config,
            caller_agent_id: None,
            caller_permissions: None,
            parent_chain: Vec::new(),
            originating_user_id: None,
        }
    }
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
