//! `run_command` with `background: true`: output to a log file and the
//! process handed to the context's background hook.

use super::run::confined_if;
use super::spawn::{program_command, shell_script_command};
use super::ProcessTags;
use crate::error::ToolError;
use crate::tool::ToolContext;
use aura_core_types::{ProcessId, ToolResult};
use std::fs::File;
use std::path::{Path, PathBuf};

/// Start the command with its output going to a log file and hand it to
/// the context's [`BackgroundProcessHook`](crate::BackgroundProcessHook).
pub(super) fn run_in_background(
    ctx: &ToolContext,
    shell_script: Option<&str>,
    program: Option<&str>,
    cmd_args: &[String],
    cwd: Option<&str>,
) -> Result<ToolResult, ToolError> {
    use std::process::Stdio;

    let hook = ctx.background_processes.as_ref().ok_or_else(|| {
        ToolError::InvalidArguments(
            "background commands are not available here; run it without 'background'".into(),
        )
    })?;
    let agent_id = ctx.caller_agent_id.ok_or_else(|| {
        ToolError::InvalidArguments("background commands need a calling agent".into())
    })?;
    let (mut cmd, command, working_dir) =
        background_command(ctx, shell_script, program, cmd_args, cwd)?;

    let process_id = aura_core_types::ProcessId::generate();
    let log_dir = hook.log_dir();
    let log_path = log_dir.join(format!("{}.log", process_id.to_hex()));
    let (log, stderr_log) = open_log(&log_dir, &log_path)?;
    ProcessTags::for_context(ctx).apply(&mut cmd);
    cmd.stdin(Stdio::null())
        .stdout(Stdio::from(log))
        .stderr(Stdio::from(stderr_log));
    let child = confined_if(
        ctx.confine_writes,
        &ctx.sandbox,
        ctx.run_scratch.as_ref(),
        || {
            cmd.spawn()
                .map_err(|e| ToolError::CommandFailed(format!("Failed to spawn '{command}': {e}")))
        },
    )?;
    let pid = child.id();

    hook.adopt(
        agent_id,
        crate::BackgroundProcess {
            process_id,
            child,
            command: command.clone(),
            cwd: working_dir,
            log_path: log_path.clone(),
            action_id: ctx.current_action_id,
            tool_use_id: ctx.current_tool_use_id.clone(),
        },
    )
    .map_err(|e| ToolError::CommandFailed(format!("tracking background command: {e}")))?;

    Ok(background_started(pid, &command, process_id, &log_path))
}

/// The command to start, its display string and its working directory.
fn background_command(
    ctx: &ToolContext,
    shell_script: Option<&str>,
    program: Option<&str>,
    cmd_args: &[String],
    cwd: Option<&str>,
) -> Result<(std::process::Command, String, PathBuf), ToolError> {
    match (shell_script, program) {
        (Some(script), _) => {
            let (cmd, dir) = shell_script_command(&ctx.sandbox, script, cwd)?;
            Ok((cmd, script.to_string(), dir))
        }
        (None, Some(program)) => program_command(&ctx.sandbox, program, cmd_args, cwd),
        (None, None) => Err(ToolError::InvalidArguments(
            "missing 'program' argument; use 'shell_script' for shell commands".into(),
        )),
    }
}

/// Create `log_path` (and `log_dir`), returning it opened for stdout and
/// again for stderr.
fn open_log(log_dir: &Path, log_path: &Path) -> Result<(File, File), ToolError> {
    let log = std::fs::create_dir_all(log_dir)
        .and_then(|()| File::create(log_path))
        .map_err(|e| {
            ToolError::CommandFailed(format!("creating log {}: {e}", log_path.display()))
        })?;
    let stderr_log = log
        .try_clone()
        .map_err(|e| ToolError::CommandFailed(format!("opening log for stderr: {e}")))?;
    Ok((log, stderr_log))
}

/// The result telling the agent the command started and where to look.
fn background_started(
    pid: u32,
    command: &str,
    process_id: ProcessId,
    log_path: &Path,
) -> ToolResult {
    let log = log_path.display().to_string();
    ToolResult::success(
        "run_command",
        format!(
            "Started in the background (pid {pid}): {command}\nJob id: {}\nOutput: {log}\n\
             You will get a message with the exit status when it ends. Check on it with \
             job_output and stop it with job_kill.",
            process_id.to_hex()
        ),
    )
    .with_metadata("background", "true")
    .with_metadata("process_id", process_id.to_hex())
    .with_metadata("pid", pid.to_string())
    .with_metadata("log_path", log)
}
