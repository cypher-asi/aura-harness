//! Extensible tool trait for trait-based dispatch.
//!
//! Each tool is a struct implementing [`Tool`], providing its name,
//! JSON schema definition, and execution logic. The [`ToolExecutor`](crate::ToolExecutor)
//! dispatches to tools via `HashMap` lookup instead of a hardcoded match.

use crate::error::ToolError;
use crate::sandbox::Sandbox;
use crate::ToolConfig;
use async_trait::async_trait;
use aura_core::{
    AgentId, AgentPermissions, AgentToolPermissions, Capability, ToolDefinition, ToolResult,
    UserToolDefaults,
};
use aura_kernel::SpawnHook;
use std::sync::Arc;

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
    /// Caller per-agent tool override, when present for this session.
    pub caller_tool_permissions: Option<AgentToolPermissions>,
    /// User default used with `caller_tool_permissions` for monotonic spawn
    /// checks.
    pub user_tool_defaults: UserToolDefaults,
    /// Phase 5: ancestor chain for the caller (immediate parent first, root
    /// last). Used for cycle prevention in `spawn_agent`.
    pub parent_chain: Vec<AgentId>,
    /// Phase 5: originating end-user id that began this delegate chain.
    /// Propagated onto every Delegate transaction for billing attribution.
    pub originating_user_id: Option<String>,
    /// Phase 5 part 2: optional spawn hook used by the `spawn_agent` tool to
    /// actually persist a new child agent. `None` means "no hook wired" â€” the
    /// tool returns a pure outcome payload without touching a store.
    pub spawn_hook: Option<Arc<dyn SpawnHook>>,
    /// Phase 5 part 2: optional cross-agent control hook used by
    /// `send_to_agent`, `agent_lifecycle`, and `delegate_task` to deliver
    /// effects to the target agent's record log. `None` means the tool
    /// short-circuits into a permission-checked outcome with no runtime
    /// side-effect (the production wiring is a `TODO(phase5-runtime)`).
    pub agent_control_hook: Option<Arc<dyn AgentControlHook>>,
    /// Phase 5 part 2: optional read hook used by `get_agent_state` to
    /// fetch a snapshot of a target agent's record log.
    pub agent_read_hook: Option<Arc<dyn AgentReadHook>>,
}

/// Hook invoked by the `send_to_agent` / `agent_lifecycle` / `delegate_task`
/// tools to actually affect the target agent. Kept as a trait so the
/// permission gate can be tested without wiring a real kernel writer.
///
/// Production wiring of this hook is deferred â€” see
/// `TODO(phase5-runtime)` in `agents/` for the runtime effects.
#[async_trait]
pub trait AgentControlHook: Send + Sync {
    /// Deliver a user-message-shaped payload to `target`.
    async fn deliver_message(
        &self,
        target: &AgentId,
        parent: &AgentId,
        originating_user_id: Option<&str>,
        content: &str,
        attachments: Option<serde_json::Value>,
    ) -> Result<(), String>;

    /// Apply a lifecycle transition to `target`.
    async fn lifecycle(
        &self,
        target: &AgentId,
        parent: &AgentId,
        originating_user_id: Option<&str>,
        action: &str,
    ) -> Result<(), String>;

    /// Emit a Delegate-tagged task to `target`.
    async fn delegate_task(
        &self,
        target: &AgentId,
        parent: &AgentId,
        originating_user_id: Option<&str>,
        task: &str,
        context: Option<&serde_json::Value>,
    ) -> Result<(), String>;
}

/// Hook used by `get_agent_state` to fetch a read-only snapshot of a
/// target agent. Kept as a trait so the gate is testable without a kernel.
#[async_trait]
pub trait AgentReadHook: Send + Sync {
    /// Return the latest `session_ready` / `assistant_message_end`
    /// snapshot for `target`, plus the agent's `Identity` + `permissions`.
    async fn snapshot(&self, target: &AgentId) -> Result<serde_json::Value, String>;
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
            caller_tool_permissions: None,
            user_tool_defaults: UserToolDefaults::default(),
            parent_chain: Vec::new(),
            originating_user_id: None,
            spawn_hook: None,
            agent_control_hook: None,
            agent_read_hook: None,
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

    /// Phase 5: capabilities required on the caller's `AgentPermissions` for
    /// this tool to be visible + callable. Default is empty (tool is
    /// universally visible, matching pre-phase-5 behavior).
    ///
    /// `ToolCatalog::visible_tools` filters the catalog against this set;
    /// the kernel `Policy` layer additionally enforces it at proposal time
    /// via `PolicyConfig::tool_capability_requirements`.
    fn required_capabilities(&self) -> Vec<Capability> {
        Vec::new()
    }
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
        Box::new(crate::git_tool::GitCommitTool),
        Box::new(crate::git_tool::GitPushTool),
        Box::new(crate::git_tool::GitCommitPushTool),
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

#[cfg(test)]
mod builtin_policy_coverage {
    use super::builtin_tools;
    use crate::GIT_TOOL_NAMES;
    use aura_kernel::PolicyConfig;
    use std::collections::HashSet;

    /// Catch-all invariant: every name in [`builtin_tools`] must be
    /// reachable through one of the kernel-policy allow-listing
    /// surfaces the harness wires up at session start. Otherwise the
    /// kernel's fail-closed `allow_unlisted = false` default denies
    /// the tool with `"Tool 'X' is not allowed"` even though the
    /// resolver and the LLM tool schema both have it — the exact bug
    /// fixed for the dev-loop tools (commit 648fbe2) and the git
    /// tools (this commit).
    ///
    /// Reachable surfaces, in priority order:
    ///
    /// 1. `PolicyConfig::default().allowed_tools` — the static
    ///    baseline (file tools).
    /// 2. [`GIT_TOOL_NAMES`] — added by `aura-node`'s
    ///    `build_kernel_with_config` for chat sessions and by
    ///    `automaton_bridge::dev_loop_git_permissions` (as
    ///    `AlwaysAllow`) for dev-loop sessions.
    /// 3. `run_command` — added by
    ///    `runtime_capabilities::fetch_agent_permissions_with_default`
    ///    via the agent-permission fallback matrix (seeded as
    ///    `AlwaysAllow` when strict_mode is off).
    ///
    /// Adding a new built-in tool below without slotting it into one
    /// of these three surfaces will fail this test instead of
    /// shipping a silently-denied tool.
    #[test]
    fn every_builtin_tool_name_has_a_policy_allow_path() {
        let baseline: HashSet<String> = PolicyConfig::default()
            .allowed_tools
            .iter()
            .cloned()
            .collect();

        let git: HashSet<String> = GIT_TOOL_NAMES.iter().map(|s| s.to_string()).collect();

        // `run_command` is special-cased by `runtime_capabilities`
        // (see `fetch_agent_permissions_with_default`); list it
        // explicitly here so the assertion below stays a pure set
        // check rather than reaching into that helper.
        let run_command_fallback: HashSet<String> =
            std::iter::once("run_command".to_string()).collect();

        let covered: HashSet<String> = baseline
            .union(&git)
            .cloned()
            .collect::<HashSet<_>>()
            .union(&run_command_fallback)
            .cloned()
            .collect();

        let uncovered: Vec<String> = builtin_tools()
            .iter()
            .map(|t| t.name().to_string())
            .filter(|name| !covered.contains(name))
            .collect();

        assert!(
            uncovered.is_empty(),
            "builtin_tools() returned tool names with no kernel-policy allow path: {uncovered:?}. \
             Either add them to `PolicyConfig::default().allowed_tools`, extend \
             `GIT_TOOL_NAMES`, or wire a new allow-listing surface into \
             `aura-node`'s `build_kernel_with_config`. Otherwise the kernel will \
             deny the tool with `\"Tool '{}' is not allowed\"` at runtime.",
            uncovered.first().map(String::as_str).unwrap_or("X")
        );
    }
}
