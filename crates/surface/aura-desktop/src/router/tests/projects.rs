//! Projects joining tabs, and commands reaching them.

use super::support::*;
use crate::router::*;
use aura_ui::events::{MessageData, MessageRole};
use std::path::Path;

#[test]
fn an_archived_agent_opens_in_the_tab_that_asked_and_nothing_waits_for_it() {
    let (mut tabs, mut events) = two_tabs();
    let archive = UiCommand::SetArchivedAgents(vec![AgentSummary {
        id: "z9".into(),
        name: "Z9".into(),
        ..AgentSummary::default()
    }]);
    tabs.apply(archive.clone());

    tabs.forward(1, UiEvent::SelectAgent("z9".into()));
    assert!(sent(&mut events).contains(&r#"SelectAgent("z9")"#.to_string()));
    // The session's answer, then a refresh of the lists after a turn.
    tabs.apply(UiCommand::SetActiveAgent("z9".into()));
    tabs.apply(list(TWO, "z9"));
    tabs.apply(archive);
    tabs.apply(UiCommand::SetActiveAgent("z9".into()));
    assert_eq!(shown(&tabs), ["a1", "z9"]);
    assert_eq!(tabs.front_index(), 1);

    // What comes next lands at once, in its tab, without waiting for
    // a list that would name the agent.
    let before = messages(&tabs);
    tabs.apply(message("reply"));
    assert_eq!(messages(&tabs), [before[0], before[1] + 1]);
    assert_eq!(shown(&tabs), ["a1", "z9"]);
}

#[test]
fn a_channel_opens_in_its_tab_and_its_chat_is_not_held_back() {
    let (mut tabs, mut events) = two_tabs();
    let channels = UiCommand::SetChannels(vec![aura_ui::events::ChannelSummary {
        id: "ch".into(),
        slug: "zode".into(),
        coordinator_id: "c1".into(),
        members: vec![AgentSummary {
            id: "c1".into(),
            name: "C1".into(),
            ..AgentSummary::default()
        }],
        ..aura_ui::events::ChannelSummary::default()
    }]);
    tabs.apply(channels.clone());

    // CHANNELS opens the channel there is, its coordinator on screen
    // at once, waiting for its chat.
    tabs.front_mut()
        .tui
        .select_pane_tab(aura_ui_terminal::PaneTab::Agents(
            aura_ui_terminal::AgentsTab::Channels,
        ));
    tabs.forward_events();
    let asked = sent(&mut events).join(" ");
    assert!(asked.contains(r#"SelectAgent("c1")"#), "{asked}");
    // Asked for once, with its chat: a `FocusAgent` first would have the
    // session switch, and build its lists, twice.
    assert!(!asked.contains(r#"FocusAgent("c1")"#), "{asked}");
    assert!(asked.contains(r#"OpenChannel("ch")"#), "{asked}");
    assert_eq!(shown(&tabs), ["a1", "c1"]);
    assert!(tabs.front().tui.chat_loading());

    // The session's answer: the coordinator, the lists (which leave it
    // out) and the coordinator again, then its chat.
    tabs.apply(UiCommand::SetActiveAgent("c1".into()));
    tabs.apply(list(TWO, "c1"));
    tabs.apply(channels);
    tabs.apply(UiCommand::SetActiveAgent("c1".into()));
    tabs.apply(UiCommand::ForAgent {
        agent_id: "c1".into(),
        command: Box::new(UiCommand::LoadConversation(vec![MessageData::new(
            MessageRole::Assistant,
            "hello from the coordinator",
        )])),
    });
    assert_eq!(shown(&tabs), ["a1", "c1"]);
    assert!(
        !tabs.front().tui.chat_loading(),
        "the chat is not held back"
    );
    assert_eq!(tabs.front().tui.messages().len(), 1);
}

#[test]
fn the_window_starts_with_every_project_in_one_tab() {
    let (mut tabs, mut events) = tabs();
    start(&mut tabs, ONE, "a1");
    assert_eq!(tabs.len(), 1);
    assert_eq!(tabs.arrangement(), [["a1", "p2"]]);
    assert_eq!(held(&tabs), [["a1", "a1.1", "p2"]]);
    assert_eq!(shown(&tabs), ["a1"]);
    assert_eq!(tabs.front().title(), "A1");

    // Picking another agent of the tab stays in the tab.
    tabs.forward(0, UiEvent::SelectAgent("a1.1".into()));
    switch(&mut tabs, ONE, "a1.1");
    assert_eq!(shown(&tabs), ["a1.1"]);
    assert_eq!(tabs.front().title(), "A1 / A1.1");
    tabs.forward_events();
    assert_eq!(sent(&mut events), [r#"SelectAgent("a1.1")"#]);
}

#[test]
fn plus_opens_a_tab_holding_the_project_picked_only() {
    let (mut tabs, mut events) = tabs();
    start(&mut tabs, ONE, "a1");
    let _ = sent(&mut events);
    tabs.new_tab();
    assert!(tabs.front().tui.folder_picker().is_some());
    tabs.forward_events();
    assert!(
        sent(&mut events).is_empty(),
        "nothing until a folder is picked"
    );
    let event = UiEvent::CreateProject {
        folder: "/new/project".into(),
        switch: true,
    };
    tabs.forward(0, event);
    assert_eq!(
        sent(&mut events),
        [r#"CreateProject { folder: "/new/project", switch: true }"#]
    );
    assert_eq!(tabs.len(), 1, "no tab until the project exists");

    // The session switches to the agent before it lists it.
    let before = messages(&tabs)[0];
    tabs.apply(UiCommand::SetActiveAgent("a3".into()));
    tabs.apply(message("for the new agent"));
    assert_eq!(messages(&tabs), [before], "kept for the new tab");
    tabs.apply(list(TWO, "a3"));
    tabs.apply(UiCommand::SetActiveAgent("a3".into()));

    assert_eq!(tabs.arrangement(), [vec!["a1", "p2"], vec!["a3"]]);
    assert_eq!(held(&tabs), [vec!["a1", "a1.1", "p2"], vec!["a3"]]);
    assert_eq!(shown(&tabs), ["a1", "a3"]);
    assert_eq!(tabs.front_index(), 1);
    assert_eq!(messages(&tabs)[0], before);
    let new = tabs.front().tui.messages();
    assert!(new.iter().any(|m| m.content() == "for the new agent"));
    let active = |tab: &Tab| -> Vec<String> {
        let agents = tab.tui.agents().iter().filter(|a| a.is_active);
        agents.map(|a| a.id.clone()).collect()
    };
    assert_eq!(active(&tabs.tabs[0]), ["a1"]);
    assert_eq!(active(&tabs.tabs[1]), ["a3"]);

    tabs.forward_events();
    assert!(sent(&mut events).is_empty());
}

#[test]
fn a_project_joins_the_tab_that_asked_for_it() {
    let (mut tabs, mut events) = two_tabs();
    tabs.focus(0);
    switch(&mut tabs, TWO, "a1");
    let _ = sent(&mut events);

    let event = UiEvent::CreateProject {
        folder: "/tmp".into(),
        switch: true,
    };
    tabs.forward(0, event);
    assert_eq!(sent(&mut events).len(), 1);
    let more: Agents = &[("a1", 0), ("a1.1", 1), ("p2", 0), ("a3", 0), ("p4", 0)];
    switch(&mut tabs, more, "p4");
    assert_eq!(tabs.arrangement(), [vec!["a1", "p2", "p4"], vec!["a3"]]);
    assert_eq!(shown(&tabs), ["p4", "a3"]);
    assert_eq!(tabs.front_index(), 0);
}

#[test]
fn an_agent_added_to_a_project_stays_in_its_tab() {
    let (mut tabs, _events) = two_tabs();
    let UiCommand::SetAgents(mut agents) = list(TWO, "a1") else {
        unreachable!("list sends agents");
    };
    for agent in &mut agents[..2] {
        agent.project_id = Some("a1".into());
    }
    agents.insert(
        2,
        AgentSummary {
            id: "a1.2".into(),
            name: "A1.2".into(),
            project_id: Some("a1".into()),
            ..AgentSummary::default()
        },
    );
    tabs.apply(UiCommand::SetAgents(agents));
    assert_eq!(tabs.arrangement(), [vec!["a1", "p2"], vec!["a3"]]);
    assert_eq!(held(&tabs), [vec!["a1", "a1.1", "a1.2", "p2"], vec!["a3"]]);
}

#[test]
fn a_project_with_every_agent_archived_stays_in_its_tab_until_removed() {
    let (mut tabs, _events) = two_tabs();

    // p2's agent and then a3, the only one of its tab, are archived.
    tabs.apply(projects(&["a1", "p2", "a3"]));
    tabs.apply(list(&[("a1", 0), ("a1.1", 1), ("a3", 0)], "a3"));
    tabs.apply(list(&[("a1", 0), ("a1.1", 1)], "a1"));
    assert_eq!(tabs.arrangement(), [vec!["a1", "p2"], vec!["a3"]]);
    assert_eq!(listed(&tabs.tabs[0]), ["a1", "p2"]);
    assert_eq!(listed(&tabs.tabs[1]), ["a3"]);

    // Removed, they leave their tabs, and the tab left with nothing
    // closes.
    tabs.apply(projects(&["a1"]));
    tabs.apply(list(&[("a1", 0), ("a1.1", 1)], "a1"));
    assert_eq!(tabs.arrangement(), [["a1"]]);
}

#[test]
fn plus_closed_without_a_folder_opens_no_tab() {
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    let (mut tabs, mut events) = tabs();
    start(&mut tabs, ONE, "a1");
    tabs.new_tab();
    let esc = KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE);
    tabs.front_mut().tui.handle_key(esc);
    tabs.forward_events();
    assert!(tabs.front().tui.folder_picker().is_none());
    let _ = sent(&mut events);

    // A project asked for later, as with Ctrl+N, joins the tab asking.
    let event = UiEvent::CreateProject {
        folder: "/later".into(),
        switch: true,
    };
    tabs.forward(0, event);
    switch(&mut tabs, TWO, "a3");
    assert_eq!(tabs.arrangement(), [["a1", "p2", "a3"]]);
}

#[test]
fn plus_on_a_folder_already_a_project_brings_its_tab_forward() {
    let (mut tabs, mut events) = two_tabs();
    let folder = std::env::temp_dir().canonicalize().expect("temp dir");
    let project = |id: &str, folder: &Path| ProjectSummary {
        id: id.into(),
        name: id.into(),
        folder: folder.display().to_string(),
        ..ProjectSummary::default()
    };
    tabs.apply(UiCommand::SetProjects(vec![
        project("a1", Path::new("/site")),
        project("p2", Path::new("/notes")),
        project("a3", &folder),
    ]));
    tabs.focus(0);
    tabs.forward_events();
    let _ = sent(&mut events);

    tabs.new_tab();
    tabs.forward(
        0,
        UiEvent::CreateProject {
            folder,
            switch: true,
        },
    );
    assert_eq!(tabs.front_index(), 1);
    assert_eq!(tabs.len(), 2);
    assert!(
        !sent(&mut events)
            .iter()
            .any(|e| e.starts_with("CreateProject")),
        "nothing to create"
    );
    // A later project is not taken for the one `+` asked for.
    tabs.focus(0);
    let more: Agents = &[("a1", 0), ("a1.1", 1), ("p2", 0), ("a3", 0), ("p4", 0)];
    switch(&mut tabs, more, "p4");
    assert_eq!(tabs.len(), 2);
}

#[test]
fn a_failed_create_opens_no_tab() {
    let (mut tabs, _events) = tabs();
    start(&mut tabs, ONE, "a1");
    plus(&mut tabs);
    tabs.apply(UiCommand::ShowError("Could not create agent".into()));
    assert_eq!(tabs.len(), 1);
    assert!(matches!(
        tabs.front().tui.notification(),
        Some((text, _)) if text.contains("Could not create agent")
    ));

    // A later project is not taken for the one `+` asked for.
    switch(&mut tabs, TWO, "a3");
    assert_eq!(tabs.arrangement(), [["a1", "p2", "a3"]]);
}

#[test]
fn commands_reach_the_tab_holding_their_agent() {
    let (mut tabs, _events) = two_tabs();
    let before = messages(&tabs);

    // Unaddressed: the tab holding the session's active agent.
    tabs.apply(message("for the active agent"));
    assert_eq!(messages(&tabs), [before[0], before[1] + 1]);

    // Addressed: the tab holding the agent, on screen or not.
    tabs.apply(UiCommand::ForAgent {
        agent_id: "a1.1".into(),
        command: Box::new(UiCommand::RequestApproval {
            id: "r1".into(),
            tool: "run_command".into(),
            description: "cargo test".into(),
            always: String::new(),
        }),
    });
    assert!(tabs.tabs[0].tui.agent_busy("a1.1"));
    assert!(tabs.tabs[0].busy());
    assert!(!tabs.tabs[1].tui.agent_busy("a1.1"));
    assert!(!tabs.tabs[1].busy());

    // Lists that aren't about agents: every tab.
    tabs.apply(UiCommand::SetApiStatus {
        url: Some("http://127.0.0.1:1".into()),
        active: true,
    });
    assert!(tabs.iter().all(|tab| tab.tui.api_active()));

    // Benchmarks: every tab lists every one, whichever tree it is in.
    tabs.apply(UiCommand::BenchList {
        benches: Vec::new(),
    });
    tabs.apply(UiCommand::BenchUpsert(Box::new(
        aura_ui_terminal::events::BenchView {
            id: "c1".into(),
            root: "a1".into(),
            ..Default::default()
        },
    )));
    let listed: Vec<usize> = tabs
        .iter()
        .map(|tab| tab.tui.bench().benches.len())
        .collect();
    assert_eq!(listed, [1, 1]);
}

#[test]
fn a_tab_lists_the_processes_of_its_own_agents() {
    let (mut tabs, _events) = two_tabs();
    let process = |id: &str, agent: &str| ProcessSummary {
        id: id.into(),
        jump_agent_id: agent.into(),
        ..ProcessSummary::default()
    };
    tabs.apply(UiCommand::SetProcesses(vec![
        process("1", "a1.1"),
        process("2", "a3"),
        process("3", ""),
    ]));
    let listed = |tab: &Tab| -> Vec<String> {
        let processes = tab.tui.processes().iter();
        processes.map(|p| p.id.clone()).collect()
    };
    assert_eq!(listed(&tabs.tabs[0]), ["1", "3"]);
    assert_eq!(listed(&tabs.tabs[1]), ["2", "3"]);
}
