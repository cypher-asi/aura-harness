//! Terminal work modes, approval levels, and the `/mode [name]` and
//! `/approval [level]` slash commands.
//!
//! Two settings decide what an agent may do without asking:
//!
//! - A [`PermissionMode`] is the kind of work: `agent`, `plan`,
//!   `read-only` or `debug`, each the kernel's session [`AgentMode`]
//!   ceiling.
//! - An [`ApprovalLevel`] (`yolo`, `ask`, `strict`) decides which of the
//!   calls that ceiling allows wait for the user first.
//!
//! The terminal sends them as `UiEvent::SetMode` (`/mode`, `/plan`,
//! `/build`) and `UiEvent::SetApproval` (`/approval`, Shift+Tab); the
//! event loop maps both onto the kernel policy (session mode ceiling plus
//! per-tool `on`/`ask` states) through `Kernel::switch_permissions`, which
//! records a `mode_change` entry before the agent's next tool call is
//! evaluated under it.

use aura_core_modes::AgentMode;
pub use aura_core_modes::{ApprovalLevel, APPROVAL_LEVEL_NAMES};
use serde::{Deserialize, Serialize};
use thiserror::Error;

/// Names accepted by `/mode`, in the order help text lists them.
pub const MODE_NAMES: &str = "agent|plan|read-only|debug";

/// The kind of work an agent does: the kernel's session-mode ceiling.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum PermissionMode {
    /// Reads, edits, commands, spawns: whatever the approval level lets
    /// through.
    #[default]
    #[serde(alias = "default", alias = "accept-edits", alias = "unrestricted")]
    Agent,
    /// Read-only planning: the kernel's `Plan` ceiling (reads, markdown
    /// notes, read-only commands).
    Plan,
    /// The kernel's `Ask` ceiling: reads only.
    #[serde(alias = "ask")]
    ReadOnly,
    /// The kernel's `Debug` ceiling: reads and sandboxed probes.
    Debug,
}

impl PermissionMode {
    /// Name used by `/mode` and in the record.
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::Agent => "agent",
            Self::Plan => "plan",
            Self::ReadOnly => "read-only",
            Self::Debug => "debug",
        }
    }

    /// One line on what the mode lets the agent do.
    #[must_use]
    pub const fn description(self) -> &'static str {
        match self {
            Self::Agent => "reads, edits, commands and spawns, as the approval level allows",
            Self::Plan => "read-only planning; markdown notes and read-only commands",
            Self::ReadOnly => "reads only",
            Self::Debug => "reads and sandboxed probes only",
        }
    }

    /// The kernel's session-mode ceiling for this mode.
    #[must_use]
    pub const fn agent_mode(self) -> AgentMode {
        match self {
            Self::Agent => AgentMode::Agent,
            Self::Plan => AgentMode::Plan,
            Self::ReadOnly => AgentMode::Ask,
            Self::Debug => AgentMode::Debug,
        }
    }

    /// The mode a session started with `--mode` / `AURA_DEFAULT_MODE`
    /// begins in. Lossless: each ceiling keeps its own mode.
    #[must_use]
    pub const fn from_agent_mode(mode: AgentMode) -> Self {
        match mode {
            AgentMode::Agent => Self::Agent,
            AgentMode::Plan => Self::Plan,
            AgentMode::Ask => Self::ReadOnly,
            AgentMode::Debug => Self::Debug,
        }
    }

    /// Parse a mode name, accepting a few spellings people reach for.
    ///
    /// # Errors
    ///
    /// Returns [`ModeCommandError::MovedToApproval`] for the names that
    /// used to pick how much asks (`accept-edits`, `unrestricted`,
    /// `yolo`) and [`ModeCommandError::UnknownMode`] for any other value.
    pub fn parse(name: &str) -> Result<Self, ModeCommandError> {
        let mode = match name.trim().to_ascii_lowercase().as_str() {
            "agent" | "default" | "normal" | "build" => Self::Agent,
            "plan" => Self::Plan,
            "read-only" | "readonly" | "read_only" | "ask" => Self::ReadOnly,
            "debug" => Self::Debug,
            moved @ ("accept-edits" | "acceptedits" | "accept_edits" | "edits" | "auto-edit"
            | "unrestricted" | "yolo" | "full" | "full-access") => {
                return Err(ModeCommandError::MovedToApproval(moved.to_string()));
            }
            other => return Err(ModeCommandError::UnknownMode(other.to_string())),
        };
        Ok(mode)
    }
}

impl std::fmt::Display for PermissionMode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.name())
    }
}

/// Errors raised when parsing a `/mode <name>` slash command.
#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum ModeCommandError {
    /// The slash command body did not match any known [`PermissionMode`].
    #[error("unknown mode `{0}`; expected one of {MODE_NAMES}")]
    UnknownMode(String),
    /// The name picked how much asks, which is now the approval level.
    #[error(
        "`{0}` is no longer a mode; how much asks is the approval level: \
         /approval {APPROVAL_LEVEL_NAMES} (or Shift+Tab)"
    )]
    MovedToApproval(String),
    /// The slash command had no argument.
    #[error("`/mode` requires a mode name; expected one of {MODE_NAMES}")]
    MissingArgument,
}

/// Parsed `/mode <name>` slash-command payload.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SlashModeCommand {
    /// The parsed mode the operator typed.
    pub requested: PermissionMode,
}

impl SlashModeCommand {
    /// Parse the body of a `/mode <name>` line. Whitespace around the
    /// argument is trimmed.
    ///
    /// # Errors
    ///
    /// Returns [`ModeCommandError::MissingArgument`] when the argument is
    /// empty after trimming, and the errors of [`PermissionMode::parse`]
    /// otherwise.
    pub fn parse(arg: &str) -> Result<Self, ModeCommandError> {
        if arg.trim().is_empty() {
            return Err(ModeCommandError::MissingArgument);
        }
        PermissionMode::parse(arg).map(|requested| Self { requested })
    }
}

/// Errors raised when parsing an `/approval <level>` slash command.
#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum ApprovalCommandError {
    /// The argument did not match any [`ApprovalLevel`].
    #[error("unknown approval level `{0}`; expected one of {APPROVAL_LEVEL_NAMES}")]
    UnknownLevel(String),
    /// The slash command had no argument.
    #[error("`/approval` requires a level; expected one of {APPROVAL_LEVEL_NAMES}")]
    MissingArgument,
}

/// Parse the body of an `/approval <level>` line.
///
/// # Errors
///
/// Returns [`ApprovalCommandError::MissingArgument`] for an empty argument
/// and [`ApprovalCommandError::UnknownLevel`] for an unknown one.
pub fn parse_approval_command(arg: &str) -> Result<ApprovalLevel, ApprovalCommandError> {
    let arg = arg.trim();
    if arg.is_empty() {
        return Err(ApprovalCommandError::MissingArgument);
    }
    ApprovalLevel::parse(arg).ok_or_else(|| ApprovalCommandError::UnknownLevel(arg.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_every_mode_and_its_aliases() {
        for (name, mode) in [
            ("agent", PermissionMode::Agent),
            ("default", PermissionMode::Agent),
            ("plan", PermissionMode::Plan),
            (" Read-Only ", PermissionMode::ReadOnly),
            ("ask", PermissionMode::ReadOnly),
            ("debug", PermissionMode::Debug),
        ] {
            assert_eq!(SlashModeCommand::parse(name).unwrap().requested, mode);
        }
    }

    #[test]
    fn old_approval_names_point_to_approval() {
        for name in ["accept-edits", "unrestricted", "yolo"] {
            let err = SlashModeCommand::parse(name).unwrap_err();
            assert!(matches!(&err, ModeCommandError::MovedToApproval(s) if s == name));
            assert!(err.to_string().contains("/approval"));
        }
    }

    #[test]
    fn rejects_unknown_mode() {
        let err = SlashModeCommand::parse("chaos").unwrap_err();
        assert!(matches!(err, ModeCommandError::UnknownMode(s) if s == "chaos"));
    }

    #[test]
    fn rejects_empty_argument() {
        assert!(matches!(
            SlashModeCommand::parse("   "),
            Err(ModeCommandError::MissingArgument)
        ));
    }

    #[test]
    fn agent_mode_round_trips() {
        for mode in [
            AgentMode::Agent,
            AgentMode::Plan,
            AgentMode::Ask,
            AgentMode::Debug,
        ] {
            assert_eq!(PermissionMode::from_agent_mode(mode).agent_mode(), mode);
        }
    }

    #[test]
    fn old_serialized_modes_still_load() {
        for (json, mode) in [
            ("\"default\"", PermissionMode::Agent),
            ("\"unrestricted\"", PermissionMode::Agent),
            ("\"accept-edits\"", PermissionMode::Agent),
            ("\"ask\"", PermissionMode::ReadOnly),
            ("\"read-only\"", PermissionMode::ReadOnly),
        ] {
            assert_eq!(serde_json::from_str::<PermissionMode>(json).unwrap(), mode);
        }
    }

    #[test]
    fn parses_approval_levels() {
        assert_eq!(
            parse_approval_command(" strict "),
            Ok(ApprovalLevel::Strict)
        );
        assert_eq!(
            parse_approval_command(""),
            Err(ApprovalCommandError::MissingArgument)
        );
        assert!(matches!(
            parse_approval_command("chaos"),
            Err(ApprovalCommandError::UnknownLevel(s)) if s == "chaos"
        ));
    }
}
