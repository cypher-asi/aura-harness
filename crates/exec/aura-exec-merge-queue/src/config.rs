//! What a repo's queue merges into and how it checks an entry first.
//!
//! The defaults hold unless the repo has a `.aura/merge-queue.toml`:
//!
//! ```toml
//! target = "main"                    # branch merges land on
//! build = "cargo build --workspace"  # "" turns the build check off
//! test = "cargo test --workspace"    # "" turns the test check off
//! check_timeout_secs = 3600          # per check
//! ```
//!
//! Without a file, the checks are inferred from the repo's files
//! ([`infer_checks`]).

use crate::error::QueueError;
use serde::Deserialize;
use std::path::Path;
use std::time::Duration;

/// Where a repo's queue config lives, relative to the repo's top.
pub const CONFIG_PATH: &str = ".aura/merge-queue.toml";
/// Branch merges land on by default.
pub const DEFAULT_TARGET: &str = "main";
/// How long one check may run by default.
pub const DEFAULT_CHECK_TIMEOUT: Duration = Duration::from_secs(60 * 60);
/// How long one git command may run.
pub const GIT_TIMEOUT: Duration = Duration::from_secs(5 * 60);

/// How a repo's queue merges.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QueueConfig {
    /// Branch merges land on
    pub target: String,
    /// Command that must pass before a merge lands, if any
    pub build: Option<String>,
    /// Second command that must pass, if any
    pub test: Option<String>,
    /// How long each check may run
    pub check_timeout: Duration,
    /// How long each git command may run
    pub git_timeout: Duration,
}

impl QueueConfig {
    /// A config merging into `target` with no checks.
    #[must_use]
    pub fn unchecked(target: &str) -> Self {
        Self {
            target: target.to_string(),
            build: None,
            test: None,
            check_timeout: DEFAULT_CHECK_TIMEOUT,
            git_timeout: GIT_TIMEOUT,
        }
    }

    /// The config of the repo whose top is `top`: its config file over
    /// the defaults and the inferred checks.
    ///
    /// # Errors
    /// The config file exists but can't be read or parsed.
    pub fn load(top: &Path) -> Result<Self, QueueError> {
        let path = top.join(CONFIG_PATH);
        let file = match std::fs::read_to_string(&path) {
            Ok(text) => toml::from_str::<ConfigFile>(&text).map_err(|e| QueueError::Config {
                path: path.display().to_string(),
                reason: e.to_string(),
            })?,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => ConfigFile::default(),
            Err(e) => return Err(QueueError::io(&path, e)),
        };
        Ok(file.over(top))
    }
}

/// The optional fields of `.aura/merge-queue.toml`.
#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct ConfigFile {
    target: Option<String>,
    build: Option<String>,
    test: Option<String>,
    check_timeout_secs: Option<u64>,
}

impl ConfigFile {
    fn over(self, top: &Path) -> QueueConfig {
        let (build, test) = infer_checks(top);
        let chosen = |set: Option<String>, inferred: Option<String>| match set {
            Some(command) if command.trim().is_empty() => None,
            Some(command) => Some(command),
            None => inferred,
        };
        QueueConfig {
            target: self
                .target
                .filter(|target| !target.trim().is_empty())
                .unwrap_or_else(|| DEFAULT_TARGET.to_string()),
            build: chosen(self.build, build),
            test: chosen(self.test, test),
            check_timeout: self
                .check_timeout_secs
                .map_or(DEFAULT_CHECK_TIMEOUT, Duration::from_secs),
            git_timeout: GIT_TIMEOUT,
        }
    }
}

/// The build and test commands a repo's files suggest: Cargo for a
/// `Cargo.toml`, npm for a `package.json`, Go for a `go.mod`; none
/// otherwise.
#[must_use]
pub fn infer_checks(top: &Path) -> (Option<String>, Option<String>) {
    let has = |name: &str| top.join(name).is_file();
    let pair = |build: &str, test: &str| (Some(build.to_string()), Some(test.to_string()));
    if has("Cargo.toml") {
        pair(
            "cargo build --workspace --all-targets",
            "cargo test --workspace",
        )
    } else if has("package.json") {
        pair("npm run build --if-present", "npm test --if-present")
    } else if has("go.mod") {
        pair("go build ./...", "go test ./...")
    } else {
        (None, None)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_rust_repo_without_a_config_builds_and_tests_with_cargo() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("Cargo.toml"), "").unwrap();
        let config = QueueConfig::load(dir.path()).unwrap();
        assert_eq!(config.target, "main");
        assert_eq!(
            config.build.as_deref(),
            Some("cargo build --workspace --all-targets")
        );
        assert_eq!(config.test.as_deref(), Some("cargo test --workspace"));
    }

    #[test]
    fn a_repo_with_nothing_to_build_has_no_checks() {
        let dir = tempfile::tempdir().unwrap();
        let config = QueueConfig::load(dir.path()).unwrap();
        assert_eq!((config.build, config.test), (None, None));
    }

    #[test]
    fn the_config_file_sets_target_checks_and_timeout() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("Cargo.toml"), "").unwrap();
        std::fs::create_dir_all(dir.path().join(".aura")).unwrap();
        std::fs::write(
            dir.path().join(CONFIG_PATH),
            "target = \"trunk\"\nbuild = \"\"\ntest = \"make check\"\ncheck_timeout_secs = 5\n",
        )
        .unwrap();
        let config = QueueConfig::load(dir.path()).unwrap();
        assert_eq!(config.target, "trunk");
        assert_eq!(config.build, None, "an empty command turns a check off");
        assert_eq!(config.test.as_deref(), Some("make check"));
        assert_eq!(config.check_timeout, Duration::from_secs(5));
    }

    #[test]
    fn a_bad_config_file_is_an_error_not_a_silent_default() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join(".aura")).unwrap();
        std::fs::write(dir.path().join(CONFIG_PATH), "targte = \"x\"").unwrap();
        assert!(matches!(
            QueueConfig::load(dir.path()),
            Err(QueueError::Config { .. })
        ));
    }
}
