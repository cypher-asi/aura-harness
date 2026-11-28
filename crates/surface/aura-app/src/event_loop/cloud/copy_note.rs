//! A cloud turn whose conversation could not be copied here: never
//! silent. The log says so at warn level, with the agent's code and why,
//! and the agent's chat shows a muted note. What this computer holds of
//! the conversation is then an earlier turn's, so the turn is settled
//! without it, from the reply it streamed.

use super::*;

/// What `agent`'s chat says when its turn's conversation was not copied,
/// `why`.
pub(super) fn copy_failed_note(why: &str) -> String {
    let why = why.trim().trim_end_matches('.');
    format!(
        "Couldn't copy this turn's conversation from its cloud machine ({why}). Its report \
         uses the reply it streamed here."
    )
}

/// The code `agent` goes by, for the log; its hex id when it has none.
pub(super) fn agent_code(state: &LoopState<'_>, agent: AgentId) -> String {
    lock(&state.hub.registry)
        .find(agent)
        .and_then(|found| found.code.clone())
        .unwrap_or_else(|| agent.to_hex())
}

/// Say in the log and in `ui`, `code`'s chat, that its turn's
/// conversation was not copied, `why`.
pub(super) async fn report_copy_failed(ui: &AgentUi, code: &str, why: &str) {
    warn!(agent = %code, error = %why, "A cloud turn's conversation was not copied here");
    ui.send(crate::event_loop::handlers::system_message(
        copy_failed_note(why),
    ))
    .await;
}
