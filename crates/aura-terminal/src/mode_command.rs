//! Terminal permission modes and the `/mode [name]` slash command.
//!
//! A [`PermissionMode`] decides which tool calls run without asking. The
//! event loop maps it onto the kernel policy (session [`AgentMode`]
//! ceiling plus per-tool `on`/`ask` states) and records a `System` entry
//! before the agent's next tool call is evaluated under it.

use aura_core_modes::AgentMode;
use serde::{Deserialize, Serialize};
use thiserror::Error;

/// Names accepted by `/mode`, in the order help text lists them.
pub const MODE_NAMES: &str = "default|accept-edits|plan|unrestricted|ask|debug";

/// Which tool calls an agent may make without asking.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum PermissionMode {
    /// Reads run; file edits, commands, spawns and anything unknown ask.
    #[default]
    Default,
    /// Like [`Self::Default`], but file edits run without asking.
    AcceptEdits,
    /// Read-only planning: the kernel's `Plan` ceiling (reads, markdown
    /// notes, read-only commands); what it allows still asks.
    Plan,
    /// Every tool runs without asking.
    Unrestricted,
    /// The kernel's `Ask` ceiling: reads only. Not in the Shift+Tab cycle.
    Ask,
    /// The kernel's `Debug` ceiling: reads and sandboxed probes. Not in
    /// the Shift+Tab cycle.
    Debug,
}

impl PermissionMode {
    /// The mode Shift+Tab switches to. `Ask` and `Debug` are only set on
    /// purpose, so cycling from them starts over at `Default`.
    #[must_use]
    pub const fn next(self) -> Self {
        match self {
            Self::Default => Self::AcceptEdits,
            Self::AcceptEdits => Self::Plan,
            Self::Plan => Self::Unrestricted,
            Self::Unrestricted | Self::Ask | Self::Debug => Self::Default,
        }
    }

    /// Name used by `/mode` and in the record.
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::Default => "default",
            Self::AcceptEdits => "accept-edits",
            Self::Plan => "plan",
            Self::Unrestricted => "unrestricted",
            Self::Ask => "ask",
            Self::Debug => "debug",
        }
    }

    /// One line on what the mode lets the agent do.
    #[must_use]
    pub const fn description(self) -> &'static str {
        match self {
            Self::Default => "asks before edits, commands and other risky tools",
            Self::AcceptEdits => "edits files without asking; commands still ask",
            Self::Plan => "read-only planning; markdown notes and read-only commands ask",
            Self::Unrestricted => "runs every tool without asking",
            Self::Ask => "reads only",
            Self::Debug => "reads and sandboxed probes only",
        }
    }

    /// The kernel's session-mode ceiling for this mode.
    #[must_use]
    pub const fn agent_mode(self) -> AgentMode {
        match self {
            Self::Default | Self::AcceptEdits | Self::Unrestricted => AgentMode::Agent,
            Self::Plan => AgentMode::Plan,
            Self::Ask => AgentMode::Ask,
            Self::Debug => AgentMode::Debug,
        }
    }

    /// The mode a session started with `--mode` / `AURA_DEFAULT_MODE`
    /// begins in. Lossless: each ceiling keeps its own mode.
    #[must_use]
    pub const fn from_agent_mode(mode: AgentMode) -> Self {
        match mode {
            AgentMode::Agent => Self::Default,
            AgentMode::Plan => Self::Plan,
            AgentMode::Ask => Self::Ask,
            AgentMode::Debug => Self::Debug,
        }
    }

    /// Parse a mode name, accepting a few spellings people reach for.
    ///
    /// # Errors
    ///
    /// Returns [`ModeCommandError::UnknownMode`] for any other value.
    pub fn parse(name: &str) -> Result<Self, ModeCommandError> {
        let mode = match name.trim().to_ascii_lowercase().as_str() {
            "default" | "agent" | "normal" => Self::Default,
            "accept-edits" | "acceptedits" | "accept_edits" | "edits" | "auto-edit" => {
                Self::AcceptEdits
            }
            "plan" => Self::Plan,
            "unrestricted" | "yolo" | "full" | "full-access" => Self::Unrestricted,
            "ask" | "read-only" | "readonly" => Self::Ask,
            "debug" => Self::Debug,
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
    /// empty after trimming and [`ModeCommandError::UnknownMode`] for any
    /// other unknown value.
    pub fn parse(arg: &str) -> Result<Self, ModeCommandError> {
        if arg.trim().is_empty() {
            return Err(ModeCommandError::MissingArgument);
        }
        PermissionMode::parse(arg).map(|requested| Self { requested })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_every_mode_and_its_aliases() {
        for (name, mode) in [
            ("default", PermissionMode::Default),
            ("agent", PermissionMode::Default),
            (" accept-edits ", PermissionMode::AcceptEdits),
            ("edits", PermissionMode::AcceptEdits),
            ("plan", PermissionMode::Plan),
            ("Unrestricted", PermissionMode::Unrestricted),
            ("yolo", PermissionMode::Unrestricted),
            ("ask", PermissionMode::Ask),
            ("debug", PermissionMode::Debug),
        ] {
            assert_eq!(SlashModeCommand::parse(name).unwrap().requested, mode);
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
    fn shift_tab_cycles_through_four_modes() {
        let mut mode = PermissionMode::Default;
        let mut seen = Vec::new();
        for _ in 0..4 {
            mode = mode.next();
            seen.push(mode);
        }
        assert_eq!(
            seen,
            [
                PermissionMode::AcceptEdits,
                PermissionMode::Plan,
                PermissionMode::Unrestricted,
                PermissionMode::Default
            ]
        );
        assert_eq!(PermissionMode::Ask.next(), PermissionMode::Default);
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
}
