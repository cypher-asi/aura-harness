//! The only place Safe Workspace spawns Git processes (invariant §1 declared
//! exception). Every command goes through [`local_git_command`], which
//! enforces a closed local-only subcommand allowlist.

use super::{SafeWorkspaceError, WorkspaceMetadata, CHECKPOINT_INDEX_FILE, CHECKPOINT_STORE_DIR};
use std::io::Write;
use std::path::Path;
use std::process::{Command, Output, Stdio};

const LOCAL_GIT_SUBCOMMANDS: &[&str] = &[
    "add",
    "apply",
    "commit",
    "commit-tree",
    "diff",
    "init",
    "log",
    "ls-files",
    "merge-base",
    "read-tree",
    "rev-list",
    "rev-parse",
    "update-ref",
    "worktree",
    "write-tree",
];

fn command_error(output: &Output) -> String {
    let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
    if stderr.is_empty() {
        String::from_utf8_lossy(&output.stdout).trim().to_string()
    } else {
        stderr
    }
}

fn local_git_subcommand<'a>(args: &'a [&str]) -> Option<&'a str> {
    let mut index = 0;
    while let Some(argument) = args.get(index) {
        if *argument == "-c" {
            index += 2;
        } else if argument.starts_with('-') {
            index += 1;
        } else {
            return Some(argument);
        }
    }
    None
}

pub(super) fn local_git_command(cwd: &Path, args: &[&str]) -> Result<Command, SafeWorkspaceError> {
    let subcommand = local_git_subcommand(args).ok_or_else(|| {
        SafeWorkspaceError::Unsupported(
            "Safe Workspace Git command is missing a subcommand".to_string(),
        )
    })?;
    if !LOCAL_GIT_SUBCOMMANDS.contains(&subcommand) {
        return Err(SafeWorkspaceError::Unsupported(format!(
            "Git subcommand '{subcommand}' is not allowed for Safe Workspace lifecycle operations"
        )));
    }

    let mut command = Command::new("git");
    command
        .current_dir(cwd)
        .args(args)
        .env("GIT_TERMINAL_PROMPT", "0");
    Ok(command)
}

pub(super) fn run_git_allow_failure(
    cwd: &Path,
    args: &[&str],
) -> Result<Output, SafeWorkspaceError> {
    Ok(local_git_command(cwd, args)?.output()?)
}

pub(super) fn run_git(cwd: &Path, args: &[&str]) -> Result<Output, SafeWorkspaceError> {
    let output = run_git_allow_failure(cwd, args)?;
    if output.status.success() {
        Ok(output)
    } else {
        Err(SafeWorkspaceError::Git(command_error(&output)))
    }
}

pub(super) fn run_git_with_input(
    cwd: &Path,
    args: &[&str],
    input: &[u8],
) -> Result<Output, SafeWorkspaceError> {
    let mut command = local_git_command(cwd, args)?;
    let mut child = command
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;
    child
        .stdin
        .take()
        .ok_or_else(|| SafeWorkspaceError::Git("could not open git stdin".to_string()))?
        .write_all(input)?;
    let output = child.wait_with_output()?;
    if output.status.success() {
        Ok(output)
    } else {
        Err(SafeWorkspaceError::Git(command_error(&output)))
    }
}

fn shadow_command(
    metadata: &WorkspaceMetadata,
    args: &[&str],
) -> Result<Command, SafeWorkspaceError> {
    let root = metadata.session_dir()?;
    let mut command = local_git_command(&metadata.workspace_root, args)?;
    command
        .env("GIT_DIR", root.join(CHECKPOINT_STORE_DIR))
        .env("GIT_WORK_TREE", &metadata.workspace_root)
        .env("GIT_INDEX_FILE", root.join(CHECKPOINT_INDEX_FILE))
        .env("GIT_AUTHOR_NAME", "Aura Safe Workspace")
        .env("GIT_AUTHOR_EMAIL", "safe-workspace@aura.local")
        .env("GIT_COMMITTER_NAME", "Aura Safe Workspace")
        .env("GIT_COMMITTER_EMAIL", "safe-workspace@aura.local");
    Ok(command)
}

pub(super) fn run_shadow(
    metadata: &WorkspaceMetadata,
    args: &[&str],
    allowed_failure: bool,
) -> Result<Output, SafeWorkspaceError> {
    let output = shadow_command(metadata, args)?.output()?;
    if output.status.success() || allowed_failure {
        Ok(output)
    } else {
        Err(SafeWorkspaceError::Git(command_error(&output)))
    }
}

pub(super) fn stdout_trimmed(output: &Output) -> String {
    String::from_utf8_lossy(&output.stdout).trim().to_string()
}
