//! Approval level: which tool calls the user is asked about.
//!
//! Orthogonal to [`crate::AgentMode`]: the mode is the ceiling on what an
//! agent may do at all, the level decides which of the calls the mode
//! allows wait for the user's answer first.

use serde::{Deserialize, Serialize};

/// Names accepted for an [`ApprovalLevel`], in the order help text lists
/// them.
pub const APPROVAL_LEVEL_NAMES: &str = "yolo|ask|strict";

/// Which tool calls ask the user before they run.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Default, Serialize, Deserialize,
)]
#[serde(rename_all = "snake_case")]
pub enum ApprovalLevel {
    /// Nothing asks. Commands may only write inside the working
    /// directory (and temp and cache folders).
    Yolo,
    /// Calls that are not reads and not on the project's allow list ask.
    #[default]
    Ask,
    /// Every call that is not a read asks, even ones on the allow list.
    Strict,
}

impl ApprovalLevel {
    /// The level Shift+Tab switches to.
    #[must_use]
    pub const fn next(self) -> Self {
        match self {
            Self::Ask => Self::Strict,
            Self::Strict => Self::Yolo,
            Self::Yolo => Self::Ask,
        }
    }

    /// Name used by `/approval`, `--approval` and in the record.
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::Yolo => "yolo",
            Self::Ask => "ask",
            Self::Strict => "strict",
        }
    }

    /// One line on what the level asks about.
    #[must_use]
    pub const fn description(self) -> &'static str {
        match self {
            Self::Yolo => "runs every tool without asking; commands may only write in this folder",
            Self::Ask => "asks before edits, commands and other risky tools not on the allow list",
            Self::Strict => {
                "asks before every edit, command and other risky tool, even allowed ones"
            }
        }
    }

    /// Whether commands run under the working-directory write confinement.
    #[must_use]
    pub const fn confines_writes(self) -> bool {
        match self {
            Self::Yolo => true,
            Self::Ask | Self::Strict => false,
        }
    }

    /// Parse a level name, accepting a few spellings people reach for.
    /// `None` for anything else.
    #[must_use]
    pub fn parse(name: &str) -> Option<Self> {
        let level = match name.trim().to_ascii_lowercase().as_str() {
            "yolo" | "auto" | "full" | "full-access" => Self::Yolo,
            "ask" | "default" | "normal" => Self::Ask,
            "strict" | "always-ask" | "paranoid" => Self::Strict,
            _ => return None,
        };
        Some(level)
    }
}

impl std::fmt::Display for ApprovalLevel {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.name())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_names_and_aliases() {
        assert_eq!(ApprovalLevel::parse(" YOLO "), Some(ApprovalLevel::Yolo));
        assert_eq!(ApprovalLevel::parse("ask"), Some(ApprovalLevel::Ask));
        assert_eq!(ApprovalLevel::parse("strict"), Some(ApprovalLevel::Strict));
        assert_eq!(ApprovalLevel::parse("chaos"), None);
    }

    #[test]
    fn cycles_through_all_three() {
        let mut level = ApprovalLevel::default();
        let mut seen = Vec::new();
        for _ in 0..3 {
            level = level.next();
            seen.push(level);
        }
        assert_eq!(
            seen,
            [
                ApprovalLevel::Strict,
                ApprovalLevel::Yolo,
                ApprovalLevel::Ask
            ]
        );
    }

    #[test]
    fn name_round_trips_through_serde() {
        for level in [
            ApprovalLevel::Yolo,
            ApprovalLevel::Ask,
            ApprovalLevel::Strict,
        ] {
            let json = serde_json::to_string(&level).unwrap();
            assert_eq!(json, format!("\"{}\"", level.name()));
            assert_eq!(ApprovalLevel::parse(level.name()), Some(level));
        }
    }
}
