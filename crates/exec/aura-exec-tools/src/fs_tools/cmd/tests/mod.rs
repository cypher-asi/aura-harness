use super::args::without_stray_zero_width;

use super::output::{
    classify_cargo_invocation, extract_cargo_errors, truncate_output, STDOUT_TRUNCATE_LIMIT,
};

use super::policy::{
    check_binary_allowlist, check_command_allowlist, is_shell_program, script_program,
    strip_windows_executable_suffix, validate_program_name,
};

use super::wait::{cmd_run, cmd_run_shell_script, read_capped, PIPE_KEEP_BYTES};

use super::*;

use crate::sandbox::Sandbox;

use std::path::Path;

use tempfile::TempDir;

fn create_test_sandbox() -> (Sandbox, TempDir) {
    let dir = TempDir::new().unwrap();
    let sandbox = Sandbox::new(dir.path()).unwrap();
    (sandbox, dir)
}

#[cfg(unix)]
fn command_fragment_running(fragment: &str) -> bool {
    let output = std::process::Command::new("ps")
        .args(["-eo", "command"])
        .output()
        .expect("ps must run on Unix test hosts");
    let stdout = String::from_utf8_lossy(&output.stdout);
    stdout.lines().any(|line| line.contains(fragment))
}

// ========================================================================
// CmdRunTool::execute gating — Phase 2 hardening
// ========================================================================

/// Build a `ToolContext` pointing at a fresh tempdir.
fn tool_ctx(config: crate::ToolConfig) -> (ToolContext, TempDir) {
    let dir = TempDir::new().unwrap();
    let sandbox = Sandbox::new(dir.path()).unwrap();
    (ToolContext::new(sandbox, config), dir)
}

fn command_config(command: crate::CommandPolicy) -> crate::ToolConfig {
    crate::ToolConfig {
        command,
        ..crate::ToolConfig::default()
    }
}

/// Background hook that keeps what it is handed.
struct KeepProcesses {
    dir: std::path::PathBuf,
    adopted: std::sync::Mutex<Vec<crate::BackgroundProcess>>,
}

impl crate::BackgroundProcessHook for KeepProcesses {
    fn log_dir(&self) -> std::path::PathBuf {
        self.dir.clone()
    }

    fn adopt(
        &self,
        _agent_id: aura_core_types::AgentId,
        process: crate::BackgroundProcess,
    ) -> Result<(), String> {
        self.adopted.lock().unwrap().push(process);
        Ok(())
    }
}

/// Output hook that keeps each chunk with when it arrived.
#[derive(Default)]
struct RecordOutput(std::sync::Mutex<Vec<(std::time::Instant, String, Vec<u8>)>>);

impl crate::CommandOutputHook for RecordOutput {
    fn output(&self, tool_use_id: &str, chunk: &[u8]) {
        self.0.lock().unwrap().push((
            std::time::Instant::now(),
            tool_use_id.to_string(),
            chunk.to_vec(),
        ));
    }
}

mod allowlists;
mod hooks;
mod pipes;
mod platform;
mod pushes;
mod results;
mod running;
mod shell_scripts;
mod tool;
mod validation;
