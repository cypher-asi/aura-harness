//! Default system prompt for the agent loop (`AgentLoopConfig::system_prompt`).
//!
//! Callers set this on `AgentLoopConfig`; the default is an empty string.

use super::SystemPromptBuilder;

/// Default system prompt for the chat / TUI surfaces that did not
/// arrive with a baked-in prompt or any of the typed identity /
/// project_info wire fields.
///
/// Chat-WS migration follow-up: the historical hand-rolled prose was
/// replaced with the canonical `<chat_capabilities>` builder preset so
/// every harness-emitted system prompt reads as the same bracketed
/// envelope. The dev-loop and chat WS paths now go through
/// [`SystemPromptBuilder`] directly; this preset covers the residual
/// callers (the TUI's `default_agent_config` and the chat session's
/// `apply_init` fallback when no typed fields and no legacy prompt
/// were sent).
#[must_use]
pub fn default_system_prompt() -> String {
    SystemPromptBuilder::new().chat_capabilities().build()
}
