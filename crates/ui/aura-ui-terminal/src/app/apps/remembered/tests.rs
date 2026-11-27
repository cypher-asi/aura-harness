//! Saving the selection and bringing it back in a fresh `App`, as the
//! next run of the app would.

use crate::app::{AgentsTab, App, LastSelection, ProjectPick};
use crate::events::{AgentSummary, ChannelSummary, UiCommand, UiEvent};
use tokio::sync::mpsc;

fn agent(id: &str, project: &str) -> AgentSummary {
    AgentSummary {
        id: id.to_string(),
        name: id.to_uppercase(),
        project_id: Some(project.to_string()),
        ..AgentSummary::default()
    }
}

/// `aa` and `ab` in project `p1`, `bb` in `p2`.
fn agents() -> Vec<AgentSummary> {
    vec![agent("aa", "p1"), agent("ab", "p1"), agent("bb", "p2")]
}

/// `#zode`, coordinated by `kk`, which the projects list leaves out.
fn zode() -> ChannelSummary {
    ChannelSummary {
        id: "c1".to_string(),
        slug: "zode".to_string(),
        coordinator_id: "kk".to_string(),
        members: vec![AgentSummary {
            id: "kk".to_string(),
            ..AgentSummary::default()
        }],
        ..ChannelSummary::default()
    }
}

fn pick(project: &str, agent: &str) -> ProjectPick {
    ProjectPick {
        project: project.to_string(),
        agent: agent.to_string(),
    }
}

/// A fresh app, given `saved` as a front end would on starting.
fn app_with(saved: Option<LastSelection>) -> (App, mpsc::Receiver<UiEvent>) {
    let (tx, rx) = mpsc::channel(16);
    let mut app = App::new().with_event_sender(tx);
    if let Some(saved) = saved {
        app.restore_selection(saved);
    }
    (app, rx)
}

/// What the session sends first, in its order: the agents, the channels,
/// then the agent it opens on.
fn open_session(app: &mut App, channels: Vec<ChannelSummary>, active: &str) {
    app.process_command(UiCommand::SetAgents(agents()));
    app.process_command(UiCommand::SetChannels(channels));
    app.process_command(UiCommand::SetActiveAgent(active.to_string()));
}

/// The events the app sent since last asked, as their debug text:
/// `UiEvent` has no `PartialEq`.
fn sent(rx: &mut mpsc::Receiver<UiEvent>) -> Vec<String> {
    std::iter::from_fn(|| rx.try_recv().ok())
        .map(|event| format!("{event:?}"))
        .collect()
}

fn text(event: &UiEvent) -> String {
    format!("{event:?}")
}

/// The selection a run left on PROJECTS showing `ab`.
fn left_on_ab() -> LastSelection {
    let (mut app, _rx) = app_with(None);
    open_session(&mut app, vec![zode()], "aa");
    app.request_agent_switch("ab".to_string());
    app.process_command(UiCommand::SetActiveAgent("ab".to_string()));
    app.last_selection()
}

/// The selection a run left on CHANNELS with `#zode` open, after PROJECTS
/// showed `bb`.
fn left_in_zode() -> LastSelection {
    let (mut app, _rx) = app_with(None);
    open_session(&mut app, vec![zode()], "bb");
    app.show_agents_tab(AgentsTab::Channels);
    app.process_command(UiCommand::SetActiveAgent("kk".to_string()));
    app.last_selection()
}

#[test]
fn the_agent_picked_in_projects_is_saved_by_project() {
    let saved = left_on_ab();
    assert!(!saved.channels_in_front);
    assert_eq!(saved.projects.first(), Some(&pick("p1", "ab")));
}

#[test]
fn the_next_run_opens_projects_on_the_agent_it_was_left_on() {
    let (mut app, mut rx) = app_with(Some(left_on_ab()));
    open_session(&mut app, vec![zode()], "aa");
    assert_eq!(app.agents_tab(), AgentsTab::Projects);
    assert_eq!(app.active_agent_id(), "ab");
    assert!(
        sent(&mut rx).contains(&text(&UiEvent::SelectAgent("ab".to_string()))),
        "the session is asked for the agent's chat"
    );
}

#[test]
fn the_agent_the_session_opens_on_already_is_left_alone() {
    let (mut app, mut rx) = app_with(Some(left_on_ab()));
    open_session(&mut app, vec![zode()], "ab");
    assert_eq!(app.active_agent_id(), "ab");
    assert!(sent(&mut rx).is_empty());
}

#[test]
fn the_open_channel_is_saved_with_channels_in_front() {
    let saved = left_in_zode();
    assert!(saved.channels_in_front);
    assert_eq!(saved.channel.as_deref(), Some("c1"));
    assert_eq!(saved.projects.first(), Some(&pick("p2", "bb")));
}

#[test]
fn the_next_run_opens_the_channel_it_was_left_on() {
    let (mut app, mut rx) = app_with(Some(left_in_zode()));
    open_session(&mut app, vec![zode()], "kk");
    assert_eq!(app.agents_tab(), AgentsTab::Channels);
    assert_eq!(app.open_channel().map(|c| c.id.as_str()), Some("c1"));
    assert!(sent(&mut rx).contains(&text(&UiEvent::OpenChannel("c1".to_string()))));
}

#[test]
fn leaving_the_restored_channel_gives_projects_its_last_agent() {
    let (mut app, mut rx) = app_with(Some(left_in_zode()));
    open_session(&mut app, vec![zode()], "kk");
    app.show_agents_tab(AgentsTab::Projects);
    assert_eq!(app.active_agent_id(), "bb");
    assert!(sent(&mut rx).contains(&text(&UiEvent::SelectAgent("bb".to_string()))));
}

#[test]
fn a_channel_no_longer_listed_falls_back_to_projects() {
    let (mut app, _rx) = app_with(Some(left_in_zode()));
    open_session(&mut app, Vec::new(), "aa");
    assert_eq!(app.agents_tab(), AgentsTab::Projects);
    assert!(!app.channel_open());
    assert_eq!(app.active_agent_id(), "bb", "PROJECTS still gets its agent");
    assert_eq!(app.last_selection().channel, None);
}

#[test]
fn an_agent_no_longer_listed_falls_back_to_the_next_project_s_pick() {
    let saved = LastSelection {
        projects: vec![pick("p9", "gone"), pick("p2", "bb")],
        ..LastSelection::default()
    };
    let (mut app, _rx) = app_with(Some(saved));
    open_session(&mut app, Vec::new(), "aa");
    assert_eq!(app.active_agent_id(), "bb");
}

#[test]
fn nothing_still_listed_leaves_the_session_s_agent_on_screen() {
    let saved = LastSelection {
        channels_in_front: true,
        channel: Some("gone".to_string()),
        projects: vec![pick("p9", "gone")],
    };
    let (mut app, mut rx) = app_with(Some(saved));
    open_session(&mut app, vec![zode()], "aa");
    assert_eq!(app.agents_tab(), AgentsTab::Projects);
    assert_eq!(app.active_agent_id(), "aa");
    assert!(sent(&mut rx).is_empty());
}

#[test]
fn without_a_saved_selection_the_app_opens_as_it_always_did() {
    for saved in [None, Some(LastSelection::default())] {
        let (mut app, mut rx) = app_with(saved);
        open_session(&mut app, vec![zode()], "aa");
        assert_eq!(app.agents_tab(), AgentsTab::Projects);
        assert_eq!(app.active_agent_id(), "aa");
        assert!(sent(&mut rx).is_empty());
    }
}

#[test]
fn the_selection_is_brought_back_only_once() {
    let (mut app, _rx) = app_with(Some(left_on_ab()));
    open_session(&mut app, vec![zode()], "aa");
    app.process_command(UiCommand::SetActiveAgent("ab".to_string()));
    app.request_agent_switch("bb".to_string());
    app.process_command(UiCommand::SetActiveAgent("bb".to_string()));
    assert_eq!(app.active_agent_id(), "bb");
}

#[test]
fn a_selection_not_yet_brought_back_is_saved_as_it_came() {
    let saved = left_in_zode();
    let (app, _rx) = app_with(Some(saved.clone()));
    assert_eq!(app.last_selection(), saved);
}

#[test]
fn each_project_keeps_the_agent_it_was_last_on() {
    let (mut app, _rx) = app_with(None);
    open_session(&mut app, Vec::new(), "ab");
    for agent in ["aa", "bb"] {
        app.request_agent_switch(agent.to_string());
        app.process_command(UiCommand::SetActiveAgent(agent.to_string()));
    }
    assert_eq!(
        app.last_selection().projects,
        vec![pick("p2", "bb"), pick("p1", "aa")]
    );
}

#[test]
fn picks_of_other_projects_are_kept_for_later_runs() {
    let saved = LastSelection {
        projects: vec![pick("p1", "ab"), pick("p7", "elsewhere")],
        ..LastSelection::default()
    };
    let (mut app, _rx) = app_with(Some(saved));
    open_session(&mut app, Vec::new(), "aa");
    app.request_agent_switch("bb".to_string());
    app.process_command(UiCommand::SetActiveAgent("bb".to_string()));
    assert_eq!(
        app.last_selection().projects,
        vec![pick("p2", "bb"), pick("p1", "ab"), pick("p7", "elsewhere")]
    );
}
