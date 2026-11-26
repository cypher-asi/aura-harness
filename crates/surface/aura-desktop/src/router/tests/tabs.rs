//! Tabs brought forward, restored, closed and moved.

use super::support::*;
use crate::router::*;

/// Whether tab `tab` lists agent `id` as in plan mode, if it lists it.
fn plan_mode_in(tabs: &Tabs, tab: usize, id: &str) -> Option<bool> {
    let agents = tabs.tabs[tab].tui.agents().iter();
    agents.filter(|a| a.id == id).map(|a| a.plan_mode).next()
}

fn set_mode_for(agent_id: &str, mode: aura_ui_terminal::PermissionMode) -> UiCommand {
    UiCommand::ForAgent {
        agent_id: agent_id.into(),
        command: Box::new(UiCommand::SetMode(mode)),
    }
}

#[test]
fn a_plan_mode_switch_survives_the_lists_sent_on_to_tabs() {
    use aura_ui_terminal::PermissionMode;
    let (mut tabs, _events) = two_tabs();

    // a1.1, off screen in the back tab, goes into plan mode.
    tabs.apply(set_mode_for("a1.1", PermissionMode::Plan));
    assert_eq!(plan_mode_in(&tabs, 0, "a1.1"), Some(true));
    assert_eq!(plan_mode_in(&tabs, 0, "a1"), Some(false));
    assert_eq!(plan_mode_in(&tabs, 1, "a1.1"), None, "not its tab's");

    // New projects re-send each tab its agents from the router's copy.
    tabs.apply(projects(&["a1", "p2", "a3"]));
    assert_eq!(plan_mode_in(&tabs, 0, "a1.1"), Some(true), "still marked");
    assert_eq!(plan_mode_in(&tabs, 0, "a1"), Some(false));
    assert_eq!(plan_mode_in(&tabs, 1, "a3"), Some(false));

    // a3, on screen in the front tab, too.
    tabs.apply(set_mode_for("a3", PermissionMode::Plan));
    tabs.apply(projects(&["a1", "p2", "a3"]));
    assert_eq!(plan_mode_in(&tabs, 1, "a3"), Some(true));
    assert_eq!(plan_mode_in(&tabs, 0, "a1.1"), Some(true));

    // Out of plan mode, the next re-send keeps it unmarked.
    tabs.apply(set_mode_for("a1.1", PermissionMode::Agent));
    assert_eq!(plan_mode_in(&tabs, 0, "a1.1"), Some(false));
    tabs.apply(projects(&["a1", "p2", "a3"]));
    assert_eq!(plan_mode_in(&tabs, 0, "a1.1"), Some(false));
    assert_eq!(plan_mode_in(&tabs, 1, "a3"), Some(true));

    // The session's own list says who is in plan mode from then on.
    tabs.apply(list(TWO, "a3"));
    assert_eq!(plan_mode_in(&tabs, 1, "a3"), Some(false));
}

#[test]
fn bringing_a_tab_forward_asks_the_session_once() {
    let (mut tabs, mut events) = two_tabs();
    tabs.focus(0);
    tabs.forward_events();
    assert_eq!(sent(&mut events), [r#"FocusAgent("a1")"#]);

    switch(&mut tabs, TWO, "a1");
    assert_eq!(shown(&tabs), ["a1", "a3"]);
    assert_eq!(tabs.front_index(), 0);
    tabs.forward_events();
    assert!(sent(&mut events).is_empty());

    tabs.next();
    tabs.previous();
    assert_eq!(
        sent(&mut events),
        [r#"FocusAgent("a3")"#, r#"FocusAgent("a1")"#]
    );
}

#[test]
fn the_tabs_of_the_last_run_come_back() {
    let (mut tabs, mut events) = tabs_with(&[&["gone", "p2"], &["a1"], &["missing"]]);
    start(&mut tabs, TWO, "a1");
    // `a3` is new to the arrangement and joins the first tab.
    assert_eq!(tabs.arrangement(), [vec!["p2", "a3"], vec!["a1"]]);
    assert_eq!(held(&tabs), [vec!["p2", "a3"], vec!["a1", "a1.1"]]);
    assert_eq!(tabs.front_index(), 1, "the session's agent is in front");
    assert_eq!(shown(&tabs), ["", "a1"]);
    tabs.forward_events();
    assert!(sent(&mut events).is_empty());

    // A tab that has shown nothing yet loads its first project.
    tabs.focus(0);
    assert_eq!(sent(&mut events), [r#"SelectAgent("p2")"#]);
    switch(&mut tabs, TWO, "p2");
    assert_eq!(shown(&tabs), ["p2", "a1"]);
    assert_eq!(tabs.front_index(), 0);
}

#[test]
fn a_project_with_every_agent_archived_comes_back_in_its_tab() {
    let (mut tabs, _events) = tabs_with(&[&["a1", "p2"], &["a3"]]);
    tabs.apply(projects(&["a1", "p2", "a3"]));
    start(&mut tabs, &[("a1", 0), ("a1.1", 1), ("a3", 0)], "a3");
    assert_eq!(tabs.arrangement(), [vec!["a1", "p2"], vec!["a3"]]);
    assert_eq!(listed(&tabs.tabs[0]), ["a1", "p2"]);
    assert_eq!(listed(&tabs.tabs[1]), ["a3"]);
}

#[test]
fn an_agent_of_another_tab_is_shown_in_its_own_tab() {
    let (mut tabs, mut events) = two_tabs();
    tabs.forward(1, UiEvent::SelectAgent("a1".into()));
    tabs.forward_events();
    assert_eq!(tabs.front_index(), 0);
    assert_eq!(sent(&mut events), [r#"FocusAgent("a1")"#]);

    tabs.forward(0, UiEvent::SelectAgent("a3".into()));
    switch(&mut tabs, TWO, "a1");
    assert_eq!(tabs.front_index(), 1);
    assert_eq!(shown(&tabs), ["a1", "a3"]);
}

#[test]
fn a_tab_that_is_not_active_gets_the_session_first() {
    let (mut tabs, mut events) = two_tabs();
    tabs.forward(0, UiEvent::NewSession);
    assert_eq!(sent(&mut events), [r#"FocusAgent("a1")"#, "NewSession"]);
    // The tab in front takes the session back.
    tabs.forward_events();
    assert_eq!(sent(&mut events), [r#"FocusAgent("a3")"#]);
}

#[test]
fn closing_a_tab_moves_its_projects_to_the_one_before() {
    let (mut tabs, mut events) = two_tabs();
    assert!(tabs.close(1));
    assert_eq!(tabs.arrangement(), [["a1", "p2", "a3"]]);
    assert_eq!(held(&tabs), [["a1", "a1.1", "p2", "a3"]]);
    assert_eq!(tabs.front_index(), 0);
    assert_eq!(sent(&mut events), [r#"FocusAgent("a1")"#]);
    assert!(!tabs.close(0), "the last tab stays");

    // The first tab's projects move to the one after it.
    let (mut tabs, _events) = two_tabs();
    assert!(tabs.close(0));
    assert_eq!(tabs.arrangement(), [["a3", "a1", "p2"]]);
    assert_eq!(shown(&tabs), ["a3"]);
}

#[test]
fn moving_a_tab_keeps_its_projects_and_the_tab_in_front() {
    let (mut tabs, mut events) = two_tabs();
    tabs.move_tab(1, 0);
    assert_eq!(tabs.arrangement(), [vec!["a3"], vec!["a1", "p2"]]);
    assert_eq!(held(&tabs), [vec!["a3"], vec!["a1", "a1.1", "p2"]]);
    assert_eq!(tabs.front_index(), 0);
    assert_eq!(tabs.front().agent(), "a3");
    tabs.forward_events();
    assert!(sent(&mut events).is_empty());

    // Another tab moving past the one in front shifts it.
    tabs.move_tab(1, 0);
    assert_eq!(tabs.arrangement(), [vec!["a1", "p2"], vec!["a3"]]);
    assert_eq!(tabs.front_index(), 1);
    assert_eq!(tabs.front().agent(), "a3");

    tabs.move_tab(0, 5);
    assert_eq!(tabs.arrangement(), [vec!["a1", "p2"], vec!["a3"]]);
}

#[test]
fn a_dragged_order_of_projects_is_listed_and_saved() {
    let (mut tabs, _events) = tabs();
    start(&mut tabs, TWO, "a1");
    assert_eq!(held(&tabs), [["a1", "a1.1", "p2", "a3"]]);

    // The front tab's list was reordered by a drag.
    let dragged: Agents = &[("p2", 0), ("a1", 0), ("a1.1", 1), ("a3", 0)];
    tabs.tabs[0].tui.process_command(list(dragged, "a1"));
    tabs.adopt_project_order();
    assert_eq!(tabs.arrangement(), [["p2", "a1", "a3"]]);

    // The session, told of the drag, lists its agents in that order.
    tabs.apply(list(dragged, "a1"));
    assert_eq!(held(&tabs), [["p2", "a1", "a1.1", "a3"]]);

    // A new project goes last; the order survives the file.
    let more: Agents = &[("p2", 0), ("a1", 0), ("a1.1", 1), ("a3", 0), ("p4", 0)];
    tabs.apply(list(more, "a1"));
    assert_eq!(held(&tabs), [["p2", "a1", "a1.1", "a3", "p4"]]);
    let path = std::env::temp_dir().join(format!("aura_order_{}", std::process::id()));
    save_arrangement(&path, &tabs.arrangement()).expect("temp dir is writable");
    let saved = load_arrangement(&path);
    std::fs::remove_file(&path).expect("just written");

    let restore: Vec<Vec<&str>> = saved
        .iter()
        .map(|tab| tab.iter().map(String::as_str).collect())
        .collect();
    let restore: Vec<&[&str]> = restore.iter().map(Vec::as_slice).collect();
    let (mut tabs, _events) = tabs_with(&restore);
    start(&mut tabs, more, "a1");
    assert_eq!(held(&tabs), [["p2", "a1", "a1.1", "a3", "p4"]]);
}

#[test]
fn the_arrangement_survives_a_round_trip_through_its_file() {
    let path = std::env::temp_dir().join(format!("aura_tabs_{}", std::process::id()));
    let arrangement: Arrangement = vec![vec!["a1".into(), "p2".into()], vec!["a3".into()]];
    save_arrangement(&path, &arrangement).expect("temp dir is writable");
    assert_eq!(load_arrangement(&path), arrangement);
    std::fs::remove_file(&path).expect("just written");
    assert!(load_arrangement(&path).is_empty());
}

#[test]
fn quit_ends_the_session_from_any_tab() {
    let (mut tabs, mut events) = two_tabs();
    tabs.forward(1, UiEvent::Quit);
    assert_eq!(sent(&mut events), ["Quit"]);
}
