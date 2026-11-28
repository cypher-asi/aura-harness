//! Phase 4a: `AURA_HOME` resolution priority order.
//!
//! Uses the pure [`AuraHome::resolve_with`] inner resolver to avoid
//! mutating the process-wide env (which would race other tests that
//! also touch `AURA_HOME` / `CODEX_HOME`).
//!
//! Coverage:
//! 1. `AURA_HOME` env wins over `CODEX_HOME`.
//! 2. `CODEX_HOME` is used when `AURA_HOME` is absent (read-only
//!    compat alias).
//! 3. Default falls back to `{home_dir}/.aura` when both env vars are
//!    unset.

use aura_config::{AuraHome, AuraHomeSource};
use std::ffi::OsString;
use std::path::PathBuf;

fn os(s: &str) -> OsString {
    OsString::from(s)
}

// `PathBuf::is_absolute` is platform-sensitive. Resolution requires
// the override to be absolute, so the fixtures must use a path shape
// that the current OS accepts as absolute.
#[cfg(unix)]
const SAMPLE_AURA: &str = "/etc/aura";
#[cfg(unix)]
const SAMPLE_CODEX: &str = "/etc/codex";
#[cfg(unix)]
const SAMPLE_HOME: &str = "/home/u";

#[cfg(windows)]
const SAMPLE_AURA: &str = r"C:\ProgramData\aura";
#[cfg(windows)]
const SAMPLE_CODEX: &str = r"C:\ProgramData\codex";
#[cfg(windows)]
const SAMPLE_HOME: &str = r"C:\Users\u";

#[test]
fn aura_home_env_wins_over_codex_home() {
    let h = AuraHome::resolve_with(
        Some(os(SAMPLE_AURA)),
        Some(os(SAMPLE_CODEX)),
        Some(PathBuf::from(SAMPLE_HOME)),
    )
    .expect("absolute AURA_HOME must resolve");
    assert_eq!(h.path, PathBuf::from(SAMPLE_AURA));
    assert_eq!(h.source, AuraHomeSource::AuraHomeEnv);
}

#[test]
fn codex_home_used_when_aura_home_absent() {
    let h = AuraHome::resolve_with(
        None,
        Some(os(SAMPLE_CODEX)),
        Some(PathBuf::from(SAMPLE_HOME)),
    )
    .expect("absolute CODEX_HOME alias must resolve");
    assert_eq!(h.path, PathBuf::from(SAMPLE_CODEX));
    assert_eq!(h.source, AuraHomeSource::CodexHomeEnv);
}

#[test]
fn default_uses_home_dot_aura() {
    let h = AuraHome::resolve_with(None, None, Some(PathBuf::from(SAMPLE_HOME)))
        .expect("home-dir fallback must resolve");
    let expected = PathBuf::from(SAMPLE_HOME).join(".aura");
    assert_eq!(h.path, expected);
    assert_eq!(h.source, AuraHomeSource::DefaultUnderHome);
}
