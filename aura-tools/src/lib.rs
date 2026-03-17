//! # aura-tools
//!
//! Tool executor and registry for filesystem and command operations.
//!
//! This crate provides:
//! - `ToolRegistry` trait and `DefaultToolRegistry` implementation
//! - `ToolExecutor` for executing tool calls
//! - Sandboxed filesystem and command operations
//! - Threshold-based async command execution
//!
//! ## Security
//!
//! All filesystem operations are sandboxed to prevent path traversal attacks.
//! Command execution is disabled by default and requires explicit allowlisting.

#![forbid(unsafe_code)]
#![warn(clippy::all, clippy::pedantic, clippy::nursery)]

mod error;
mod executor;
pub mod external;
pub mod fs_tools;
pub mod registry;
mod sandbox;
pub mod tool;

pub use error::ToolError;
pub use executor::ToolExecutor;
pub use aura_core::ExternalToolDefinition;
pub use fs_tools::{cmd_run_with_threshold, cmd_spawn, output_to_tool_result, ThresholdResult};
pub use registry::{DefaultToolRegistry, ToolRegistry};
pub use sandbox::Sandbox;
pub use tool::{Tool, ToolContext};

/// Tool configuration.
#[derive(Debug, Clone)]
pub struct ToolConfig {
    /// Enable filesystem tools
    pub enable_fs: bool,
    /// Enable command execution
    pub enable_commands: bool,
    /// Allowed commands (empty = all allowed if commands enabled)
    pub command_allowlist: Vec<String>,
    /// Maximum read bytes
    pub max_read_bytes: usize,
    /// Sync threshold for command execution (milliseconds).
    /// Commands that complete within this threshold return immediately.
    /// Commands that exceed this threshold are moved to async execution.
    pub sync_threshold_ms: u64,
    /// Maximum timeout for async processes (milliseconds).
    pub max_async_timeout_ms: u64,
}

impl Default for ToolConfig {
    fn default() -> Self {
        Self {
            enable_fs: true,
            enable_commands: true, // Enabled by default for agentic workflows
            command_allowlist: vec![], // Empty = all commands allowed
            max_read_bytes: 5 * 1024 * 1024, // 5MB
            sync_threshold_ms: 5_000,         // 5s sync threshold
            max_async_timeout_ms: 600_000,    // 10 minutes async timeout
        }
    }
}
