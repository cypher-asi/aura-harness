//! `AURA_HOME` resolution (Phase 4a).
//!
//! ## Resolution order
//!
//! 1. `AURA_HOME` env var (highest priority).
//! 2. `CODEX_HOME` env var (read-only compat alias used for V1
//!    transition from Codex; see invariants below).
//! 3. Platform default: `{home_dir}/.aura`.
//!
//! ## Invariants ([rules.md §13])
//!
//! - Resolution is **pure** — it depends only on env vars and the
//!   platform home directory; there is no filesystem I/O at
//!   construct time. Consumers that need to materialise the
//!   directory must do so explicitly via `tokio::fs::create_dir_all`
//!   or similar.
//! - The `CODEX_HOME` alias is **READ-ONLY**. Future write paths in
//!   `aura migrate` (Phase 4a stub) and `aura-store-db` always write
//!   to the resolved `AURA_HOME` path; this knob exists only to
//!   ease V1 transition for users who previously ran Codex.
//! - Relative paths are rejected with
//!   [`AuraHomeError::NotAbsolute`]. The invariant prevents
//!   subcommands from accidentally creating an `aura_data/` directory
//!   underneath whichever cwd they happen to be invoked from.
//! - When no path can be resolved (env vars unset AND no platform
//!   home dir), [`AuraHomeError::NoHome`] is returned. The caller
//!   is expected to surface this through the CLI error chain rather
//!   than substitute a relative fallback.

use std::path::PathBuf;

use thiserror::Error;

/// Reasons [`AuraHome::resolve`] (or its testable inner
/// [`AuraHome::resolve_with`]) can fail.
#[derive(Debug, Error)]
pub enum AuraHomeError {
    /// Neither `AURA_HOME` nor `CODEX_HOME` is set, and the platform
    /// home directory could not be determined.
    #[error("AURA_HOME / CODEX_HOME unset and platform HOME directory unavailable")]
    NoHome,
    /// An env override was set, but the path is not absolute.
    #[error("AURA_HOME path is not absolute: {0}")]
    NotAbsolute(PathBuf),
}

/// Which input wound up satisfying the resolution.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AuraHomeSource {
    /// `AURA_HOME` env var.
    AuraHomeEnv,
    /// `CODEX_HOME` env var (read-only compat alias).
    CodexHomeEnv,
    /// Platform default: `{home_dir}/.aura`.
    DefaultUnderHome,
}

/// Resolved aura-home path + the source it was derived from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuraHome {
    /// Absolute path to the resolved aura-home directory. No I/O has
    /// been performed; the directory may not yet exist.
    pub path: PathBuf,
    /// Which input fed the resolution.
    pub source: AuraHomeSource,
}

impl AuraHome {
    /// Resolve from process env vars + the platform home directory.
    ///
    /// Pure with respect to filesystem state (no I/O at construct
    /// time) but observes process env state, so concurrent tests
    /// that mutate `AURA_HOME` / `CODEX_HOME` must serialize through
    /// a shared lock or use [`AuraHome::resolve_with`] directly.
    ///
    /// # Errors
    ///
    /// See [`AuraHomeError`].
    pub fn resolve() -> Result<Self, AuraHomeError> {
        Self::resolve_with(
            std::env::var_os(crate::env::AURA_HOME),
            std::env::var_os(crate::env::CODEX_HOME),
            dirs::home_dir(),
        )
    }

    /// Pure inner resolver — testable without env mutation.
    ///
    /// # Errors
    ///
    /// See [`AuraHomeError`].
    pub fn resolve_with(
        aura_home_env: Option<std::ffi::OsString>,
        codex_home_env: Option<std::ffi::OsString>,
        home_dir: Option<PathBuf>,
    ) -> Result<Self, AuraHomeError> {
        if let Some(raw) = aura_home_env {
            let path = PathBuf::from(raw);
            if !path.is_absolute() {
                return Err(AuraHomeError::NotAbsolute(path));
            }
            return Ok(Self {
                path,
                source: AuraHomeSource::AuraHomeEnv,
            });
        }
        if let Some(raw) = codex_home_env {
            let path = PathBuf::from(raw);
            if !path.is_absolute() {
                return Err(AuraHomeError::NotAbsolute(path));
            }
            return Ok(Self {
                path,
                source: AuraHomeSource::CodexHomeEnv,
            });
        }
        let home = home_dir.ok_or(AuraHomeError::NoHome)?;
        Ok(Self {
            path: home.join(".aura"),
            source: AuraHomeSource::DefaultUnderHome,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ffi::OsString;
    use std::path::PathBuf;

    fn os(s: &str) -> OsString {
        OsString::from(s)
    }

    // `PathBuf::is_absolute` differs by platform: `/etc/aura` is
    // absolute on Unix but NOT on Windows. Keep the fixtures
    // cfg-gated so the inner tests work on both.
    #[cfg(unix)]
    const SAMPLE_AURA: &str = "/etc/aura";
    #[cfg(unix)]
    const SAMPLE_CODEX: &str = "/etc/codex";
    #[cfg(unix)]
    const SAMPLE_HOME: &str = "/home/u";
    #[cfg(unix)]
    const RELATIVE_SAMPLE: &str = "relative/path";

    #[cfg(windows)]
    const SAMPLE_AURA: &str = r"C:\ProgramData\aura";
    #[cfg(windows)]
    const SAMPLE_CODEX: &str = r"C:\ProgramData\codex";
    #[cfg(windows)]
    const SAMPLE_HOME: &str = r"C:\Users\u";
    #[cfg(windows)]
    const RELATIVE_SAMPLE: &str = r"relative\path";

    #[test]
    fn aura_home_env_wins() {
        let h = AuraHome::resolve_with(
            Some(os(SAMPLE_AURA)),
            Some(os(SAMPLE_CODEX)),
            Some(PathBuf::from(SAMPLE_HOME)),
        )
        .expect("absolute override must parse");
        assert_eq!(h.path, PathBuf::from(SAMPLE_AURA));
        assert_eq!(h.source, AuraHomeSource::AuraHomeEnv);
    }

    #[test]
    fn codex_home_alias_used_when_aura_absent() {
        let h = AuraHome::resolve_with(
            None,
            Some(os(SAMPLE_CODEX)),
            Some(PathBuf::from(SAMPLE_HOME)),
        )
        .expect("absolute override must parse");
        assert_eq!(h.path, PathBuf::from(SAMPLE_CODEX));
        assert_eq!(h.source, AuraHomeSource::CodexHomeEnv);
    }

    #[test]
    fn default_falls_back_to_home_dot_aura() {
        let h = AuraHome::resolve_with(None, None, Some(PathBuf::from(SAMPLE_HOME)))
            .expect("home-relative default must succeed");
        assert_eq!(h.path, PathBuf::from(SAMPLE_HOME).join(".aura"));
        assert_eq!(h.source, AuraHomeSource::DefaultUnderHome);
    }

    #[test]
    fn no_home_errors() {
        let err = AuraHome::resolve_with(None, None, None).expect_err("must error");
        assert!(matches!(err, AuraHomeError::NoHome));
    }

    #[test]
    fn relative_aura_home_rejected() {
        let err = AuraHome::resolve_with(
            Some(os(RELATIVE_SAMPLE)),
            None,
            Some(PathBuf::from(SAMPLE_HOME)),
        )
        .expect_err("relative path must be rejected");
        assert!(matches!(err, AuraHomeError::NotAbsolute(_)));
    }

    #[test]
    fn relative_codex_home_rejected() {
        let err = AuraHome::resolve_with(
            None,
            Some(os(RELATIVE_SAMPLE)),
            Some(PathBuf::from(SAMPLE_HOME)),
        )
        .expect_err("relative path must be rejected");
        assert!(matches!(err, AuraHomeError::NotAbsolute(_)));
    }
}
