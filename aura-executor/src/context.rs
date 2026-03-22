//! Execution context for executors.

use aura_core::{ActionId, AgentId};
use std::path::PathBuf;
use std::time::Duration;

/// Context provided to executors when executing an action.
#[derive(Debug, Clone)]
pub struct ExecuteContext {
    /// The agent executing the action
    pub agent_id: AgentId,
    /// The action being executed
    pub action_id: ActionId,
    /// Workspace root for this agent (sandbox root for tools)
    pub workspace_root: PathBuf,
    /// Configuration limits
    pub limits: ExecuteLimits,
}

/// Execution limits enforced by the executor.
#[derive(Debug, Clone)]
pub struct ExecuteLimits {
    /// Maximum bytes to read from files
    pub read_bytes: usize,
    /// Maximum bytes to write to files
    pub write_bytes: usize,
    /// Maximum command execution time
    pub command_timeout: Duration,
    /// Maximum stdout bytes from commands
    pub stdout_bytes: usize,
    /// Maximum stderr bytes from commands
    pub stderr_bytes: usize,
}

impl Default for ExecuteLimits {
    fn default() -> Self {
        Self {
            read_bytes: 5 * 1024 * 1024, // 5MB
            write_bytes: 1024 * 1024,    // 1MB
            command_timeout: Duration::from_secs(10),
            stdout_bytes: 256 * 1024, // 256KB
            stderr_bytes: 256 * 1024, // 256KB
        }
    }
}

impl ExecuteContext {
    /// Create a new execution context.
    #[must_use]
    pub fn new(agent_id: AgentId, action_id: ActionId, workspace_root: PathBuf) -> Self {
        Self {
            agent_id,
            action_id,
            workspace_root,
            limits: ExecuteLimits::default(),
        }
    }

    /// Set custom limits.
    #[must_use]
    pub const fn with_limits(mut self, limits: ExecuteLimits) -> Self {
        self.limits = limits;
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn test_execute_limits_default() {
        let limits = ExecuteLimits::default();

        assert_eq!(limits.read_bytes, 5 * 1024 * 1024); // 5MB
        assert_eq!(limits.write_bytes, 1024 * 1024); // 1MB
        assert_eq!(limits.command_timeout, Duration::from_secs(10));
        assert_eq!(limits.stdout_bytes, 256 * 1024); // 256KB
        assert_eq!(limits.stderr_bytes, 256 * 1024); // 256KB
    }

    #[test]
    fn test_execute_context_new() {
        let agent_id = AgentId::generate();
        let action_id = ActionId::generate();
        let workspace = PathBuf::from("/tmp/workspace");

        let ctx = ExecuteContext::new(agent_id, action_id, workspace.clone());

        assert_eq!(ctx.agent_id, agent_id);
        assert_eq!(ctx.action_id, action_id);
        assert_eq!(ctx.workspace_root, workspace);
        // Should have default limits
        assert_eq!(ctx.limits.read_bytes, ExecuteLimits::default().read_bytes);
    }

    #[test]
    fn test_execute_context_with_limits() {
        let agent_id = AgentId::generate();
        let action_id = ActionId::generate();
        let workspace = PathBuf::from("/tmp/workspace");

        let custom_limits = ExecuteLimits {
            read_bytes: 1024,
            write_bytes: 512,
            command_timeout: Duration::from_secs(5),
            stdout_bytes: 100,
            stderr_bytes: 100,
        };

        let ctx =
            ExecuteContext::new(agent_id, action_id, workspace).with_limits(custom_limits.clone());

        assert_eq!(ctx.limits.read_bytes, 1024);
        assert_eq!(ctx.limits.write_bytes, 512);
        assert_eq!(ctx.limits.command_timeout, Duration::from_secs(5));
        assert_eq!(ctx.limits.stdout_bytes, 100);
        assert_eq!(ctx.limits.stderr_bytes, 100);
    }

    #[test]
    fn test_execute_context_clone() {
        let agent_id = AgentId::generate();
        let action_id = ActionId::generate();
        let workspace = PathBuf::from("/tmp/workspace");

        let ctx1 = ExecuteContext::new(agent_id, action_id, workspace);
        let ctx2 = ctx1.clone();

        assert_eq!(ctx1.agent_id, ctx2.agent_id);
        assert_eq!(ctx1.action_id, ctx2.action_id);
        assert_eq!(ctx1.workspace_root, ctx2.workspace_root);
    }

    #[test]
    fn test_execute_limits_clone() {
        let limits1 = ExecuteLimits {
            read_bytes: 100,
            write_bytes: 50,
            command_timeout: Duration::from_millis(500),
            stdout_bytes: 10,
            stderr_bytes: 10,
        };
        let limits2 = limits1.clone();

        assert_eq!(limits1.read_bytes, limits2.read_bytes);
        assert_eq!(limits1.command_timeout, limits2.command_timeout);
    }

    #[test]
    fn test_execute_context_workspace_root_preserved() {
        let workspace = PathBuf::from("/custom/workspace/path");
        let ctx = ExecuteContext::new(AgentId::generate(), ActionId::generate(), workspace.clone());
        assert_eq!(ctx.workspace_root, workspace);
    }

    #[test]
    fn test_execute_context_with_limits_replaces_defaults() {
        let custom_limits = ExecuteLimits {
            read_bytes: 42,
            write_bytes: 42,
            command_timeout: Duration::from_millis(42),
            stdout_bytes: 42,
            stderr_bytes: 42,
        };

        let ctx = ExecuteContext::new(
            AgentId::generate(),
            ActionId::generate(),
            PathBuf::from("/tmp"),
        )
        .with_limits(custom_limits);

        assert_eq!(ctx.limits.read_bytes, 42);
        assert_eq!(ctx.limits.write_bytes, 42);
        assert_eq!(ctx.limits.command_timeout, Duration::from_millis(42));
        assert_eq!(ctx.limits.stdout_bytes, 42);
        assert_eq!(ctx.limits.stderr_bytes, 42);
    }

    #[test]
    fn test_execute_context_debug_impl() {
        let ctx = ExecuteContext::new(
            AgentId::generate(),
            ActionId::generate(),
            PathBuf::from("/tmp"),
        );
        let debug_str = format!("{ctx:?}");
        assert!(debug_str.contains("ExecuteContext"));
        assert!(debug_str.contains("workspace_root"));
    }
}
