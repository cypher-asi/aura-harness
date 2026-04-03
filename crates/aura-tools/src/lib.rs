//! # aura-tools
//!
//! Tool executor and registry for filesystem and command operations.
//!
//! This crate provides:
//! - `ToolRegistry` trait and `DefaultToolRegistry` implementation
//! - `ToolResolver` for unified tool dispatch (built-in + domain)
//! - Sandboxed filesystem and command operations
//! - Threshold-based async command execution
//!
//! ## Security
//!
//! All filesystem operations are sandboxed to prevent path traversal attacks.
//! Command execution is disabled by default and requires explicit allowlisting.

#![forbid(unsafe_code)]
#![warn(clippy::all, clippy::pedantic, clippy::nursery)]
#![allow(
    clippy::missing_errors_doc,
    clippy::missing_const_for_fn,
    clippy::must_use_candidate,
    clippy::unnecessary_literal_bound,
    clippy::option_if_let_else,
    clippy::doc_markdown
)]

pub mod automaton_tools;
pub mod catalog;
pub(crate) mod definitions;
pub mod domain_tools;
mod error;
mod executor;
pub(crate) mod fs_tools;
pub(crate) mod registry;
pub mod resolver;
mod sandbox;
pub(crate) mod tool;

pub use catalog::ToolCatalog;
pub use error::ToolError;
pub use executor::ToolExecutor;
pub use fs_tools::{cmd_run_with_threshold, cmd_spawn, output_to_tool_result, ThresholdResult};
pub use registry::{DefaultToolRegistry, ToolRegistry};
pub use resolver::ToolResolver;
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
    /// Extra filesystem paths to allow beyond the workspace root.
    /// Granted by skill permissions at runtime.
    pub extra_allowed_paths: Vec<std::path::PathBuf>,
}

impl Default for ToolConfig {
    fn default() -> Self {
        Self {
            enable_fs: true,
            enable_commands: true,
            command_allowlist: vec![],
            max_read_bytes: 5 * 1024 * 1024,
            sync_threshold_ms: 5_000,
            max_async_timeout_ms: 600_000,
            extra_allowed_paths: vec![],
        }
    }
}
