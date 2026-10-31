//! Run a command under a pseudo-terminal, so it lays out and colors its
//! output the way it would in an interactive terminal (`ls` columns,
//! `git` / `grep` colors).

use super::{truncate_output, STDOUT_TRUNCATE_LIMIT};
use crate::error::ToolError;
use crate::sandbox::Sandbox;
use aura_core_types::ToolResult;
use portable_pty::{native_pty_system, Child, CommandBuilder, PtySize};
use std::io::{Read, Write};
use std::sync::mpsc;
use std::time::{Duration, Instant};
use tracing::debug;

/// What to run inside the terminal.
pub(super) enum PtyCommand<'a> {
    /// A program spawned directly with `args`.
    Program {
        /// Executable name or path.
        program: &'a str,
        /// Arguments passed verbatim.
        args: &'a [String],
    },
    /// A script run by `sh -c` (`cmd.exe /C` on Windows).
    ShellScript(&'a str),
}

/// Terminal height reported to the program.
const ROWS: u16 = 50;
/// How long to keep reading after the program exits, for output still
/// buffered in the terminal.
const DRAIN_GRACE: Duration = Duration::from_millis(200);
/// Bytes kept from a runaway program before the rest is dropped.
const MAX_CAPTURE_BYTES: usize = 1 << 20;
/// Programs that only color their output when asked, as distro shell
/// aliases do (`alias ls='ls --color=auto'`).
#[cfg(target_os = "linux")]
const COLOR_AUTO_PROGRAMS: &[&str] = &["ls", "dir", "vdir", "grep", "egrep", "fgrep", "diff"];

/// Run `command` in a `cols`-wide pseudo-terminal.
///
/// Returns `None` when no pseudo-terminal can be opened, so the caller
/// can fall back to running over pipes. Output is the merged stdout and
/// stderr with ANSI styling kept and carriage returns resolved.
pub(super) fn cmd_run_in_pty(
    sandbox: &Sandbox,
    command: &PtyCommand<'_>,
    cwd: Option<&str>,
    cols: u16,
    timeout_ms: u64,
) -> Option<Result<ToolResult, ToolError>> {
    let working_dir = match cwd {
        Some(dir) => match sandbox.resolve_existing(dir) {
            Ok(dir) => dir,
            Err(e) => return Some(Err(e)),
        },
        None => sandbox.root().to_path_buf(),
    };

    let pair = match native_pty_system().openpty(PtySize {
        rows: ROWS,
        cols: cols.max(1),
        pixel_width: 0,
        pixel_height: 0,
    }) {
        Ok(pair) => pair,
        Err(e) => {
            debug!(error = %e, "No pseudo-terminal; running over pipes");
            return None;
        }
    };

    let mut builder = command_builder(command);
    builder.cwd(&working_dir);
    apply_terminal_env(&mut builder, cols);

    let child = pair.slave.spawn_command(builder);
    drop(pair.slave);
    let mut child = match child {
        Ok(child) => child,
        Err(e) => {
            return Some(Err(ToolError::CommandFailed(format!(
                "Failed to spawn command '{}': {e}",
                command_label(command)
            ))))
        }
    };

    // End-of-file on stdin, so programs that read it (`cat`, `wc`) finish
    // instead of waiting for input that never comes. The writer stays
    // open until output is read: dropping it types a newline, which the
    // terminal would echo.
    let writer = pair.master.take_writer().ok().map(|mut writer| {
        let _ = writer.write_all(b"\x04");
        writer
    });

    let reader = match pair.master.try_clone_reader() {
        Ok(reader) => reader,
        Err(e) => {
            kill_tree(child.as_mut());
            return Some(Err(ToolError::CommandFailed(format!(
                "Failed to read terminal output: {e}"
            ))));
        }
    };
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || read_until_closed(reader, &tx));

    let mut captured = Vec::new();
    let status = match wait_capturing(child.as_mut(), &rx, &mut captured, timeout_ms) {
        Ok(status) => status,
        Err(e) => return Some(Err(e)),
    };
    let deadline = Instant::now() + DRAIN_GRACE;
    while let Ok(chunk) = rx.recv_timeout(deadline.saturating_duration_since(Instant::now())) {
        capture(&mut captured, &chunk);
    }
    drop(writer);
    drop(pair.master);

    let output = normalize_terminal_output(&String::from_utf8_lossy(&captured));
    let output = truncate_output(&output, STDOUT_TRUNCATE_LIMIT);
    let exit_code = i32::try_from(status.exit_code()).unwrap_or(-1);
    let result = if status.success() {
        ToolResult::success("run_command", output)
    } else {
        let mut result = ToolResult::failure("run_command", String::new());
        result.stdout = output.into();
        result.exit_code = Some(exit_code);
        result
    };
    Some(Ok(result.with_metadata("exit_code", exit_code.to_string())))
}

fn command_builder(command: &PtyCommand<'_>) -> CommandBuilder {
    match command {
        PtyCommand::Program { program, args } => {
            let mut builder = CommandBuilder::new(program);
            if wants_color_flag(program, args) {
                builder.arg("--color=auto");
            }
            builder.args(args.iter());
            builder
        }
        PtyCommand::ShellScript(script) => {
            #[cfg(windows)]
            let mut builder = CommandBuilder::new("cmd.exe");
            #[cfg(windows)]
            builder.args(["/C", script]);
            #[cfg(not(windows))]
            let mut builder = CommandBuilder::new("sh");
            #[cfg(not(windows))]
            builder.args(["-c", script]);
            builder
        }
    }
}

fn command_label<'a>(command: &PtyCommand<'a>) -> &'a str {
    match command {
        PtyCommand::Program { program, .. } => program,
        PtyCommand::ShellScript(script) => script,
    }
}

#[cfg(target_os = "linux")]
fn wants_color_flag(program: &str, args: &[String]) -> bool {
    COLOR_AUTO_PROGRAMS.contains(&program) && !args.iter().any(|arg| arg.starts_with("--color"))
}

#[cfg(not(target_os = "linux"))]
fn wants_color_flag(_program: &str, _args: &[String]) -> bool {
    false
}

/// Look like an interactive color terminal, but never page or prompt:
/// nobody can type into this terminal.
fn apply_terminal_env(builder: &mut CommandBuilder, cols: u16) {
    builder.env("TERM", "xterm-256color");
    builder.env("COLUMNS", cols.to_string());
    builder.env("LINES", ROWS.to_string());
    builder.env("CLICOLOR", "1");
    builder.env_remove("NO_COLOR");
    builder.env("PAGER", "cat");
    builder.env("GIT_PAGER", "cat");
    builder.env("GIT_TERMINAL_PROMPT", "0");
}

fn read_until_closed(mut reader: Box<dyn Read + Send>, tx: &mpsc::Sender<Vec<u8>>) {
    let mut buf = [0u8; 8192];
    loop {
        // Linux reports EIO once the last program holding the terminal exits.
        match reader.read(&mut buf) {
            Ok(0) | Err(_) => return,
            Ok(n) => {
                if tx.send(buf[..n].to_vec()).is_err() {
                    return;
                }
            }
        }
    }
}

fn wait_capturing(
    child: &mut (dyn Child + Send + Sync),
    rx: &mpsc::Receiver<Vec<u8>>,
    captured: &mut Vec<u8>,
    timeout_ms: u64,
) -> Result<portable_pty::ExitStatus, ToolError> {
    let timeout = Duration::from_millis(timeout_ms);
    let start = Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(status)) => return Ok(status),
            Ok(None) => {}
            Err(e) => {
                return Err(ToolError::CommandFailed(format!(
                    "Failed to wait for command: {e}"
                )))
            }
        }
        if start.elapsed() > timeout {
            kill_tree(child);
            return Err(ToolError::CommandFailed(
                "Command timed out: Process timed out".into(),
            ));
        }
        match rx.recv_timeout(Duration::from_millis(10)) {
            Ok(chunk) => capture(captured, &chunk),
            Err(mpsc::RecvTimeoutError::Timeout) => {}
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                std::thread::sleep(Duration::from_millis(10));
            }
        }
    }
}

fn capture(captured: &mut Vec<u8>, chunk: &[u8]) {
    let room = MAX_CAPTURE_BYTES.saturating_sub(captured.len());
    captured.extend_from_slice(&chunk[..chunk.len().min(room)]);
}

/// The program leads its own session, so its pid is also the process
/// group of everything it started.
fn kill_tree(child: &mut (dyn Child + Send + Sync)) {
    #[cfg(unix)]
    if let Some(pid) = child.process_id() {
        let _ = std::process::Command::new("kill")
            .args(["-KILL", "--", &format!("-{pid}")])
            .status();
    }
    let _ = child.kill();
    let _ = child.wait();
}

/// Turn raw terminal output into plain lines: `\r\n` becomes `\n`, and a
/// line redrawn with `\r` (progress bars) keeps only its final text.
fn normalize_terminal_output(raw: &str) -> String {
    raw.split('\n')
        .map(|line| {
            let line = line.trim_end_matches('\r');
            line.rsplit('\r').next().unwrap_or(line)
        })
        .collect::<Vec<_>>()
        .join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalizes_line_endings_and_redraws() {
        assert_eq!(normalize_terminal_output("a\r\nb\r\n"), "a\nb\n");
        assert_eq!(
            normalize_terminal_output("10%\r50%\r100%\r\ndone"),
            "100%\ndone"
        );
        assert_eq!(normalize_terminal_output("x\r\r\n"), "x\n");
    }

    #[cfg(unix)]
    fn run(sandbox: &Sandbox, command: &PtyCommand<'_>, cols: u16) -> ToolResult {
        cmd_run_in_pty(sandbox, command, None, cols, 10_000)
            .expect("pseudo-terminal available")
            .expect("command ran")
    }

    #[cfg(unix)]
    #[test]
    fn output_goes_to_a_terminal() {
        let dir = tempfile::tempdir().unwrap();
        let sandbox = Sandbox::new(dir.path()).unwrap();
        let result = run(
            &sandbox,
            &PtyCommand::ShellScript("test -t 1 && echo tty; echo err >&2"),
            80,
        );
        assert!(result.ok);
        assert_eq!(String::from_utf8_lossy(&result.stdout), "tty\nerr\n");
    }

    #[cfg(unix)]
    #[test]
    fn ls_lays_out_columns_to_the_terminal_width() {
        let dir = tempfile::tempdir().unwrap();
        for name in ["alpha", "bravo", "charlie", "delta"] {
            std::fs::write(dir.path().join(name), "").unwrap();
        }
        let sandbox = Sandbox::new(dir.path()).unwrap();
        let args = Vec::new();
        let result = run(
            &sandbox,
            &PtyCommand::Program {
                program: "ls",
                args: &args,
            },
            80,
        );
        let stdout = String::from_utf8_lossy(&result.stdout);
        let first_line = stdout.lines().next().unwrap_or_default();
        assert!(
            first_line.contains("alpha") && first_line.contains("delta"),
            "{stdout:?}"
        );
    }

    #[cfg(unix)]
    #[test]
    fn stdin_is_closed() {
        let dir = tempfile::tempdir().unwrap();
        let sandbox = Sandbox::new(dir.path()).unwrap();
        let start = Instant::now();
        let result = run(&sandbox, &PtyCommand::ShellScript("cat; echo done"), 80);
        assert!(start.elapsed() < Duration::from_secs(5));
        assert_eq!(String::from_utf8_lossy(&result.stdout).trim(), "done");
    }

    #[cfg(unix)]
    #[test]
    fn failure_keeps_output_and_exit_code() {
        let dir = tempfile::tempdir().unwrap();
        let sandbox = Sandbox::new(dir.path()).unwrap();
        let result = run(&sandbox, &PtyCommand::ShellScript("echo nope; exit 3"), 80);
        assert!(!result.ok);
        assert_eq!(result.exit_code, Some(3));
        assert_eq!(String::from_utf8_lossy(&result.stdout), "nope\n");
    }

    #[cfg(unix)]
    #[test]
    fn timeout_kills_the_command() {
        let dir = tempfile::tempdir().unwrap();
        let sandbox = Sandbox::new(dir.path()).unwrap();
        let start = Instant::now();
        let result = cmd_run_in_pty(
            &sandbox,
            &PtyCommand::ShellScript("sleep 30"),
            None,
            80,
            200,
        )
        .expect("pseudo-terminal available");
        assert!(matches!(result, Err(ToolError::CommandFailed(_))));
        assert!(start.elapsed() < Duration::from_secs(5));
    }
}
