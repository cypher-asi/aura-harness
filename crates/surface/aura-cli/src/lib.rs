//! # aura-cli
//!
//! Layer: surface
//!
//! The `aura` command line. The root `src/main.rs` entry point is a thin
//! shim that delegates to [`run_cli`]; this crate owns argument parsing,
//! the subcommands (sign-in, plugins, run traces, evals) and the
//! ratatui terminal launcher. The agent session itself is `aura-app`'s:
//! this crate turns its flags into `aura-app` options. The `aura-node`
//! binary body lives in [`aura_node::run_node`], re-exported here as
//! [`run_node`].
//!
//! ## Mode resolution
//!
//! The `aura --mode <agent|plan|ask|debug>` flag ([`ModeFlag`]) is the
//! requested mode for the interactive terminal session. It resolves via
//! [`aura_config::session_mode`]: the flag, else the daemon default
//! ([`aura_config::AgentConfig::default_mode`]), else
//! [`AgentMode::Agent`]. The result lands on the kernel's
//! `PolicyConfig::mode`, which gates every tool call. The headless node
//! ignores the flag; it resolves a mode per `POST /v1/run` request.
//!
//! ## Invariants ([`.cursor/rules.md`] §13)
//!
//! - No upward dependency above the surface layer.
//! - No `anyhow` in any function signature *exposed to library
//!   consumers*; [`run`] / [`run_node`] return `anyhow::Result`
//!   because they are the binary entry-points and the root
//!   `main.rs` files immediately bubble the error to the process
//!   exit code, matching the same boundary the previous root
//!   binaries had.

#![forbid(unsafe_code)]

use aura_core_modes::{AgentMode, ApprovalLevel};
use clap::ValueEnum;
use thiserror::Error;

mod auth_commands;
mod changelog_commands;
pub mod cli;
mod cloud_commands;
pub mod evals;
mod gate_commands;
mod last_selection;
mod runner;
mod trace_commands;

pub use runner::{desktop_launch, run, run_cli, runtime_builder, runtime_thread_start};

/// The `aura-node` entrypoint; `aura-node` owns the gateway binary.
pub use aura_node::run_node;

/// `clap`-friendly mirror of [`AgentMode`].
///
/// Identical variants and serde representation; lives here so the
/// `aura --mode <agent|plan|ask|debug>` CLI surface stays a thin
/// surface-layer concern.
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
#[clap(rename_all = "lower")]
pub enum ModeFlag {
    /// Full unrestricted operation subject to user permissions.
    Agent,
    /// Read + markdown writes + read-only subprocesses.
    Plan,
    /// Read-only (the terminal's `read-only` mode).
    #[value(alias = "read-only")]
    Ask,
    /// Read + sandboxed probes only.
    Debug,
}

impl From<ModeFlag> for AgentMode {
    fn from(value: ModeFlag) -> Self {
        match value {
            ModeFlag::Agent => AgentMode::Agent,
            ModeFlag::Plan => AgentMode::Plan,
            ModeFlag::Ask => AgentMode::Ask,
            ModeFlag::Debug => AgentMode::Debug,
        }
    }
}

impl From<AgentMode> for ModeFlag {
    fn from(value: AgentMode) -> Self {
        match value {
            AgentMode::Agent => ModeFlag::Agent,
            AgentMode::Plan => ModeFlag::Plan,
            AgentMode::Ask => ModeFlag::Ask,
            AgentMode::Debug => ModeFlag::Debug,
        }
    }
}

/// `clap`-friendly mirror of [`ApprovalLevel`], for `aura --approval`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
#[clap(rename_all = "lower")]
pub enum ApprovalFlag {
    /// Run everything; commands can write only inside the working folder.
    Yolo,
    /// Ask for anything not on the project's allow list.
    Ask,
    /// Ask for everything except reads, even what the allow list covers.
    Strict,
}

impl From<ApprovalFlag> for ApprovalLevel {
    fn from(value: ApprovalFlag) -> Self {
        match value {
            ApprovalFlag::Yolo => ApprovalLevel::Yolo,
            ApprovalFlag::Ask => ApprovalLevel::Ask,
            ApprovalFlag::Strict => ApprovalLevel::Strict,
        }
    }
}

/// Errors raised by surface-layer CLI utilities.
#[derive(Debug, Error)]
pub enum CliError {
    /// The supplied CLI argument did not parse as an
    /// [`AgentMode`]. The string is the offending input.
    #[error("invalid --mode value `{0}`; expected one of agent|plan|ask|debug")]
    InvalidMode(String),
}

/// Parse a free-form `--mode <name>` argument into an
/// [`AgentMode`].
///
/// Useful when wiring older subcommands that already take a
/// `String` and need a small upgrade path.
///
/// # Errors
///
/// Returns [`CliError::InvalidMode`] on any value not in
/// `{agent, plan, ask, debug}`.
pub fn parse_mode_str(value: &str) -> Result<AgentMode, CliError> {
    let trimmed = value.trim();
    match trimmed {
        "agent" => Ok(AgentMode::Agent),
        "plan" => Ok(AgentMode::Plan),
        "ask" => Ok(AgentMode::Ask),
        "debug" => Ok(AgentMode::Debug),
        other => Err(CliError::InvalidMode(other.to_string())),
    }
}

/// Bundle of CLI inputs that influence the AgentMode resolution
/// priority. Phase 9 surfaces this struct so callers can pass a
/// single typed value into the daemon's resolver.
#[derive(Debug, Clone, Copy, Default)]
pub struct CliModeInputs {
    /// `--mode <agent|plan|ask|debug>` parsed from clap.
    pub cli_flag: Option<AgentMode>,
}

impl CliModeInputs {
    /// Construct from an optional [`ModeFlag`] (the clap-parsed
    /// `Option<ModeFlag>`).
    #[must_use]
    pub fn from_flag(flag: Option<ModeFlag>) -> Self {
        Self {
            cli_flag: flag.map(AgentMode::from),
        }
    }
}

/// Pinned version banner string used by the Phase 9 surface-layer
/// smoke tests. Kept intentionally short so the snapshot pin is
/// trivial to maintain across Phase 10 changes.
#[must_use]
pub fn version_banner() -> String {
    format!(
        "aura {VERSION} — Phase 10 surface composition root",
        VERSION = env!("CARGO_PKG_VERSION")
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mode_flag_roundtrips_to_agent_mode() {
        for mode in [
            AgentMode::Agent,
            AgentMode::Plan,
            AgentMode::Ask,
            AgentMode::Debug,
        ] {
            let flag: ModeFlag = mode.into();
            let back: AgentMode = flag.into();
            assert_eq!(back, mode);
        }
    }

    #[test]
    fn parse_mode_str_accepts_known_values() {
        assert_eq!(parse_mode_str("agent").unwrap(), AgentMode::Agent);
        assert_eq!(parse_mode_str("plan").unwrap(), AgentMode::Plan);
        assert_eq!(parse_mode_str("ask").unwrap(), AgentMode::Ask);
        assert_eq!(parse_mode_str("debug").unwrap(), AgentMode::Debug);
    }

    #[test]
    fn parse_mode_str_rejects_unknown() {
        assert!(matches!(
            parse_mode_str("yolo"),
            Err(CliError::InvalidMode(_))
        ));
    }

    #[test]
    fn version_banner_contains_phase_marker() {
        assert!(version_banner().contains("Phase 10"));
    }
}
