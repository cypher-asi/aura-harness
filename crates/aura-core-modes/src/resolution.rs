//! Session [`AgentMode`] resolution.
//!
//! A session's mode is resolved once, at session start, by the surface
//! that owns the session (the CLI runner, the HTTP/WS gateway, or the
//! automaton bridge) and threaded into the kernel's policy config.
//! Child agents never re-resolve: they inherit the parent's mode and
//! may only narrow it during subagent derivation.

use crate::modes::AgentMode;

/// Inputs to [`resolve_session_mode`], highest precedence first.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct AgentModeInputs {
    /// Mode explicitly requested for this session: the `--mode` CLI
    /// flag or the `mode` field on a `RuntimeRequest`.
    pub requested: Option<AgentMode>,
    /// Daemon-wide default (`FleetConfig::default_mode`, overridable
    /// via `AURA_FLEET_DEFAULT_MODE`).
    pub daemon_default: Option<AgentMode>,
}

/// Resolve the session mode: requested, then daemon default, then
/// [`AgentMode::Agent`].
#[must_use]
pub fn resolve_session_mode(inputs: AgentModeInputs) -> AgentMode {
    inputs
        .requested
        .or(inputs.daemon_default)
        .unwrap_or(AgentMode::Agent)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn falls_back_to_agent_when_nothing_is_set() {
        assert_eq!(
            resolve_session_mode(AgentModeInputs::default()),
            AgentMode::Agent
        );
    }

    #[test]
    fn daemon_default_applies_when_nothing_is_requested() {
        let inputs = AgentModeInputs {
            requested: None,
            daemon_default: Some(AgentMode::Plan),
        };
        assert_eq!(resolve_session_mode(inputs), AgentMode::Plan);
    }

    #[test]
    fn requested_mode_beats_daemon_default() {
        let inputs = AgentModeInputs {
            requested: Some(AgentMode::Ask),
            daemon_default: Some(AgentMode::Plan),
        };
        assert_eq!(resolve_session_mode(inputs), AgentMode::Ask);
    }
}
