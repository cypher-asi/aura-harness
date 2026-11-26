//! Agent-loop limits for running a model outside the terminal's selection.

use super::*;

/// What it takes to point an agent loop at a model without changing the
/// terminal's selected model (used for permanent agents with their own
/// `model`).
#[derive(Debug, Clone, Copy)]
pub struct ModelLimits(pub(super) RouterLimits);

/// The effort Gate chose for the work `config` runs, while the effort is
/// Gate's to choose: `None` when the user pinned one or Gate chose none.
pub fn routed_effort(config: &AgentLoopConfig) -> Option<ThinkingEffort> {
    config
        .user_thinking_effort
        .is_none()
        .then_some(config.routed_thinking_effort)
        .flatten()
}

impl ModelLimits {
    /// Configure `config` to run model `id`.
    pub fn configure(self, config: &mut AgentLoopConfig, id: &str) {
        let limits = self.0;
        config.model = id.to_string();
        config.routed_thinking_effort = None;
        config.max_context_tokens = Some(aura_engine::model_context::context_window_for_model(id));
        match backend_for(id) {
            ModelBackend::Local => {
                config.stream_timeout = LOCAL_STREAM_TIMEOUT;
                config.stream_event_timeout = LOCAL_STREAM_TIMEOUT;
            }
            ModelBackend::Router
            | ModelBackend::Claude
            | ModelBackend::Chatgpt
            | ModelBackend::OpenRouter => {
                config.stream_timeout = limits.stream_timeout;
                config.stream_event_timeout = limits.stream_event_timeout;
            }
        }
    }

    /// Point `config` at the model and reasoning effort Gate's `decision`
    /// chose.
    pub fn route(self, config: &mut AgentLoopConfig, decision: &aura_gate::route::Decision) {
        if config.model != decision.model.id {
            self.configure(config, &decision.model.id);
        }
        config.routed_thinking_effort = Some(decision.effort);
    }

    /// Limits taken from `config` as the router settings.
    #[cfg(test)]
    pub(crate) const fn of(config: &AgentLoopConfig) -> Self {
        Self(RouterLimits {
            stream_timeout: config.stream_timeout,
            stream_event_timeout: config.stream_event_timeout,
        })
    }
}
