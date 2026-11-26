//! Model and thinking-level selection handlers, plus chat system lines.

use crate::event_loop::LoopState;
use aura_ui::events::{MessageData, MessageRole};
use aura_ui::UiCommand;
use tracing::info;

/// A system line for the chat.
pub(in crate::event_loop) fn system_message(content: String) -> UiCommand {
    UiCommand::ShowMessage(MessageData {
        role: MessageRole::System,
        content,
        is_streaming: false,
        timestamp_ms: None,
        command: None,
        activity: None,
        model: None,
        usage: None,
        sources: Vec::new(),
        boot: None,
    })
}

pub(in crate::event_loop) async fn show_system_message(state: &LoopState<'_>, content: String) {
    let _ = state.commands.send(system_message(content)).await;
}

/// Probe the local model server, publish the model list, and explain how
/// to get the local default running when it isn't.
pub(in crate::event_loop) async fn announce_models(state: &mut LoopState<'_>) {
    state.models.refresh_local().await;
    state.auth.refresh_plan_tiers().await;
    let summary = crate::event_loop::auth::auth_summary(
        &state.auth,
        state.agent_loop.config_mut().auth_token.as_deref(),
    );
    state.models.set_auth(summary);
    if let Some(note) = state.models.refresh_plans(state.auth.as_ref()).await {
        show_system_message(state, note).await;
    }
    state.models.refresh_route(state.agent_loop.config_mut());
    let _ = state.commands.send(state.models.ui_command()).await;
    if let Some(hint) = state.models.local_hint() {
        show_system_message(state, hint).await;
    }
}

pub(in crate::event_loop) async fn handle_select_model(state: &mut LoopState<'_>, id: &str) {
    let typed = id.trim();
    if needs_local_probe(&state.models, typed) {
        state.models.refresh_local().await;
    }
    match state.models.select(state.agent_loop.config_mut(), typed) {
        Ok(()) => {
            info!(model = %state.models.current(), typed, "Model selected");
            let display = state.models.display();
            let message = if state.models.current().eq_ignore_ascii_case(typed) {
                format!("Model: {display}")
            } else {
                format!("Model: {display} — matched \"{typed}\"")
            };
            let _ = state.commands.send(UiCommand::ShowSuccess(message)).await;
            if let Some(hint) = state.models.local_hint() {
                show_system_message(state, hint).await;
            }
        }
        Err(problem) => {
            let _ = state.commands.send(UiCommand::ShowWarning(problem)).await;
        }
    }
    let _ = state.commands.send(state.models.ui_command()).await;
}

/// Whether selecting `typed` should first re-probe the local server,
/// which can take a few seconds: for `gate` (whose pick may be local), a
/// local id, and an alias that names no model or a local one among the
/// models listed now (a local model is listed only once the server was
/// probed with it installed). A cloud alias such as `opus` or a cloud id
/// switches without waiting.
fn needs_local_probe(models: &crate::model_selection::ModelSelection, typed: &str) -> bool {
    use aura_model::model_catalog::{backend_for, ModelBackend, GATE_MODEL_ID};
    let is_local = |id: &str| backend_for(id) == ModelBackend::Local;
    typed.eq_ignore_ascii_case(GATE_MODEL_ID)
        || is_local(typed)
        || (crate::model_selection::may_be_alias(typed)
            && models.resolve(typed).is_none_or(|id| is_local(&id)))
}

/// Pin the thinking level of the next turns to `level`, or hand it back
/// to each model for `None`.
pub(in crate::event_loop) async fn handle_set_thinking(
    state: &mut LoopState<'_>,
    level: Option<&str>,
) {
    let command = match state
        .models
        .set_thinking(state.agent_loop.config_mut(), level)
    {
        Ok(()) => {
            let shown = state.models.thinking_display();
            info!(thinking = shown, "Thinking level set");
            UiCommand::ShowSuccess(format!("Thinking: {shown}"))
        }
        Err(problem) => UiCommand::ShowWarning(problem),
    };
    let _ = state.commands.send(command).await;
    let _ = state.commands.send(state.models.ui_command()).await;
}
