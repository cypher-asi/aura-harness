use super::*;
use crate::app::HoverTarget;
use crate::events::UiCommand;
use crossterm::event::{KeyModifiers, MouseButton, MouseEvent, MouseEventKind};

fn app() -> (App, tokio::sync::mpsc::Receiver<UiEvent>) {
    let (tx, rx) = tokio::sync::mpsc::channel(16);
    let mut app = App::new();
    app.link.event_tx = Some(tx);
    app.active_agent_id = "aa".into();
    (app, rx)
}

fn set_placement(app: &mut App, state: PlacementState) {
    app.process_command(UiCommand::SetPlacement {
        agent_id: "aa".into(),
        state,
    });
}

fn mouse(kind: MouseEventKind, column: u16, row: u16) -> MouseEvent {
    MouseEvent {
        kind,
        column,
        row,
        modifiers: KeyModifiers::NONE,
    }
}

#[test]
fn labels_show_where_and_how() {
    assert_eq!(switch_label(&PlacementState::default()), "⌂ local");
    let cloud = PlacementState {
        placement: PlacementView::Cloud,
        machine_state: Some("asleep".into()),
        ..PlacementState::default()
    };
    assert_eq!(switch_label(&cloud), "☁  cloud · asleep");
    let moving = PlacementState {
        moving: Some("moving to cloud".into()),
        ..PlacementState::default()
    };
    assert_eq!(switch_label(&moving), "◐ moving to cloud");
    let foreign = PlacementState {
        placement: PlacementView::Cloud,
        owner: Some("Alice".into()),
        ..PlacementState::default()
    };
    assert_eq!(switch_label(&foreign), "☁  Alice");
}

#[test]
fn another_users_cloud_agent_is_marked_and_not_moved() {
    let (mut app, mut rx) = app();
    set_placement(
        &mut app,
        PlacementState {
            placement: PlacementView::Cloud,
            owner: Some("Alice".into()),
            ..PlacementState::default()
        },
    );
    app.process_command(UiCommand::SetPlacement {
        agent_id: "bb".into(),
        state: PlacementState {
            placement: PlacementView::Cloud,
            ..PlacementState::default()
        },
    });
    assert_eq!(app.cloud_badge("aa"), Some((" ☁  Alice".into(), true)));
    assert_eq!(app.cloud_badge("bb"), Some((" ☁ ".into(), false)));
    assert_eq!(app.cloud_badge("cc"), None);
    app.handle_command("/local");
    assert!(rx.try_recv().is_err(), "only Alice can bring it back");
    assert!(app
        .notifications
        .current
        .as_ref()
        .is_some_and(|(text, _)| text.contains("Placed by Alice")));
}

#[test]
fn cloud_and_local_commands_ask_for_a_move() {
    let (mut app, mut rx) = app();
    app.handle_command("/cloud");
    assert!(matches!(
        rx.try_recv(),
        Ok(UiEvent::SetPlacement { agent_id, placement: PlacementView::Cloud }) if agent_id == "aa"
    ));
    app.handle_command("/local");
    assert!(
        rx.try_recv().is_err(),
        "the move just asked for is under way"
    );
    set_placement(&mut app, PlacementState::default());
    app.handle_command("/local");
    assert!(rx.try_recv().is_err(), "already local: nothing to do");
    app.handle_command("/cloud setup");
    assert!(matches!(rx.try_recv(), Ok(UiEvent::CloudSetup)));
}

#[test]
fn a_move_under_way_is_not_asked_for_again() {
    let (mut app, mut rx) = app();
    set_placement(
        &mut app,
        PlacementState {
            moving: Some("moving to cloud".into()),
            ..PlacementState::default()
        },
    );
    app.handle_command("/cloud");
    assert!(rx.try_recv().is_err());
}

#[test]
fn clicking_the_switch_shows_the_move_before_the_app_answers() {
    let (mut app, mut rx) = app();
    let row = Rect::new(0, 30, 100, 1);
    app.set_footer_area(row);
    let rect = switch_rect(row, &switch_label(&app.placement())).expect("fits");
    let click = mouse(MouseEventKind::Down(MouseButton::Left), rect.x + 1, 30);
    assert!(app.click_placement_switch(click));
    assert!(matches!(
        rx.try_recv(),
        Ok(UiEvent::SetPlacement {
            placement: PlacementView::Cloud,
            ..
        })
    ));
    assert_eq!(switch_label(&app.placement()), "◐ moving to cloud");
    assert!(app.animating(), "the switch turns while it moves");

    // The app could not move it: the switch goes back.
    set_placement(&mut app, PlacementState::default());
    assert_eq!(switch_label(&app.placement()), "⌂ local");
    assert_eq!(app.switch_sweep(), None, "nothing landed");
}

#[test]
fn the_switch_takes_clicks_and_the_pointer_a_column_either_side() {
    let (mut app, mut rx) = app();
    let row = Rect::new(0, 30, 100, 1);
    app.set_footer_area(row);
    let rect = switch_rect(row, &switch_label(&app.placement())).expect("fits");
    let moved = |column| mouse(MouseEventKind::Moved, column, 30);
    for column in [rect.x - 1, rect.x, rect.right()] {
        app.handle_mouse(moved(column));
        assert_eq!(app.hover(), Some(HoverTarget::PlacementSwitch), "{column}");
        assert!(app.placement_switch_lit());
    }
    app.handle_mouse(moved(rect.x - 2));
    assert_ne!(app.hover(), Some(HoverTarget::PlacementSwitch));
    assert!(!app.placement_switch_lit());
    let press = |column| mouse(MouseEventKind::Down(MouseButton::Left), column, 30);
    assert!(!app.click_placement_switch(press(rect.x - 2)));
    assert!(app.click_placement_switch(press(rect.right())));
    assert!(rx.try_recv().is_ok());
}

#[test]
fn the_switch_is_not_lit_while_it_moves_or_for_another_users_agent() {
    let (mut app, _rx) = app();
    let row = Rect::new(0, 30, 100, 1);
    let hover = |app: &mut App| {
        app.set_footer_area(row);
        let label = switch_label(&app.placement());
        let rect = switch_rect(row, &label).expect("fits");
        app.handle_mouse(mouse(MouseEventKind::Moved, rect.x, 30));
        assert_eq!(app.hover(), Some(HoverTarget::PlacementSwitch));
        app.placement_switch_lit()
    };
    assert!(hover(&mut app));
    let moving = PlacementState {
        moving: Some("moving to cloud".into()),
        ..PlacementState::default()
    };
    set_placement(&mut app, moving);
    assert!(!hover(&mut app));
    let foreign = PlacementState {
        placement: PlacementView::Cloud,
        owner: Some("Alice".into()),
        ..PlacementState::default()
    };
    set_placement(&mut app, foreign);
    assert!(!hover(&mut app));
}

#[test]
fn a_move_that_lands_sweeps_the_new_label_once() {
    let (mut app, _rx) = app();
    set_placement(
        &mut app,
        PlacementState {
            moving: Some("moving to cloud".into()),
            ..PlacementState::default()
        },
    );
    let cloud = PlacementState {
        placement: PlacementView::Cloud,
        ..PlacementState::default()
    };
    set_placement(&mut app, cloud.clone());
    assert_eq!(app.switch_sweep(), Some(0));
    let frames = crate::animation::shimmer_pass_frames("☁  cloud");
    for _ in 0..frames {
        assert!(app.animating(), "the sweep draws every frame");
        app.tick();
    }
    assert_eq!(app.switch_sweep(), None);
    assert!(!app.animating(), "then the switch rests");

    // An update that only repeats where the agent is sweeps nothing.
    set_placement(&mut app, cloud);
    assert_eq!(app.switch_sweep(), None);
}

#[test]
fn host_buttons_work_by_key() {
    let (mut app, mut rx) = app();
    app.process_command(UiCommand::SetHost(Box::new(HostView {
        agent_id: "aa".into(),
        placement: PlacementView::Cloud,
        actions: vec![HostAction::Pull, HostAction::Sleep],
        ..HostView::default()
    })));
    assert!(app.host_view().is_some());
    assert!(app.host_key(KeyEvent::new(KeyCode::Char('p'), KeyModifiers::NONE)));
    assert!(matches!(
        rx.try_recv(),
        Ok(UiEvent::HostAction {
            action: HostAction::Pull,
            ..
        })
    ));
    assert!(!app.host_key(KeyEvent::new(KeyCode::Char('z'), KeyModifiers::NONE)));
}

#[test]
fn another_agents_host_view_is_not_shown() {
    let (mut app, _rx) = app();
    app.process_command(UiCommand::SetHost(Box::new(HostView {
        agent_id: "bb".into(),
        ..HostView::default()
    })));
    assert!(app.host_view().is_none());
}
