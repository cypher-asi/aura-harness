//! UI events that open views.

use crate::event_loop::{gate, trace, LoopState};
use aura_ui::UiEvent;

/// Views of status, Gate, graphs and runs.
pub(super) async fn handle(state: &mut LoopState<'_>, event: UiEvent) {
    match event {
        UiEvent::ShowStatus => trace::status_report(state).await,
        UiEvent::ShowGate => gate::view::show(state).await,
        UiEvent::OpenGraph(id) => trace::open_graph(state, id).await,
        UiEvent::OpenFlow(agent_id) => {
            if let Some(flow) = &state.flow {
                flow.open(state.agent_or_active(&agent_id));
            }
        }
        UiEvent::ListRuns => trace::list(state).await,
        UiEvent::LabelStep { run_id, seq, value } => {
            trace::label(state, &run_id, seq, value).await;
        }
        UiEvent::ShowHelp | UiEvent::ShowHistory(_) => {}
        // `domain_of` routes no other event here.
        _ => {}
    }
}
