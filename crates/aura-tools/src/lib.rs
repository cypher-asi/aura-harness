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
#![warn(clippy::all)]
#![allow(
    clippy::missing_errors_doc,
    clippy::missing_const_for_fn,
    clippy::must_use_candidate,
    clippy::unnecessary_literal_bound,
    clippy::option_if_let_else,
    clippy::doc_markdown
)]

pub mod agents;
pub mod automaton_tools;
pub mod catalog;
pub(crate) mod definitions;
pub mod domain_tools;
mod error;
mod executor;
pub(crate) mod fs_tools;
pub mod git_tool;
pub mod http_tool;
pub mod intent_classifier;
pub(crate) mod registry;
pub mod resolver;
mod sandbox;
pub mod schema;
pub(crate) mod tool;

pub use catalog::ToolCatalog;
pub use error::ToolError;
pub use executor::ToolExecutor;
pub use fs_tools::{cmd_run_with_threshold, cmd_spawn, output_to_tool_result, ThresholdResult};
pub use git_tool::{GitCommitPushTool, GitCommitTool, GitPushTool, GIT_TOOL_NAMES};
pub use http_tool::{HttpAuthSource, HttpMethod, HttpToolDefinition, HttpToolDefinitionBuilder};
pub use intent_classifier::{ClassifierError, IntentClassifier};
pub use registry::{DefaultToolRegistry, ToolRegistry};
pub use resolver::ToolResolver;
pub use sandbox::Sandbox;
pub use schema::{from_claude_json, to_claude_json, SchemaError};
pub use tool::{AgentControlHook, AgentReadHook, Tool, ToolContext};

/// Tool configuration.
#[derive(Debug, Clone)]
pub struct ToolConfig {
    /// Enable filesystem tools
    pub enable_fs: bool,
    /// Enable command execution
    pub enable_commands: bool,
    /// Allowed commands (empty = all allowed if commands enabled)
    pub command_allowlist: Vec<String>,
    /// Allowed binary names for `run_command`.
    ///
    /// Unlike [`Self::command_allowlist`], which matches the first whitespace
    /// token of the full shell string, this list is checked **after**
    /// resolving `program` through `which`, so it guards against PATH
    /// shadowing tricks (e.g. a malicious `rg` shim dropped next to
    /// `cargo`).
    ///
    /// Empty vec = no binary allow-list enforcement (backwards compatible).
    /// Any non-empty list causes `run_command` to reject programs whose
    /// resolved file name is not present. (Wave 5 / T3.2.)
    pub binary_allowlist: Vec<String>,
    /// When `false` (default), `run_command` refuses the legacy
    /// "empty args treated as shell script" form. Callers must then
    /// supply `program` + non-empty `args`, avoiding the shell-injection
    /// surface that made `command: "git status; rm -rf"` executable.
    /// (Wave 5 / T3.1.)
    pub allow_shell: bool,
    /// Allow-list of verbatim shell scripts permitted via the
    /// `shell_script` field of `run_command`. Used together with
    /// [`Self::allow_shell`] == `true` Ã¢â‚¬â€ the caller explicitly opts into
    /// a shell interpreter by passing a script, and that script MUST be
    /// present in this list for the invocation to be accepted.
    ///
    /// Empty (default) means no shell scripts are permitted, so the
    /// `shell_script` field is effectively disabled until an operator
    /// populates it. This guards against the historic shell-injection
    /// path where an attacker-controlled `program` string was routed
    /// through `sh -c` / `cmd.exe /C`. (Phase 2 hardening.)
    pub allowed_shell_scripts: Vec<String>,
    /// Maximum read bytes
    pub max_read_bytes: usize,
    /// Sync threshold for command execution (milliseconds).
    /// Commands that complete within this threshold return immediately.
    /// Commands that exceed this threshold are moved to async execution.
    pub sync_threshold_ms: u64,
    /// Maximum timeout for async processes (milliseconds).
    pub max_async_timeout_ms: u64,
    /// Per-attempt timeout for `git push` (milliseconds).
    ///
    /// Push is a network operation against Orbit and is routinely
    /// slower than `git add` / `git commit`, which run entirely
    /// locally. Keeping it on its own knob lets slow pushes finish
    /// without having to raise every other async command's ceiling,
    /// and decouples push tolerance from the workspace-wide
    /// `max_async_timeout_ms` cap (clamped to 120s for every other
    /// git operation inside `git_tool::workspace_timeout`).
    pub git_push_timeout_ms: u64,
    /// Number of `git push` attempts, including the first. Values
    /// below 1 are coerced to 1 at the call-site. Each retry waits
    /// an exponentially increasing backoff (2s, 5s, 15s) before the
    /// next attempt; only timeouts and transient network errors
    /// (`could not read from remote`, `RPC failed`, `early EOF`,
    /// `connection reset`) are retried — auth failures and
    /// non-fast-forward rejections short-circuit immediately.
    pub git_push_attempts: u32,
    /// Extra filesystem paths to allow beyond the workspace root.
    /// Granted by skill permissions at runtime.
    pub extra_allowed_paths: Vec<std::path::PathBuf>,
}

impl Default for ToolConfig {
    /// Fail-closed defaults: filesystem tools are on, but command
    /// execution is off and every shell / script hook is empty. An
    /// operator who wants `run_command` must set
    /// `enable_commands: true` **and** populate `binary_allowlist`
    /// with the specific binaries they trust. Leaving either field
    /// at the default value keeps `run_command` completely inert,
    /// even if a delegate proposal reaches [`CmdRunTool::execute`].
    /// (Phase 5 hardening Ã¢â‚¬â€ closes finding M1.)
    fn default() -> Self {
        Self {
            enable_fs: true,
            enable_commands: false,
            command_allowlist: vec![],
            binary_allowlist: vec![],
            allow_shell: false,
            allowed_shell_scripts: vec![],
            max_read_bytes: 5 * 1024 * 1024,
            sync_threshold_ms: 5_000,
            max_async_timeout_ms: 600_000,
            git_push_timeout_ms: 300_000,
            git_push_attempts: 3,
            extra_allowed_paths: vec![],
        }
    }
}

#[cfg(test)]
mod default_tests {
    use super::ToolConfig;

    #[test]
    fn default_config_disables_commands() {
        let cfg = ToolConfig::default();
        assert!(
            !cfg.enable_commands,
            "fresh ToolConfig must start with commands disabled"
        );
        assert!(
            cfg.binary_allowlist.is_empty(),
            "fresh ToolConfig must have an empty binary_allowlist"
        );
        assert!(
            cfg.command_allowlist.is_empty(),
            "fresh ToolConfig must have an empty command_allowlist"
        );
        assert!(
            !cfg.allow_shell,
            "fresh ToolConfig must not allow shell scripts"
        );
        assert!(
            cfg.allowed_shell_scripts.is_empty(),
            "fresh ToolConfig must have an empty allowed_shell_scripts"
        );
    }
}
