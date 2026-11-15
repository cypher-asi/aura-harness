//! Typed `/mode <name>` slash-command parser.
//!
//! Not wired into the TUI input loop yet: changing mode mid-session
//! must append a System record before the next tool proposal, so the
//! `/mode` command is not part of the documented resolution chain.

use aura_core_modes::AgentMode;
use thiserror::Error;

/// Errors raised when parsing a `/mode <name>` slash command.
#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum ModeCommandError {
    /// The slash command body did not match any known [`AgentMode`].
    #[error("unknown mode `{0}`; expected one of agent|plan|ask|debug")]
    UnknownMode(String),
    /// The slash command had no argument.
    #[error("`/mode` requires a mode name; expected one of agent|plan|ask|debug")]
    MissingArgument,
}

/// Parsed `/mode <name>` slash-command payload.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SlashModeCommand {
    /// The parsed mode the operator typed.
    pub requested: AgentMode,
}

impl SlashModeCommand {
    /// Parse the body of a `/mode <name>` line.
    ///
    /// Accepts the lower-snake names that match the
    /// [`AgentMode`] serde representation. Whitespace around the
    /// argument is trimmed.
    ///
    /// # Errors
    ///
    /// Returns [`ModeCommandError::MissingArgument`] when the
    /// argument is empty after trimming and
    /// [`ModeCommandError::UnknownMode`] for any other unknown
    /// value.
    pub fn parse(arg: &str) -> Result<Self, ModeCommandError> {
        let trimmed = arg.trim();
        if trimmed.is_empty() {
            return Err(ModeCommandError::MissingArgument);
        }
        let mode = match trimmed {
            "agent" => AgentMode::Agent,
            "plan" => AgentMode::Plan,
            "ask" => AgentMode::Ask,
            "debug" => AgentMode::Debug,
            other => return Err(ModeCommandError::UnknownMode(other.to_string())),
        };
        Ok(Self { requested: mode })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_all_four_modes() {
        assert_eq!(
            SlashModeCommand::parse("agent").unwrap().requested,
            AgentMode::Agent
        );
        assert_eq!(
            SlashModeCommand::parse(" plan ").unwrap().requested,
            AgentMode::Plan
        );
        assert_eq!(
            SlashModeCommand::parse("ask").unwrap().requested,
            AgentMode::Ask
        );
        assert_eq!(
            SlashModeCommand::parse("debug").unwrap().requested,
            AgentMode::Debug
        );
    }

    #[test]
    fn rejects_unknown_mode() {
        let err = SlashModeCommand::parse("yolo").unwrap_err();
        assert!(matches!(err, ModeCommandError::UnknownMode(s) if s == "yolo"));
    }

    #[test]
    fn rejects_empty_argument() {
        assert!(matches!(
            SlashModeCommand::parse("   "),
            Err(ModeCommandError::MissingArgument)
        ));
    }
}
