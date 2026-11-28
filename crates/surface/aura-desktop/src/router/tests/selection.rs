//! Each tab coming back on what it had selected in the last run.

use super::support::*;
use crate::router::*;
use aura_ui_terminal::{AgentsTab, LastSelection, ProjectPick};

fn pick(project: &str, agent: &str) -> ProjectPick {
    ProjectPick {
        project: project.into(),
        agent: agent.into(),
    }
}

fn picks(picks: &[(&str, &str)]) -> LastSelection {
    LastSelection {
        projects: picks.iter().map(|(p, a)| pick(p, a)).collect(),
        ..LastSelection::default()
    }
}

/// `#zode`, coordinated by `kk`, in no project.
fn zode() -> UiCommand {
    UiCommand::SetChannels(vec![aura_ui::events::ChannelSummary {
        id: "c1".into(),
        slug: "zode".into(),
        coordinator_id: "kk".into(),
        members: vec![AgentSummary {
            id: "kk".into(),
            ..AgentSummary::default()
        }],
        ..aura_ui::events::ChannelSummary::default()
    }])
}

#[test]
fn the_tab_comes_back_on_the_agent_it_was_left_on() {
    let (mut tabs, mut events) = tabs();
    tabs.remember(picks(&[("a1", "a1.1")]));
    start(&mut tabs, ONE, "a1");
    tabs.forward_events();
    assert_eq!(tabs.front().agent(), "a1.1");
    assert!(
        sent(&mut events).contains(&r#"SelectAgent("a1.1")"#.to_string()),
        "the session is asked for its chat"
    );
}

#[test]
fn a_tab_brought_to_the_front_shows_the_agent_it_was_left_on() {
    let (mut tabs, mut events) = tabs_with(&[&["a3"], &["a1", "p2"]]);
    tabs.remember(picks(&[("a3", "a3"), ("p2", "p2")]));
    start(&mut tabs, TWO, "a3");
    tabs.forward_events();
    assert_eq!(shown(&tabs), ["a3", ""]);
    let _ = sent(&mut events);

    tabs.focus(1);
    assert_eq!(sent(&mut events), [r#"SelectAgent("p2")"#]);
    switch(&mut tabs, TWO, "p2");
    tabs.forward_events();
    assert_eq!(shown(&tabs), ["a3", "p2"]);
    assert!(sent(&mut events).is_empty(), "no second switch");
}

#[test]
fn the_tab_the_session_opens_on_brings_back_the_open_channel() {
    let (mut tabs, mut events) = tabs();
    tabs.remember(LastSelection {
        channels_in_front: true,
        channel: Some("c1".into()),
        projects: vec![pick("p2", "p2")],
    });
    tabs.apply(list(ONE, "kk"));
    tabs.apply(zode());
    tabs.apply(UiCommand::SetActiveAgent("kk".into()));
    tabs.forward_events();
    assert_eq!(tabs.front().tui.agents_tab(), AgentsTab::Channels);
    assert!(sent(&mut events).contains(&r#"OpenChannel("c1")"#.to_string()));

    let saved = tabs.last_selection();
    assert!(saved.channels_in_front);
    assert_eq!(saved.channel.as_deref(), Some("c1"));
}

#[test]
fn a_tab_opened_later_does_not_open_the_channel() {
    let (mut tabs, _events) = tabs_with(&[&["a1"], &["p2"]]);
    tabs.remember(LastSelection {
        channels_in_front: true,
        channel: Some("c1".into()),
        projects: Vec::new(),
    });
    tabs.apply(zode());
    tabs.apply(list(ONE, "kk"));
    tabs.apply(UiCommand::SetActiveAgent("kk".into()));
    assert_eq!(tabs.len(), 2);
    assert_eq!(tabs.front_index(), 0);
    tabs.focus(1);
    switch(&mut tabs, ONE, "p2");
    assert_eq!(
        tabs.iter()
            .map(|tab| tab.tui.agents_tab())
            .collect::<Vec<_>>(),
        [AgentsTab::Channels, AgentsTab::Projects]
    );
}

#[test]
fn saving_takes_each_project_s_pick_from_the_tab_holding_it() {
    let (mut tabs, _events) = tabs_with(&[&["a1", "p2"], &["a3"]]);
    tabs.remember(picks(&[("p9", "gone"), ("a1", "a1.1")]));
    start(&mut tabs, TWO, "a1");
    tabs.forward_events();
    switch(&mut tabs, TWO, "a1.1");
    tabs.focus(1);
    switch(&mut tabs, TWO, "a3");
    assert_eq!(tabs.front_index(), 1);
    assert_eq!(
        tabs.last_selection().projects,
        [pick("a3", "a3"), pick("a1", "a1.1"), pick("p9", "gone")]
    );
}

#[test]
fn without_a_saved_selection_the_tabs_open_as_they_always_did() {
    let (mut tabs, mut events) = tabs();
    tabs.remember(LastSelection::default());
    start(&mut tabs, ONE, "a1");
    tabs.forward_events();
    assert_eq!(shown(&tabs), ["a1"]);
    assert!(sent(&mut events).is_empty());
}
