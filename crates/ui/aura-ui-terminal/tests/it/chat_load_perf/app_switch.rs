//! Going round the agents panel's apps, each showing a chat held in memory.

use super::bench::Bench;
use super::fixtures::{conversation, id};
use super::report::print_row;
use aura_ui_terminal::events::{AgentDefinitionSummary, AgentSummary, ChannelSummary};
use aura_ui_terminal::{AgentsTab, App, PaneTab, UiCommand, UiEvent};
use std::time::{Duration, Instant};
use tokio::sync::mpsc::Receiver;

const ROUNDS: usize = 6;

/// Going round the agents panel's apps, each showing a chat held in
/// memory: PROJECTS a listed agent, AGENTS an instance of a permanent
/// agent, ARCHIVE an archived one and CHANNELS a channel's coordinator.
/// Times picking each app and the frame after it, the slowest round of
/// `ROUNDS`, past one round that lays every chat out.
pub(crate) fn app_switch_bench(entries: usize) {
    let (tx, mut rx) = tokio::sync::mpsc::channel(1024);
    let mut bench = Bench::new();
    bench.app = App::new().with_event_sender(tx);
    list_one_agent_per_app(&mut bench.app);
    for agent in 0..4 {
        bench.load_for(agent, conversation(entries));
    }
    bench.frame();

    let apps = [
        AgentsTab::Roster,
        AgentsTab::Archive,
        AgentsTab::Channels,
        AgentsTab::Projects,
    ];
    let (picks, frames) = slowest_switches(&mut bench, &mut rx, &apps);
    println!("\nswitching apps, {entries}-entry chats held, slowest of {ROUNDS} rounds:\n");
    for (i, tab) in apps.iter().enumerate() {
        print_row(&format!("to {}: pick", tab.label()), picks[i]);
        print_row(&format!("to {}: frame after it", tab.label()), frames[i]);
    }
}

/// Lists agents 0 and 1 (an instance of `reviewer`), archived agent 2 and
/// channel coordinator 3, with agent 0 active.
fn list_one_agent_per_app(app: &mut App) {
    let listed = |i: usize, definition: Option<&str>| AgentSummary {
        id: id(i),
        name: format!("Agent {i}"),
        definition: definition.map(str::to_string),
        is_active: i == 0,
        ..AgentSummary::default()
    };
    app.process_command(UiCommand::SetAgents(vec![
        listed(0, None),
        listed(1, Some("reviewer")),
    ]));
    app.process_command(UiCommand::SetArchivedAgents(vec![AgentSummary {
        archived_at_ms: Some(1),
        ..listed(2, None)
    }]));
    app.process_command(UiCommand::SetAgentDefinitions(vec![
        AgentDefinitionSummary {
            name: "reviewer".to_string(),
            description: "Reviews changes".to_string(),
            abbreviation: None,
        },
    ]));
    app.process_command(UiCommand::SetChannels(vec![ChannelSummary {
        id: "c1".to_string(),
        slug: "release".to_string(),
        title: "Release".to_string(),
        coordinator_id: id(3),
        ..ChannelSummary::default()
    }]));
    app.process_command(UiCommand::SetActiveAgent(id(0)));
}

/// The slowest pick and frame after it for each of `apps`, over `ROUNDS`
/// rounds past the first.
fn slowest_switches(
    bench: &mut Bench,
    rx: &mut Receiver<UiEvent>,
    apps: &[AgentsTab],
) -> (Vec<Duration>, Vec<Duration>) {
    let mut picks = vec![Duration::ZERO; apps.len()];
    let mut frames = vec![Duration::ZERO; apps.len()];
    for round in 0..=ROUNDS {
        for (i, tab) in apps.iter().enumerate() {
            let start = Instant::now();
            bench.app.select_pane_tab(PaneTab::Agents(*tab));
            let pick = start.elapsed();
            let frame = bench.frame();
            while rx.try_recv().is_ok() {}
            if round > 0 {
                picks[i] = picks[i].max(pick);
                frames[i] = frames[i].max(frame);
            }
        }
    }
    (picks, frames)
}
