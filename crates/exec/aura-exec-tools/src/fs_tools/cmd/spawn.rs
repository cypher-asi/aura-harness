//! Building and spawning the processes `run_command` runs: the direct
//! (no shell) path, the opt-in shell-script path, and the environment
//! every child gets.

use super::policy::validate_program_name;
use super::ProcessTags;
use crate::error::ToolError;
use crate::sandbox::Sandbox;
use tracing::{debug, instrument};

#[cfg(windows)]
use super::windows::{refresh_system_path, windows_resolve_program};

/// Spawn a command directly, WITHOUT routing through a shell interpreter.
///
/// `program` is validated via [`validate_program_name`] so that shell
/// metacharacters cannot sneak in. `args` is passed verbatim to
/// `Command::args` — individual arguments may contain arbitrary bytes
/// (including spaces, pipes, semicolons) because they are never
/// re-parsed by a shell.
///
/// Callers that genuinely need a shell (multi-command pipelines, glob
/// expansion, etc.) must use [`cmd_spawn_shell_script`] and funnel
/// through `ToolConfig::command.allowed_shell_scripts`.
///
/// Returns the [`Child`](std::process::Child) and a display-only string
/// suitable for audit logs. The string is not executable by itself.
#[instrument(skip(sandbox), fields(program = %program))]
pub fn cmd_spawn(
    sandbox: &Sandbox,
    program: &str,
    args: &[String],
    cwd: Option<&str>,
) -> Result<(std::process::Child, String), ToolError> {
    spawn_program(sandbox, program, args, cwd, &ProcessTags::default())
}

/// [`cmd_spawn`] with `tags` in the environment.
pub(super) fn spawn_program(
    sandbox: &Sandbox,
    program: &str,
    args: &[String],
    cwd: Option<&str>,
    tags: &ProcessTags,
) -> Result<(std::process::Child, String), ToolError> {
    use std::process::Stdio;

    let (mut cmd, display, _working_dir) = program_command(sandbox, program, args, cwd)?;
    tags.apply(&mut cmd);
    cmd.stdout(Stdio::piped()).stderr(Stdio::piped());
    let child = cmd.spawn().map_err(|e| {
        ToolError::CommandFailed(format!("Failed to spawn command '{program}': {e}"))
    })?;
    Ok((child, display))
}

/// The [`Command`](std::process::Command) [`cmd_spawn`] runs, without its
/// stdio set, plus its display string and working directory.
pub(super) fn program_command(
    sandbox: &Sandbox,
    program: &str,
    args: &[String],
    cwd: Option<&str>,
) -> Result<(std::process::Command, String, std::path::PathBuf), ToolError> {
    use std::process::Command;

    validate_program_name(program)?;

    let working_dir = match cwd {
        Some(dir) => sandbox.resolve_existing(dir)?,
        None => sandbox.root().to_path_buf(),
    };

    debug!(
        ?working_dir,
        arg_count = args.len(),
        "Spawning command (no shell)"
    );

    let display = if args.is_empty() {
        program.to_string()
    } else {
        format!("{} {}", program, args.join(" "))
    };

    // On Windows, Rust's `Command::new("npm")` calls `CreateProcessW`
    // after a PATH search that does NOT honour `PATHEXT`, so shims that
    // ship as `.cmd` / `.bat` (npm, yarn, pnpm, ng, vite, eslint,
    // prettier, …) all fail with `program not found`. Resolve the bare
    // name through `which` first so we feed `Command` an absolute path
    // and the child process is launched directly without `cmd.exe`
    // re-parsing argv (preserves the no-shell guarantee that protects
    // arguments containing spaces, semicolons, etc.).
    #[cfg(windows)]
    let fresh_path = refresh_system_path();

    let spawn_target: std::ffi::OsString = {
        #[cfg(windows)]
        {
            windows_resolve_program(program, fresh_path.as_deref(), &working_dir)
                .map_or_else(|| program.into(), std::path::PathBuf::into_os_string)
        }
        #[cfg(not(windows))]
        {
            program.into()
        }
    };

    let mut cmd = Command::new(&spawn_target);
    cmd.args(args);
    configure_process_group(&mut cmd);

    #[cfg(windows)]
    {
        if let Some(ref fresh_path) = fresh_path {
            cmd.env("PATH", fresh_path);
        }
        cmd.env("PYTHONUTF8", "1");
        cmd.env("PYTHONIOENCODING", "utf-8");
    }

    // Strip ANSI color and progress-bar output from spawned tools.
    // Cargo, ripgrep, ls, etc. all inherit these, keeping captured
    // stdout/stderr free of escape sequences and carriage returns so
    // it renders cleanly when the harness forwards it to the LLM and
    // then on into the main chat.
    apply_plain_output_env(&mut cmd);
    // A child reading the harness's stdin would steal keystrokes from the
    // terminal UI; agent commands are never interactive.
    cmd.current_dir(&working_dir)
        .stdin(std::process::Stdio::null());
    Ok((cmd, display, working_dir))
}

/// Disable colored / progress output for spawned child processes, and
/// cap Cargo's build parallelism.
///
/// Applied uniformly to every command the harness launches so we never
/// have to worry about ANSI escape sequences or `\r`-driven progress
/// bars leaking into captured stdout/stderr.
fn apply_plain_output_env(cmd: &mut std::process::Command) {
    cmd.env("CARGO_TERM_COLOR", "never");
    cmd.env("CARGO_TERM_PROGRESS_WHEN", "never");
    cmd.env("NO_COLOR", "1");
    cmd.env("CLICOLOR", "0");
    cmd.env("CLICOLOR_FORCE", "0");
    cmd.env("TERM", "dumb");
    if let Some(jobs) = build_jobs_limit() {
        cmd.env(BUILD_JOBS_VAR, jobs);
    }
}

pub(super) const BUILD_JOBS_VAR: &str = "CARGO_BUILD_JOBS";

/// Half the CPUs for Cargo unless the user picked a job count, so a build
/// started by one agent leaves room for the UI and other agents.
pub(super) fn build_jobs_limit() -> Option<String> {
    if std::env::var_os(BUILD_JOBS_VAR).is_some() {
        return None;
    }
    let cpus = std::thread::available_parallelism().map_or(2, std::num::NonZeroUsize::get);
    Some((cpus / 2).max(1).to_string())
}

/// Isolate each command tool invocation from the harness process group.
///
/// Hard timeouts need to terminate shells and their grandchildren
/// together. On Unix, setting the child process group to the child's pid
/// lets `terminate_child_process_group` kill the whole tree without
/// touching the harness. Windows currently falls back to `Child::kill`.
#[cfg(unix)]
fn configure_process_group(cmd: &mut std::process::Command) {
    use std::os::unix::process::CommandExt;
    cmd.process_group(0);
}

#[cfg(not(unix))]
fn configure_process_group(_cmd: &mut std::process::Command) {}

/// Spawn a raw shell script under `sh -c` / `cmd.exe /C`.
///
/// This is the ONLY code path that exposes a shell interpreter to
/// caller-controlled strings. It is gated by the tool layer behind an
/// explicit `ToolConfig::command.allowed_shell_scripts`
/// allow-list; do not call it from other places without applying the
/// same gate.
#[instrument(skip(sandbox), fields(shell_script = %shell_script))]
pub(super) fn cmd_spawn_shell_script(
    sandbox: &Sandbox,
    shell_script: &str,
    cwd: Option<&str>,
    tags: &ProcessTags,
) -> Result<(std::process::Child, String), ToolError> {
    use std::process::Stdio;

    let (mut cmd, _working_dir) = shell_script_command(sandbox, shell_script, cwd)?;
    tags.apply(&mut cmd);
    cmd.stdout(Stdio::piped()).stderr(Stdio::piped());
    let child = cmd
        .spawn()
        .map_err(|e| ToolError::CommandFailed(format!("Failed to spawn shell script: {e}")))?;
    Ok((child, shell_script.to_string()))
}

/// The [`Command`](std::process::Command) [`cmd_spawn_shell_script`] runs,
/// without its stdio set, plus its working directory.
pub(super) fn shell_script_command(
    sandbox: &Sandbox,
    shell_script: &str,
    cwd: Option<&str>,
) -> Result<(std::process::Command, std::path::PathBuf), ToolError> {
    use std::process::Command;

    let working_dir = match cwd {
        Some(dir) => sandbox.resolve_existing(dir)?,
        None => sandbox.root().to_path_buf(),
    };

    debug!(?working_dir, "Spawning shell script (opt-in)");

    #[cfg(windows)]
    let mut cmd = {
        use std::os::windows::process::CommandExt;
        let mut c = Command::new("cmd.exe");
        c.raw_arg(format!("/C {shell_script}"));
        c
    };

    #[cfg(not(windows))]
    let mut cmd = {
        let mut c = Command::new("sh");
        c.args(["-c", shell_script]);
        c
    };
    configure_process_group(&mut cmd);

    #[cfg(windows)]
    {
        if let Some(fresh_path) = refresh_system_path() {
            cmd.env("PATH", fresh_path);
        }
        cmd.env("PYTHONUTF8", "1");
        cmd.env("PYTHONIOENCODING", "utf-8");
    }

    apply_plain_output_env(&mut cmd);
    cmd.current_dir(&working_dir)
        .stdin(std::process::Stdio::null());
    Ok((cmd, working_dir))
}
