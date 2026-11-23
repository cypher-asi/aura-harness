//! `/compare <args>`: what each subcommand does, and the list and the
//! view of one comparison it shows.

use super::ended::{apply, diff, dismiss};
use super::lifecycle::{cancel, start};
use super::prepare::prepare;
use super::record::comparisons;
use super::view;
use crate::event_loop::agent_events::AgentUi;
use crate::event_loop::handlers::system_message;
use crate::event_loop::LoopState;
use aura_core_types::{AgentId, ComparePayload};
use aura_gate::compare::{parse, CompareCommand};
use aura_ui_terminal::UiCommand;
use std::fmt::Write;

/// `/compare <args>` from the chat of `agent_id`.
pub(crate) async fn handle(state: &mut LoopState<'_>, agent_id: AgentId, args: &str) {
    let ui = AgentUi::new(state.commands.clone(), agent_id);
    let command = match parse(args) {
        Ok(command) => command,
        Err(problem) => {
            ui.send(UiCommand::ShowWarning(problem)).await;
            return;
        }
    };
    state.park_finished_spawns().await;
    let shown = match command {
        CompareCommand::List => Ok(list(state)),
        CompareCommand::Show(n) => show(state, n),
        CompareCommand::Start(request) => prepare(state, agent_id, request)
            .await
            .map(|text| text + "\n  /compare go starts it · /compare cancel drops it"),
        CompareCommand::Go => start(state).await,
        CompareCommand::Cancel => Ok(cancel(state)),
        CompareCommand::Apply(run) => apply(state, None, &run).await,
        CompareCommand::Diff(run) => diff(state, &ui, None, run.as_deref()).await,
        CompareCommand::Dismiss => dismiss(state, None).await,
    };
    match shown {
        Ok(text) if text.is_empty() => {}
        Ok(text) => ui.send(system_message(text)).await,
        Err(problem) => ui.send(UiCommand::ShowWarning(problem)).await,
    }
}

/// `/compare list`: every comparison, then the one running and the one
/// waiting to start.
fn list(state: &LoopState<'_>) -> String {
    let all: Vec<(String, Option<ComparePayload>)> = comparisons(state)
        .into_iter()
        .map(|(_, name, payload)| (name, payload))
        .collect();
    let mut text = view::list(&all);
    if let Some(running) = state.compare.running() {
        let _ = write!(
            text,
            "\n{} is running; /compare cancel stops it.",
            running.name
        );
    }
    if state.compare.pending.is_some() {
        text.push_str("\nA comparison is waiting: /compare go starts it.");
    }
    text
}

/// `/compare show [n]`: comparison `n`, else the latest one recorded.
fn show(state: &LoopState<'_>, n: Option<usize>) -> Result<String, String> {
    let all = comparisons(state);
    let chosen = match n {
        Some(n) => all.get(n - 1),
        None => all.iter().rev().find(|(_, _, payload)| payload.is_some()),
    };
    match chosen {
        Some((_, name, Some(payload))) => Ok(view::render(name, payload)),
        Some((_, name, None)) => Err(format!("{name} has no record")),
        None => Err("No such comparison; /compare lists them".to_string()),
    }
}
