//! Tabs as workspaces: opening, switching, dragging and closing them.

use super::fixtures::{agent, seed, session};
use super::support::snapshot;
use crate::router::testing::pick_move;
use crate::shell::keys::Press;
use crate::shell::tests::{stage, Stage};
use crate::tab_bar::Hit;
use aura_ui::events::UiCommand;
use winit::event::MouseButton;
use winit::keyboard::{ModifiersState, NamedKey};

#[test]
fn the_window_opens_with_one_tab() {
    let mut h = stage();
    seed(&mut h);
    assert_eq!(h.shell.tabs.len(), 1);
    assert_eq!(h.shell.bar.title(), Some("AURA-HARNESS / AGENT01"));
    assert!(h.shell.bar.spot(Hit::Tab(1)).is_none());
    assert!(h.asked().is_empty());
}

/// A second tab opened on a picked folder, with a draft typed in it.
fn open_second_tab_with_a_draft(h: &mut Stage) {
    h.click(Hit::New, MouseButton::Left);
    assert!(h.asked().is_empty(), "nothing until a folder is picked");
    h.pick_folder();
    assert_eq!(h.shell.tabs.len(), 1, "the tab opens with its project");
    h.joined("a3", "Agent03");
    assert_eq!(h.shell.tabs.len(), 2);
    assert_eq!(h.shell.bar.title(), Some("AGENT03"));
    assert!(!h.screen().contains("Here's what I found"));
    assert!(
        !h.screen().contains("Reviewer"),
        "a project of the first tab"
    );

    h.type_text("a draft for the new agent");
    assert!(h.screen().contains("a draft for the new agent"));
    snapshot("tabs", h);
}

#[test]
fn tabs_are_workspaces_with_their_own_projects() {
    let mut h = stage();
    session(&mut h);
    let _ = h.asked();
    open_second_tab_with_a_draft(&mut h);

    h.key(ModifiersState::CONTROL, Press::of(NamedKey::Tab));
    assert_eq!(h.asked(), [r#"FocusAgent("a1")"#]);
    assert_eq!(h.shell.tabs.front_index(), 0);
    let first = h.screen();
    assert!(first.contains("Here's what I found"));
    assert!(first.contains("Reviewer"));
    assert!(!first.contains("Agent03"), "a project of the second tab");
    assert!(!first.contains("a draft for the new agent"));

    h.click(Hit::Tab(1), MouseButton::Left);
    assert_eq!(h.asked(), [r#"FocusAgent("a3")"#]);
    assert!(h.screen().contains("a draft for the new agent"));

    h.key(ModifiersState::ALT, Press::typing("1"));
    assert_eq!(h.asked(), [r#"FocusAgent("a1")"#]);
    assert_eq!(h.shell.tabs.front_index(), 0);
    assert!(h.screen().contains("Here's what I found"));
}

#[test]
fn tabs_drag_into_a_new_order() {
    let mut h = stage();
    seed(&mut h);
    h.click(Hit::New, MouseButton::Left);
    h.pick_folder();
    h.joined("a3", "Agent03");
    let _ = h.asked();
    assert_eq!(h.shell.tabs.arrangement(), [vec!["a1"], vec!["a3"]]);

    h.drag_tab(Hit::Tab(1), Hit::Tab(0));
    assert_eq!(h.shell.tabs.arrangement(), [vec!["a3"], vec!["a1"]]);
    assert_eq!(h.shell.tabs.front_index(), 0);
    assert_eq!(h.shell.bar.title(), Some("AGENT03"));
    assert!(h.asked().is_empty());

    // Numbers pick tabs by their new places.
    h.key(ModifiersState::ALT, Press::typing("2"));
    assert_eq!(h.asked(), [r#"FocusAgent("a1")"#]);
}

#[test]
fn a_tab_dragged_while_a_move_closes_another_stays_the_one_dragged() {
    let mut h = stage();
    seed(&mut h);
    h.click(Hit::New, MouseButton::Left);
    h.pick_folder();
    h.joined("a3", "Agent03");
    h.click(Hit::New, MouseButton::Left);
    h.pick_folder();
    h.apply([
        UiCommand::SetActiveAgent("a4".into()),
        UiCommand::SetAgents(vec![
            agent("a1", "Agent01", 0, Some("aura-harness")),
            agent("a2", "Reviewer", 1, None),
            agent("a3", "Agent03", 0, None),
            agent("a4", "Agent04", 0, None),
        ]),
        UiCommand::SetActiveAgent("a4".into()),
    ]);
    let tabs = || [vec!["a1"], vec!["a3"], vec!["a4"]];
    assert_eq!(h.shell.tabs.arrangement(), tabs());

    // a1's tab asks to move its project to a4's: the next update does it
    // and closes a1's tab.
    h.shell.frame(h.now);
    let first = h.shell.tabs.id_at(0).expect("three tabs");
    let tab = h.shell.tabs.by_id_mut(first).expect("open");
    pick_move(&mut tab.tui, "Reviewer", "Workspace 3");
    assert_eq!(h.shell.tabs.arrangement(), tabs(), "not yet");

    // Pressing a3's tab is that update.
    let a3 = h.shell.bar.spot(Hit::Tab(1)).expect("drawn");
    h.shell.pointer_moved(a3);
    h.shell.button(MouseButton::Left, true, h.now);
    assert_eq!(h.shell.tabs.arrangement(), [vec!["a3"], vec!["a4", "a1"]]);

    // Dragged past a4's, a3's tab goes after it.
    h.shell.frame(h.now);
    let last = h.shell.bar.spot(Hit::Tab(1)).expect("drawn");
    h.shell.pointer_moved(last);
    h.shell.button(MouseButton::Left, false, h.now);
    assert_eq!(h.shell.tabs.arrangement(), [vec!["a4", "a1"], vec!["a3"]]);
    assert_eq!(h.shell.tabs.front().agent(), "a3");
}

#[test]
fn a_closed_tab_hands_its_projects_to_the_one_before_it() {
    let mut h = stage();
    seed(&mut h);
    let _ = h.asked();
    let held = ModifiersState::CONTROL | ModifiersState::SHIFT;
    h.key(held, Press::typing("T"));
    h.pick_folder();
    h.joined("a3", "Agent03");
    assert_eq!(h.shell.tabs.len(), 2);

    h.key(held, Press::typing("W"));
    assert_eq!(h.shell.tabs.len(), 1);
    assert_eq!(h.shell.tabs.front().agent(), "a1");
    assert_eq!(h.asked(), [r#"FocusAgent("a1")"#]);
    let all = h.screen();
    assert!(all.contains("Agent03"));
    assert!(all.contains("Reviewer"));
}
