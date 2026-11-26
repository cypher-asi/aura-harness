//! Projects moved between tabs.

use super::support::*;
use crate::router::*;
use aura_ui_terminal::WorkspaceTarget;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseButton};

/// Three tabs of one project each, `a3`'s in front.
fn three_tabs() -> (Tabs, mpsc::Receiver<UiEvent>) {
    let (mut tabs, mut events) = tabs_with(&[&["a1"], &["p2"], &["a3"]]);
    start(&mut tabs, TWO, "a3");
    assert_eq!(tabs.front_index(), 2);
    let _ = sent(&mut events);
    (tabs, events)
}

#[test]
fn a_project_moves_to_another_tab() {
    let (mut tabs, mut events) = two_tabs();
    tabs.move_project(0, "p2", WorkspaceTarget::Existing(1));
    assert_eq!(tabs.arrangement(), [vec!["a1"], vec!["a3", "p2"]]);
    assert_eq!(held(&tabs), [vec!["a1", "a1.1"], vec!["p2", "a3"]]);
    assert_eq!(shown(&tabs), ["a1", "a3"]);
    assert_eq!(tabs.front_index(), 1);
    tabs.forward_events();
    assert!(sent(&mut events).is_empty(), "no tab lost what it shows");
}

#[test]
fn a_project_moves_to_a_new_tab() {
    let (mut tabs, mut events) = two_tabs();
    tabs.move_project(0, "p2", WorkspaceTarget::New);
    assert_eq!(tabs.arrangement(), [vec!["a1"], vec!["a3"], vec!["p2"]]);
    assert_eq!(held(&tabs), [vec!["a1", "a1.1"], vec!["a3"], vec!["p2"]]);
    assert_eq!(tabs.front_index(), 1, "the tab in front stays");
    assert!(sent(&mut events).is_empty());

    // Brought forward, the new tab loads its project.
    tabs.focus(2);
    assert_eq!(sent(&mut events), [r#"SelectAgent("p2")"#]);
}

#[test]
fn moving_the_last_project_of_a_tab_closes_it() {
    let (mut tabs, mut events) = two_tabs();
    tabs.move_project(1, "a3", WorkspaceTarget::Existing(0));
    assert_eq!(tabs.arrangement(), [["a1", "p2", "a3"]]);
    assert_eq!(held(&tabs), [["a1", "a1.1", "p2", "a3"]]);
    assert_eq!(tabs.front_index(), 0, "the tab it went to comes forward");
    assert_eq!(shown(&tabs), ["a1"]);
    assert_eq!(sent(&mut events), [r#"FocusAgent("a1")"#]);

    // A tab closing before the one in front shifts it.
    let (mut tabs, mut events) = three_tabs();
    tabs.move_project(0, "a1", WorkspaceTarget::Existing(1));
    assert_eq!(tabs.arrangement(), [vec!["p2", "a1"], vec!["a3"]]);
    assert_eq!(tabs.front_index(), 1);
    assert_eq!(tabs.front().agent(), "a3");
    assert!(sent(&mut events).is_empty());

    // The one in front closing, after the tab it goes to.
    let (mut tabs, _events) = three_tabs();
    tabs.move_project(2, "a3", WorkspaceTarget::Existing(1));
    assert_eq!(tabs.arrangement(), [vec!["a1"], vec!["p2", "a3"]]);
    assert_eq!(tabs.front_index(), 1);
}

#[test]
fn moving_the_project_a_tab_shows_loads_its_next() {
    let (mut tabs, mut events) = two_tabs();
    tabs.focus(0);
    switch(&mut tabs, TWO, "a1");
    let _ = sent(&mut events);

    tabs.move_project(0, "a1", WorkspaceTarget::Existing(1));
    assert_eq!(tabs.arrangement(), [vec!["p2"], vec!["a3", "a1"]]);
    assert_eq!(tabs.front_index(), 0, "the tab in front stays");
    assert_eq!(sent(&mut events), [r#"SelectAgent("p2")"#]);
    tabs.forward_events();
    assert!(sent(&mut events).is_empty(), "asked once");

    switch(&mut tabs, TWO, "p2");
    assert_eq!(shown(&tabs), ["p2", "a3"]);
    assert_eq!(held(&tabs), [vec!["p2"], vec!["a1", "a1.1", "a3"]]);
    assert_eq!(tabs.front_index(), 0);
    tabs.forward_events();
    assert!(sent(&mut events).is_empty());
}

#[test]
fn moves_that_cannot_be_done_are_ignored() {
    let (mut tabs, mut events) = two_tabs();
    let before = (tabs.arrangement(), held(&tabs).concat().len());
    let cases = [
        (0, "gone", WorkspaceTarget::Existing(1), "unknown key"),
        (1, "a1", WorkspaceTarget::Existing(0), "not in the tab"),
        (0, "a1", WorkspaceTarget::Existing(0), "to the tab it is in"),
        (0, "a1", WorkspaceTarget::Existing(2), "to no tab"),
        (1, "a3", WorkspaceTarget::New, "the tab's only project"),
        (2, "a3", WorkspaceTarget::Existing(0), "from no tab"),
    ];
    for (from, key, to, why) in cases {
        tabs.move_project(from, key, to);
        assert_eq!(
            (tabs.arrangement(), held(&tabs).concat().len()),
            before,
            "{why}"
        );
        assert_eq!(tabs.front_index(), 1, "{why}");
        assert!(sent(&mut events).is_empty(), "{why}");
    }
}

/// The index each tab's UI was last told it has among the tabs, with
/// the titles it was told.
fn places(tabs: &Tabs) -> Vec<(usize, Vec<String>)> {
    tabs.iter()
        .map(|tab| {
            let (labels, current) = tab.tui.workspaces();
            (current, labels.to_vec())
        })
        .collect()
}

/// Each tab's UI knows every tab's title and its own place.
fn assert_places(tabs: &Tabs) {
    let titles: Vec<String> = tabs.iter().map(Tab::title).collect();
    let want: Vec<_> = (0..tabs.len()).map(|i| (i, titles.clone())).collect();
    assert_eq!(places(tabs), want);
}

#[test]
fn each_tab_knows_its_place_as_tabs_open_close_and_move() {
    let (mut tabs, _events) = tabs();
    start(&mut tabs, TWO, "a1");
    assert_eq!(places(&tabs), [(0, vec!["A1".to_string()])]);

    let (mut tabs, _events) = three_tabs();
    assert_places(&tabs);
    assert_eq!(places(&tabs)[0].1, ["", "", "A3"]);

    tabs.move_tab(2, 0);
    assert_places(&tabs);
    assert_eq!(places(&tabs)[1].1, ["A3", "", ""]);

    tabs.move_project(0, "a3", WorkspaceTarget::New);
    assert_eq!(tabs.len(), 3, "a3 was alone: nothing moves");
    tabs.move_project(1, "a1", WorkspaceTarget::Existing(2));
    assert_eq!(tabs.len(), 2);
    assert_places(&tabs);

    tabs.move_project(1, "a1", WorkspaceTarget::New);
    assert_eq!(tabs.len(), 3);
    assert_places(&tabs);

    assert!(tabs.close(0));
    assert_places(&tabs);
    assert_eq!(places(&tabs).len(), 2);

    // A tab opened for a new project knows its place too.
    tabs.focus(0);
    plus(&mut tabs);
    let more: Agents = &[("a1", 0), ("a1.1", 1), ("p2", 0), ("a3", 0), ("p4", 0)];
    switch(&mut tabs, more, "p4");
    assert_eq!(tabs.len(), 3);
    assert_places(&tabs);
}

/// The UI of the tab at `index`, drawn 120 columns by 30 rows.
fn screen(tabs: &mut Tabs, index: usize) -> Vec<String> {
    testing::screen(&mut tabs.tabs[index].tui)
}

use testing::cell_of;

fn click(tabs: &mut Tabs, index: usize, button: MouseButton, cell: (u16, u16)) {
    testing::click(&mut tabs.tabs[index].tui, button, cell);
}

/// Right-click `name` in the projects list of the tab at `index`,
/// then pick `entry` from its "Move to workspace".
fn pick_move(tabs: &mut Tabs, index: usize, name: &str, entry: &str) {
    testing::pick_move(&mut tabs.tabs[index].tui, name, entry);
}

#[test]
fn the_menu_of_a_tab_moves_a_project_to_another() {
    let (mut tabs, mut events) = two_tabs();
    // `two_tabs` picked its folder without the browser `+` opened.
    let esc = KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE);
    tabs.tabs[0].tui.handle_key(esc);
    tabs.focus(0);
    switch(&mut tabs, TWO, "a1");
    tabs.forward_events();
    let _ = sent(&mut events);

    // The menu offers the other tab, by its place and title.
    let rows = screen(&mut tabs, 0);
    click(&mut tabs, 0, MouseButton::Right, cell_of(&rows, "P2"));
    let rows = screen(&mut tabs, 0);
    click(
        &mut tabs,
        0,
        MouseButton::Left,
        cell_of(&rows, "Move to workspace"),
    );
    let rows = screen(&mut tabs, 0);
    assert!(
        rows.iter().any(|row| row.contains("Workspace 2  A3")),
        "{}",
        rows.join("\n")
    );
    click(
        &mut tabs,
        0,
        MouseButton::Left,
        cell_of(&rows, "Workspace 2"),
    );
    assert_eq!(tabs.arrangement(), [vec!["a1", "p2"], vec!["a3"]]);

    tabs.forward_events();
    assert_eq!(tabs.arrangement(), [vec!["a1"], vec!["a3", "p2"]]);
    assert_eq!(held(&tabs), [vec!["a1", "a1.1"], vec!["p2", "a3"]]);
    assert_eq!(tabs.front_index(), 0);
    assert!(sent(&mut events).is_empty(), "the session hears nothing");

    // An agent moves its whole project, and its emptied tab closes.
    pick_move(&mut tabs, 0, "A1.1", "Workspace 2");
    tabs.forward_events();
    assert_eq!(tabs.arrangement(), [["a3", "p2", "a1"]]);
    assert_eq!(tabs.front_index(), 0);
}

#[test]
fn moves_asked_in_one_update_go_to_the_tabs_the_menus_named() {
    let (mut tabs, _events) = three_tabs();
    // a1's tab asks to go to a3's, then a3's to p2's: the first move
    // closes a1's tab and shifts the others.
    pick_move(&mut tabs, 0, "A1", "Workspace 3");
    pick_move(&mut tabs, 2, "A3", "Workspace 2");
    tabs.forward_events();
    assert_eq!(tabs.arrangement(), [vec!["p2", "a3"], vec!["a1"]]);
    assert_places(&tabs);
}

/// Each update from now on asks the session nothing more.
fn settled(tabs: &mut Tabs, events: &mut mpsc::Receiver<UiEvent>) {
    for update in 0..3 {
        tabs.forward_events();
        assert!(sent(events).is_empty(), "update {update} asked again");
    }
}

#[test]
fn a_tab_left_with_nothing_to_show_hands_the_front_to_its_moved_agent() {
    // p2 has every agent archived: once a1 goes, its tab shows none.
    let agents: Agents = &[("a1", 0), ("a1.1", 1), ("a3", 0)];
    let (mut tabs, mut events) = tabs_with(&[&["a1", "p2"], &["a3"]]);
    tabs.apply(projects(&["a1", "p2", "a3"]));
    start(&mut tabs, agents, "a1");
    tabs.focus(1);
    switch(&mut tabs, agents, "a3");
    tabs.focus(0);
    switch(&mut tabs, agents, "a1");
    let _ = sent(&mut events);
    assert_eq!(shown(&tabs), ["a1", "a3"]);
    assert_eq!(tabs.front_index(), 0);

    tabs.move_project(0, "a1", WorkspaceTarget::Existing(1));
    assert_eq!(tabs.arrangement(), [vec!["p2"], vec!["a3", "a1"]]);
    assert_eq!(tabs.front_index(), 1);
    // The session has a1 already and would send its chat only on a
    // switch, so it turns to a3, which a3's tab shows, on the way.
    assert_eq!(
        sent(&mut events),
        [r#"FocusAgent("a3")"#, r#"SelectAgent("a1")"#]
    );
    settled(&mut tabs, &mut events);

    switch(&mut tabs, agents, "a3");
    assert_eq!(shown(&tabs), ["a1", "a3"]);
    assert_eq!(tabs.front_index(), 1);
    settled(&mut tabs, &mut events);

    // The switch back to a1, with its chat.
    tabs.apply(UiCommand::SetActiveAgent("a1".into()));
    tabs.apply(message("the chat of a1"));
    tabs.apply(list(agents, "a1"));
    tabs.apply(UiCommand::SetActiveAgent("a1".into()));
    assert_eq!(shown(&tabs)[1], "a1");
    assert_eq!(tabs.front_index(), 1);
    let chat = tabs.tabs[1].tui.messages();
    assert!(chat.iter().any(|m| m.content() == "the chat of a1"));
    assert_eq!(held(&tabs), [Vec::<&str>::new(), vec!["a1", "a1.1", "a3"]]);
    settled(&mut tabs, &mut events);

    // The emptied tab, brought forward, asks for nothing it can't show.
    tabs.focus(0);
    settled(&mut tabs, &mut events);
}

#[test]
fn the_session_alone_with_the_moved_agent_shows_it_without_a_switch() {
    let agents: Agents = &[("a1", 0)];
    let (mut tabs, mut events) = tabs_with(&[&["a1", "p2"], &["p3"]]);
    tabs.apply(projects(&["a1", "p2", "p3"]));
    start(&mut tabs, agents, "a1");
    let _ = sent(&mut events);

    tabs.move_project(0, "a1", WorkspaceTarget::Existing(1));
    assert_eq!(tabs.arrangement(), [vec!["p2"], vec!["p3", "a1"]]);
    assert_eq!(tabs.front_index(), 1);
    assert_eq!(shown(&tabs)[1], "a1");
    assert!(sent(&mut events).is_empty(), "no agent to turn to");
    settled(&mut tabs, &mut events);
}

#[test]
fn until_the_session_turns_it_speaks_to_the_tab_showing_its_agent() {
    let (mut tabs, mut events) = two_tabs();
    tabs.focus(0);
    switch(&mut tabs, TWO, "a1");
    let _ = sent(&mut events);

    tabs.move_project(0, "a1", WorkspaceTarget::Existing(1));
    assert_eq!(sent(&mut events), [r#"SelectAgent("p2")"#]);
    // The session still has a1: a1's old tab shows it, the other
    // holds it now.
    assert_eq!(shown(&tabs), ["a1", "a3"]);
    let error = |tab: &Tab, text: &str| matches!(tab.tui.notification(), Some((shown, _)) if shown.contains(text));
    tabs.apply(UiCommand::ShowError("a1 failed".into()));
    assert!(error(&tabs.tabs[0], "a1 failed"));
    assert!(!error(&tabs.tabs[1], "a1 failed"));

    // Turned to p2, it speaks to p2's tab.
    switch(&mut tabs, TWO, "p2");
    assert_eq!(shown(&tabs), ["p2", "a3"]);
    tabs.apply(UiCommand::ShowError("p2 failed".into()));
    assert!(error(&tabs.tabs[0], "p2 failed"));
    assert!(!error(&tabs.tabs[1], "p2 failed"));
    settled(&mut tabs, &mut events);
}

#[test]
fn closing_a_tab_does_the_moves_asked_before_it() {
    // a1's tab asks to go to a3's, which closes it and shifts p2's.
    let (mut tabs, _events) = three_tabs();
    pick_move(&mut tabs, 0, "A1", "Workspace 3");
    assert!(tabs.close(1), "p2's tab");
    assert_eq!(tabs.arrangement(), [["a3", "a1", "p2"]]);
    assert_eq!(tabs.front_index(), 0);

    // The tab closed is the one the move emptied.
    let (mut tabs, _events) = three_tabs();
    pick_move(&mut tabs, 0, "A1", "Workspace 3");
    assert!(tabs.close(0), "closed, emptied");
    assert_eq!(tabs.arrangement(), [vec!["p2"], vec!["a3", "a1"]]);
    assert_eq!(tabs.front_index(), 1);

    // The move leaves one tab, which stays.
    let (mut tabs, _events) = tabs_with(&[&["a1"], &["a3"]]);
    start(&mut tabs, &[("a1", 0), ("a3", 0)], "a3");
    pick_move(&mut tabs, 0, "A1", "Workspace 2");
    assert!(!tabs.close(1), "the last tab stays");
    assert_eq!(tabs.arrangement(), [["a3", "a1"]]);
    assert_eq!(tabs.front_index(), 0);
}

#[test]
fn a_tabs_own_switch_goes_to_the_session_alone() {
    let (mut tabs, mut events) = tabs();
    start(&mut tabs, ONE, "a1");
    let _ = sent(&mut events);

    // The tab switched on screen already; nothing is asked for the
    // agent it showed before, which would turn the session there.
    tabs.forward(0, UiEvent::FocusAgent("a1.1".into()));
    assert_eq!(sent(&mut events), [r#"FocusAgent("a1.1")"#]);
    switch(&mut tabs, ONE, "a1.1");

    tabs.forward(0, UiEvent::SelectAgent("a1".into()));
    assert_eq!(sent(&mut events), [r#"SelectAgent("a1")"#]);
    switch(&mut tabs, ONE, "a1");

    // A select of the agent the session has is still sent: its chat
    // comes with the answer.
    tabs.forward(0, UiEvent::SelectAgent("a1".into()));
    assert_eq!(sent(&mut events), [r#"SelectAgent("a1")"#]);
    switch(&mut tabs, ONE, "a1");

    // A focus on it gets its answer here, as the session sends none.
    tabs.forward(0, UiEvent::FocusAgent("a1".into()));
    assert!(sent(&mut events).is_empty());
    assert_eq!(shown(&tabs), ["a1"]);
}

#[test]
fn a_failed_switch_is_no_longer_waited_for() {
    let (mut tabs, mut events) = tabs();
    start(&mut tabs, ONE, "a1");
    tabs.forward(0, UiEvent::SelectAgent("a1.1".into()));
    assert_eq!(sent(&mut events), [r#"SelectAgent("a1.1")"#]);
    tabs.apply(UiCommand::ShowError("Could not open agent".into()));
    tabs.apply(UiCommand::SwitchFailed("a1.1".into()));

    // The session still has a1, so asking for a1.1 again is sent.
    tabs.forward(0, UiEvent::FocusAgent("a1.1".into()));
    assert_eq!(sent(&mut events), [r#"FocusAgent("a1.1")"#]);
}
