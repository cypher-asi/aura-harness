//! Write confinement for commands that run without asking (the `yolo`
//! approval level).
//!
//! On Linux a confined command runs under a Landlock ruleset that lets it
//! read and execute anything but write, create, rename or delete only
//! beneath [`writable_roots`]: the workspace, temp folders, the terminal
//! devices and the usual tool caches. Landlock restricts the calling
//! thread and every process it starts, so [`run_confined`] does the
//! spawning on a short-lived thread restricted first, leaving the rest of
//! the harness untouched. Elsewhere confinement is unavailable and
//! [`run_confined`] refuses to run anything.

use crate::error::ToolError;
use std::path::{Path, PathBuf};

/// Extra folders confined commands may write to, separated like `PATH`.
pub const WRITABLE_PATHS_VAR: &str = "AURA_YOLO_WRITABLE_PATHS";

/// Device files every command may write: output sinks and terminals.
const WRITABLE_DEVICES: &[&str] = &[
    "/dev/null",
    "/dev/zero",
    "/dev/full",
    "/dev/tty",
    "/dev/ptmx",
    "/dev/pts",
    "/dev/shm",
];

/// Folders under the home directory that build tools write their caches
/// and downloads to.
const HOME_CACHES: &[&str] = &[
    ".cache",
    ".cargo",
    ".rustup",
    ".npm",
    ".bun",
    ".yarn",
    ".pnpm-store",
    ".local/share/pnpm",
    ".gradle",
    ".m2",
    "go/pkg",
];

/// Variables naming cache or output folders outside the home defaults.
const CACHE_VARS: &[&str] = &[
    "CARGO_HOME",
    "RUSTUP_HOME",
    "CARGO_TARGET_DIR",
    "npm_config_cache",
    "GOPATH",
    "GOCACHE",
    "XDG_CACHE_HOME",
];

/// Where a confined command working in `workspace` may write. Paths that
/// don't exist are dropped when the ruleset is built.
#[must_use]
pub fn writable_roots(workspace: &Path) -> Vec<PathBuf> {
    let mut roots = vec![
        workspace.to_path_buf(),
        std::env::temp_dir(),
        PathBuf::from("/tmp"),
        PathBuf::from("/var/tmp"),
    ];
    roots.extend(WRITABLE_DEVICES.iter().map(PathBuf::from));
    if let Some(home) = std::env::var_os("HOME").map(PathBuf::from) {
        roots.extend(HOME_CACHES.iter().map(|dir| home.join(dir)));
    }
    roots.extend(
        CACHE_VARS
            .iter()
            .filter_map(std::env::var_os)
            .map(PathBuf::from),
    );
    if let Some(extra) = std::env::var_os(WRITABLE_PATHS_VAR) {
        roots.extend(std::env::split_paths(&extra).filter(|p| !p.as_os_str().is_empty()));
    }
    roots
}

/// Whether this system can confine commands. Checked once, by confining
/// a throwaway thread.
#[must_use]
pub fn supported() -> bool {
    static SUPPORTED: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *SUPPORTED.get_or_init(|| {
        std::thread::spawn(|| imp::restrict(&[]).is_ok())
            .join()
            .unwrap_or(false)
    })
}

/// Run `f` on a thread confined to writing beneath
/// [`writable_roots`]`(workspace)`; every process `f` starts inherits the
/// confinement.
///
/// # Errors
///
/// [`ToolError::Forbidden`] when the system cannot confine commands, and
/// [`ToolError::CommandFailed`] when the thread panics.
pub fn run_confined<T: Send>(
    workspace: &Path,
    f: impl FnOnce() -> Result<T, ToolError> + Send,
) -> Result<T, ToolError> {
    let roots = writable_roots(workspace);
    std::thread::scope(|scope| {
        std::thread::Builder::new()
            .name("aura-confined".to_string())
            .spawn_scoped(scope, || {
                imp::restrict(&roots)?;
                f()
            })
            .map_err(|e| ToolError::CommandFailed(format!("starting confined thread: {e}")))?
            .join()
            .map_err(|_| ToolError::CommandFailed("confined command thread panicked".into()))?
    })
}

#[cfg(target_os = "linux")]
mod imp {
    use crate::error::ToolError;
    use landlock::{
        path_beneath_rules, AccessFs, Ruleset, RulesetAttr, RulesetCreatedAttr, RulesetStatus, ABI,
    };
    use std::path::PathBuf;

    /// Newest Landlock ABI whose write rights the ruleset handles; older
    /// kernels get the rights they know (best effort).
    const ABI_VERSION: ABI = ABI::V5;

    /// Confine the calling thread to writing beneath `roots`.
    pub(super) fn restrict(roots: &[PathBuf]) -> Result<(), ToolError> {
        let write = AccessFs::from_write(ABI_VERSION);
        let status = Ruleset::default()
            .handle_access(write)
            .and_then(|ruleset| ruleset.create())
            .and_then(|ruleset| ruleset.add_rules(path_beneath_rules(roots, write)))
            .and_then(|ruleset| ruleset.restrict_self())
            .map_err(|e| unavailable(&e.to_string()))?;
        match status.ruleset {
            RulesetStatus::FullyEnforced | RulesetStatus::PartiallyEnforced => Ok(()),
            RulesetStatus::NotEnforced => Err(unavailable("this kernel does not enforce Landlock")),
        }
    }

    fn unavailable(why: &str) -> ToolError {
        ToolError::Forbidden(format!(
            "cannot confine the command to writing in the workspace ({why}); \
             switch the approval level to ask to run it after approving"
        ))
    }
}

#[cfg(not(target_os = "linux"))]
mod imp {
    use crate::error::ToolError;
    use std::path::PathBuf;

    pub(super) fn restrict(_roots: &[PathBuf]) -> Result<(), ToolError> {
        Err(ToolError::Forbidden(
            "commands can only be confined to the workspace on Linux; \
             switch the approval level to ask to run it after approving"
                .into(),
        ))
    }
}

#[cfg(all(test, target_os = "linux"))]
mod tests {
    use super::*;
    use std::process::Command;

    fn sh(workspace: &Path, script: &str) -> bool {
        let workspace = workspace.to_path_buf();
        let script = script.to_string();
        run_confined(&workspace.clone(), move || {
            Ok(Command::new("sh")
                .args(["-c", &script])
                .current_dir(&workspace)
                .status()
                .map_err(|e| ToolError::CommandFailed(e.to_string()))?
                .success())
        })
        .unwrap()
    }

    #[test]
    fn confined_commands_write_only_inside_the_workspace() {
        if !supported() {
            eprintln!("Landlock unsupported here; skipping");
            return;
        }
        let parent = tempfile::tempdir_in(std::env::current_dir().unwrap()).unwrap();
        let workspace = parent.path().join("ws");
        std::fs::create_dir(&workspace).unwrap();

        assert!(sh(
            &workspace,
            "echo hi > inside.txt && mkdir sub && rm inside.txt"
        ));
        assert!(!sh(&workspace, "echo hi > ../outside.txt"));
        assert!(!parent.path().join("outside.txt").exists());
        assert!(!sh(&workspace, "mkdir ../escaped"));
        assert!(sh(
            &workspace,
            "cat /etc/hostname >/dev/null || cat /etc/os-release >/dev/null"
        ));
        assert!(sh(&workspace, "echo ok > /dev/null"));
    }

    #[test]
    fn the_harness_thread_stays_unconfined() {
        if !supported() {
            return;
        }
        let parent = tempfile::tempdir_in(std::env::current_dir().unwrap()).unwrap();
        let workspace = parent.path().join("ws");
        std::fs::create_dir(&workspace).unwrap();
        assert!(!sh(&workspace, "touch ../outside"));
        std::fs::write(parent.path().join("outside"), "harness").unwrap();
    }

    #[test]
    fn writable_roots_start_with_the_workspace() {
        let roots = writable_roots(Path::new("/w"));
        assert_eq!(roots[0], PathBuf::from("/w"));
        assert!(roots.contains(&PathBuf::from("/dev/null")));
    }
}
