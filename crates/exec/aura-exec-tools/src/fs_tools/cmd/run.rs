//! Running a policy-checked `run_command` request: in a terminal, as a
//! shell script or as a direct program, confined when asked.

use super::args::RunArgs;
use super::background::run_in_background;
use super::pty;
use super::wait::{cmd_run_shell_script, run_program};
use super::ProcessTags;
use crate::error::ToolError;
use crate::fs_tools::spawn_blocking_tool;
use crate::sandbox::Sandbox;
use crate::tool::ToolContext;
use aura_core_types::ToolResult;
use aura_exec_traits::RunScratch;

/// Spawn the actual process via `spawn_blocking_tool`, dispatching to
/// either the shell-script or direct-execution path.
///
/// Precondition: `enforce_command_policy(&args, ctx)` returned `Ok`.
/// The defensive `ok_or_else` for `program` covers a future refactor
/// that forgets to call the policy helper; it surfaces the same
/// `InvalidArguments` message as the policy gate would.
pub(super) async fn execute_run_command(
    args: RunArgs,
    ctx: &ToolContext,
) -> Result<ToolResult, ToolError> {
    let tags = ProcessTags::for_context(ctx);
    if args.background {
        return run_in_background(
            ctx,
            args.shell_script.as_deref(),
            args.program.as_deref(),
            &args.cmd_args,
            args.cwd.as_deref(),
        );
    }

    if let Some(cols) = args.tty_cols {
        if let Some(result) = run_in_terminal(&args, ctx, tags.clone(), cols).await? {
            return result;
        }
    }

    run_over_pipes(args, ctx, tags).await
}

/// Run the shell script or program on a blocking thread with piped
/// output, confined when the context asks.
async fn run_over_pipes(
    args: RunArgs,
    ctx: &ToolContext,
    tags: ProcessTags,
) -> Result<ToolResult, ToolError> {
    let RunArgs {
        cwd,
        timeout_ms,
        shell_script,
        program,
        cmd_args,
        ..
    } = args;
    let sandbox = ctx.sandbox.clone();
    let confine = ctx.confine_writes;
    let scratch = ctx.run_scratch.clone();

    if let Some(script) = shell_script {
        return spawn_blocking_tool(move || {
            confined_if(confine, &sandbox, scratch.as_ref(), || {
                cmd_run_shell_script(&sandbox, &script, cwd.as_deref(), timeout_ms, &tags)
            })
        })
        .await;
    }

    let program = program.ok_or_else(|| {
        ToolError::InvalidArguments(
            "missing 'program' argument; use 'shell_script' for shell commands".into(),
        )
    })?;

    spawn_blocking_tool(move || {
        confined_if(confine, &sandbox, scratch.as_ref(), || {
            run_program(
                &sandbox,
                &program,
                &cmd_args,
                cwd.as_deref(),
                timeout_ms,
                &tags,
            )
        })
    })
    .await
}

/// Run the command in a pseudo-terminal `cols` wide, confined when the
/// context asks. `None` when it did not run there (no pseudo-terminal, or
/// nothing to run), so the caller falls back to pipes.
async fn run_in_terminal(
    args: &RunArgs,
    ctx: &ToolContext,
    tags: ProcessTags,
    cols: u16,
) -> Result<Option<Result<ToolResult, ToolError>>, ToolError> {
    let sandbox = ctx.sandbox.clone();
    let confine = ctx.confine_writes;
    let scratch = ctx.run_scratch.clone();
    let timeout_ms = args.timeout_ms;
    let (cwd, shell_script, program, cmd_args) = (
        args.cwd.clone(),
        args.shell_script.clone(),
        args.program.clone(),
        args.cmd_args.clone(),
    );
    tokio::task::spawn_blocking(move || {
        let command = match (&shell_script, &program) {
            (Some(script), _) => pty::PtyCommand::ShellScript(script),
            (None, Some(program)) => pty::PtyCommand::Program {
                program,
                args: &cmd_args,
            },
            (None, None) => return None,
        };
        let run =
            || pty::cmd_run_in_pty(&sandbox, &command, cwd.as_deref(), cols, timeout_ms, &tags);
        if confine {
            match crate::confine::run_confined(sandbox.root(), scratch.as_ref(), || Ok(run())) {
                Ok(result) => result,
                Err(e) => Some(Err(e)),
            }
        } else {
            run()
        }
    })
    .await
    .map_err(|e| ToolError::CommandFailed(format!("terminal task failed: {e}")))
}

/// Run `f`, on a thread confined to writing in the sandbox (and `scratch`
/// in place of the shared temp and cache folders) when `confine` is set
/// (see [`crate::confine`]).
pub(super) fn confined_if<T: Send>(
    confine: bool,
    sandbox: &Sandbox,
    scratch: Option<&RunScratch>,
    f: impl FnOnce() -> Result<T, ToolError> + Send,
) -> Result<T, ToolError> {
    if confine {
        crate::confine::run_confined(sandbox.root(), scratch, f)
    } else {
        f()
    }
}
