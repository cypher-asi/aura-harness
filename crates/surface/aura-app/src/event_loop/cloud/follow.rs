//! The cloud turns an earlier run of Aura asked for and never saw end:
//! their machines worked on while Aura was closed, or after it crashed.
//! Each is followed to its end as a turn asked from here is, its chat
//! shown from its record as the hub copies it.

use super::forward::{chat_reload, title_sink, Titles, TurnExtras};
use super::submit::{note_turn, watch_turn};
use super::*;
use crate::cloud::hub::TurnMark;

/// Said in the chat of a followed turn.
const FOLLOWED: &str = "This turn kept running on its cloud machine while Aura was closed. \
                        Its chat fills in as the machine's work is copied here.";

/// Follow each turn `hub`'s machines were asked for that no hub saw end.
pub(super) async fn follow_unfinished(state: &LoopState<'_>, hub: &Arc<CloudHub>) {
    for (agent, mark) in hub.unfinished_turns() {
        if state.cloud.is_running(agent) {
            continue;
        }
        info!(%agent, run = %mark.run_id, "Following a cloud turn that ran on while Aura was closed");
        follow(state, hub, agent, mark).await;
    }
}

/// Follow `agent`'s turn `mark`: it works, shows in PROCESSES and can be
/// stopped as a turn started here can, and what it was asked settles
/// when it ends.
async fn follow(state: &LoopState<'_>, hub: &Arc<CloudHub>, agent: AgentId, mark: TurnMark) {
    let cancel = CancellationToken::new();
    let model = mark.model.as_deref().unwrap_or_default();
    let mut row = crate::event_loop::handlers::turn_summary(agent, &mark.label, model);
    row.model.clone_from(&mark.model);
    row.started_ms = mark.started_ms;
    note_turn(state, agent, row, &cancel).await;
    let ui = AgentUi::new(state.commands.clone(), agent);
    // Its machine sends the title again as the run's history replays.
    let titles = (mark.titled && state.hub.titles_turn(agent, true)).then(|| {
        Titles::new(
            title_sink(Arc::clone(&state.hub), Arc::clone(hub), agent),
            &mark.label,
        )
    });
    let extras = TurnExtras {
        titles,
        context_limit: mark
            .model
            .as_deref()
            .map(aura_engine::context_window_for_model),
        reload: Some(chat_reload(Arc::clone(&state.hub), ui.clone(), agent)),
        from_record: true,
        waiting: None,
        // Its run is known already; the save after this event keeps it.
        run_started: None,
    };
    ui.send(UiCommand::ShowMessage(MessageData::new(
        MessageRole::System,
        FOLLOWED,
    )))
    .await;
    let rx = hub.follow_turn(agent, mark, cancel.clone());
    watch_turn(state, agent, ui, rx, extras, cancel);
}
