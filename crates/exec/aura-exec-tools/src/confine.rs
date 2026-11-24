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
//!
//! A command with a [`RunScratch`] writes its temp files, caches and
//! builds beneath the scratch folder instead of the shared ones, so
//! runs meant to be independent can't pass files to each other.

use crate::error::ToolError;
use aura_exec_traits::RunScratch;
use std::path::{Path, PathBuf};

/// Extra folders confined commands may write to, separated like `PATH`.
pub const WRITABLE_PATHS_VAR: &str = "AURA_YOLO_WRITABLE_PATHS";

/// Device files every command may write: output sinks and terminals.
/// None of them keeps what one command writes for another to read.
const WRITABLE_DEVICES: &[&str] = &[
    "/dev/null",
    "/dev/zero",
    "/dev/full",
    "/dev/tty",
    "/dev/ptmx",
    "/dev/pts",
];

/// POSIX shared memory, which keeps files like a temp folder does. It
/// stays writable even for a run with a [`RunScratch`]: `shm_open` (and
/// with it Python's `multiprocessing`, Chromium and anything else using
/// shared memory or POSIX semaphores) always works in this folder, and no
/// variable moves it; only a private mount namespace could. Runs can
/// still see each other's segments here, but these live only as long as
/// the processes that made them, so they don't carry builds or caches
/// from one run to the next.
const SHARED_MEMORY: &str = "/dev/shm";

/// What beneath the Cargo home a command with a [`RunScratch`] may still
/// write. The registry and git checkouts are download caches keyed by
/// crate version and commit, which builds only read once fetched, so
/// sharing them passes nothing between runs. The lock files are how Cargo
/// guards those caches; without them it refuses to resolve at all.
/// `.global-cache` is Cargo's last-use log for cache cleaning, which it
/// only warns about when it can't update.
const CARGO_HOME_SHARED: &[&str] = &[
    "registry",
    "git",
    ".package-cache",
    ".package-cache-mutate",
    ".global-cache",
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
///
/// With a `scratch`, the shared temp folders, tool caches and Cargo
/// target folders are replaced by the scratch folder, which the
/// command's variables point those tools at (see [`RunScratch::vars`]),
/// so what one run writes there no other run can read back.
#[must_use]
pub fn writable_roots(workspace: &Path, scratch: Option<&RunScratch>) -> Vec<PathBuf> {
    let mut roots = vec![workspace.to_path_buf()];
    match scratch {
        Some(scratch) => roots.extend(scratch_roots(scratch)),
        None => roots.extend(shared_roots(workspace)),
    }
    roots.extend(WRITABLE_DEVICES.iter().map(PathBuf::from));
    // Folders the operator named for confined commands on purpose.
    if let Some(extra) = std::env::var_os(WRITABLE_PATHS_VAR) {
        roots.extend(std::env::split_paths(&extra).filter(|p| !p.as_os_str().is_empty()));
    }
    roots
}

/// The temp folders, shared memory, home caches and build folders every
/// confined command without a scratch folder may write.
fn shared_roots(workspace: &Path) -> Vec<PathBuf> {
    let mut roots = vec![
        std::env::temp_dir(),
        PathBuf::from("/tmp"),
        PathBuf::from("/var/tmp"),
        PathBuf::from(SHARED_MEMORY),
    ];
    if let Some(home) = home_dir() {
        roots.extend(HOME_CACHES.iter().map(|dir| home.join(dir)));
    }
    roots.extend(
        CACHE_VARS
            .iter()
            .filter_map(std::env::var_os)
            .map(PathBuf::from),
    );
    roots.extend(cargo_target_dirs(workspace));
    roots
}

/// The scratch folder, plus the shared folders a run with one still
/// writes, which are kept shared on purpose:
///
/// - Cargo's download caches (see [`CARGO_HOME_SHARED`]);
/// - the rustup home, where toolchains install by version, so a run that
///   installs the toolchain its copy pins leaves nothing another run
///   could tell apart from its own install;
/// - shared memory, which can't be redirected (see [`SHARED_MEMORY`]).
fn scratch_roots(scratch: &RunScratch) -> Vec<PathBuf> {
    let mut roots = vec![scratch.dir().to_path_buf(), PathBuf::from(SHARED_MEMORY)];
    if let Some(cargo_home) = tool_home("CARGO_HOME", ".cargo") {
        roots.extend(CARGO_HOME_SHARED.iter().map(|entry| cargo_home.join(entry)));
    }
    roots.extend(tool_home("RUSTUP_HOME", ".rustup"));
    roots
}

fn home_dir() -> Option<PathBuf> {
    std::env::var_os("HOME").map(PathBuf::from)
}

/// The folder `var` names, else `default` beneath the home folder.
fn tool_home(var: &str, default: &str) -> Option<PathBuf> {
    std::env::var_os(var)
        .map(PathBuf::from)
        .or_else(|| home_dir().map(|home| home.join(default)))
}

/// Build folders named by `[build] target-dir` in the Cargo configs that
/// apply to `workspace`. Without them a confined `cargo` can't use a
/// shared target folder outside the workspace, and agents fall back to
/// private folders that each rebuild every dependency from scratch.
fn cargo_target_dirs(workspace: &Path) -> Vec<PathBuf> {
    workspace
        .ancestors()
        .filter_map(|dir| {
            let text = ["config.toml", "config"]
                .iter()
                .find_map(|name| std::fs::read_to_string(dir.join(".cargo").join(name)).ok())?;
            let config = toml::from_str::<toml::Table>(&text).ok()?;
            let target = config.get("build")?.get("target-dir")?.as_str()?;
            Some(dir.join(target))
        })
        .collect()
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
/// [`writable_roots`]`(workspace, scratch)`; every process `f` starts
/// inherits the confinement.
///
/// # Errors
///
/// [`ToolError::Forbidden`] when the system cannot confine commands, and
/// [`ToolError::CommandFailed`] when the thread panics.
pub fn run_confined<T: Send>(
    workspace: &Path,
    scratch: Option<&RunScratch>,
    f: impl FnOnce() -> Result<T, ToolError> + Send,
) -> Result<T, ToolError> {
    let roots = writable_roots(workspace, scratch);
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
        run_confined(&workspace.clone(), None, move || {
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
        let roots = writable_roots(Path::new("/w"), None);
        assert_eq!(roots[0], PathBuf::from("/w"));
        assert!(roots.contains(&PathBuf::from("/dev/null")));
    }

    #[test]
    fn writable_roots_include_the_configured_cargo_target_dir() {
        let repo = tempfile::tempdir().unwrap();
        std::fs::create_dir(repo.path().join(".cargo")).unwrap();
        std::fs::write(
            repo.path().join(".cargo/config.toml"),
            "[build]\ntarget-dir = \"../shared-target\"\n",
        )
        .unwrap();
        let workspace = repo.path().join("crates/app");
        std::fs::create_dir_all(&workspace).unwrap();

        let roots = writable_roots(&workspace, None);
        assert!(roots.contains(&repo.path().join("../shared-target")));
    }

    /// A workspace whose Cargo config names a target folder outside it,
    /// with the folder that target resolves to.
    fn workspace_with_target_dir() -> (tempfile::TempDir, PathBuf, PathBuf) {
        let repo = tempfile::tempdir().unwrap();
        std::fs::create_dir(repo.path().join(".cargo")).unwrap();
        std::fs::write(
            repo.path().join(".cargo/config.toml"),
            "[build]\ntarget-dir = \"../shared-target\"\n",
        )
        .unwrap();
        let target = repo.path().join("../shared-target");
        let workspace = repo.path().to_path_buf();
        (repo, workspace, target)
    }

    /// The roots as computed before scratch folders existed.
    fn roots_without_scratch_support(workspace: &Path) -> Vec<PathBuf> {
        let mut roots = vec![
            workspace.to_path_buf(),
            std::env::temp_dir(),
            PathBuf::from("/tmp"),
            PathBuf::from("/var/tmp"),
        ];
        roots.extend(WRITABLE_DEVICES.iter().map(PathBuf::from));
        roots.push(PathBuf::from("/dev/shm"));
        if let Some(home) = std::env::var_os("HOME").map(PathBuf::from) {
            roots.extend(HOME_CACHES.iter().map(|dir| home.join(dir)));
        }
        roots.extend(
            CACHE_VARS
                .iter()
                .filter_map(std::env::var_os)
                .map(PathBuf::from),
        );
        roots.extend(cargo_target_dirs(workspace));
        if let Some(extra) = std::env::var_os(WRITABLE_PATHS_VAR) {
            roots.extend(std::env::split_paths(&extra).filter(|p| !p.as_os_str().is_empty()));
        }
        roots
    }

    #[test]
    fn confine_roots_with_a_scratch_replace_the_shared_temp_and_build_folders() {
        let (_repo, workspace, target) = workspace_with_target_dir();
        let scratch_parent = tempfile::tempdir().unwrap();
        let scratch = RunScratch::new(scratch_parent.path().join("a-1.tmp"));

        let roots = writable_roots(&workspace, Some(&scratch));

        assert_eq!(roots[0], workspace);
        assert!(roots.contains(&scratch.dir().to_path_buf()));
        for shared in ["/tmp", "/var/tmp"] {
            assert!(!roots.contains(&PathBuf::from(shared)), "{shared}");
        }
        assert!(
            roots.contains(&PathBuf::from("/dev/shm")),
            "shared memory can't be redirected, so it stays writable"
        );
        assert!(!roots.contains(&std::env::temp_dir()));
        assert!(!roots.contains(&target), "{target:?}");
        if let Some(home) = home_dir() {
            assert!(!roots.contains(&home.join(".cache")));
            assert!(!roots.contains(&home.join(".cargo")));
        }
        assert!(roots.contains(&PathBuf::from("/dev/null")));
        assert!(roots.contains(&PathBuf::from("/dev/pts")));
    }

    #[test]
    fn confine_roots_with_a_scratch_keep_the_cargo_download_caches() {
        let (_repo, workspace, _target) = workspace_with_target_dir();
        let scratch = RunScratch::new(workspace.join("scratch"));
        let roots = writable_roots(&workspace, Some(&scratch));
        if let Some(cargo_home) = tool_home("CARGO_HOME", ".cargo") {
            for entry in CARGO_HOME_SHARED {
                assert!(roots.contains(&cargo_home.join(entry)), "{entry}");
            }
        }
        if let Some(rustup_home) = tool_home("RUSTUP_HOME", ".rustup") {
            assert!(roots.contains(&rustup_home));
        }
    }

    #[test]
    fn confine_roots_without_a_scratch_are_unchanged() {
        let (_repo, workspace, target) = workspace_with_target_dir();

        let mut roots = writable_roots(&workspace, None);
        let mut before = roots_without_scratch_support(&workspace);

        assert_eq!(roots[0], workspace);
        for shared in ["/tmp", "/var/tmp", "/dev/shm"] {
            assert!(roots.contains(&PathBuf::from(shared)), "{shared}");
        }
        assert!(roots.contains(&target));
        roots.sort();
        before.sort();
        assert_eq!(roots, before);
    }

    #[test]
    fn confined_commands_with_a_scratch_write_there_but_not_in_tmp() {
        if !supported() {
            return;
        }
        let parent = tempfile::tempdir_in(std::env::current_dir().unwrap()).unwrap();
        let workspace = parent.path().join("ws");
        std::fs::create_dir(&workspace).unwrap();
        let scratch = RunScratch::new(parent.path().join("ws.tmp"));
        scratch.create().unwrap();
        let in_tmp = PathBuf::from("/tmp").join(format!(
            "aura-confine-scratch-{}",
            parent.path().file_name().unwrap().to_string_lossy()
        ));
        let script = format!(
            "touch {}/tmp/made && ! touch {}",
            scratch.dir().display(),
            in_tmp.display()
        );

        let ws = workspace.clone();
        let ok = run_confined(&workspace, Some(&scratch), move || {
            Ok(Command::new("sh")
                .args(["-c", &script])
                .current_dir(&ws)
                .status()
                .map_err(|e| ToolError::CommandFailed(e.to_string()))?
                .success())
        })
        .unwrap();

        let leaked = in_tmp.exists();
        let _ = std::fs::remove_file(&in_tmp);
        assert!(ok);
        assert!(!leaked);
        assert!(scratch.dir().join("tmp/made").exists());
    }
}
