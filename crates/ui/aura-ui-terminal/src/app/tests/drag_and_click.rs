use super::*;

#[test]
fn test_dragging_a_project_reorders_the_list_live() {
    let mut app = app_with_three_projects();
    left(&mut app, MouseEventKind::Down, 4);

    // Onto bb: aa, with its child, takes bb's place at once.
    left(&mut app, MouseEventKind::Drag, 8);
    assert_eq!(agent_ids(&app), ["bb", "aa", "aa1", "cc"]);
    assert!(app.project_dragging());
    assert_eq!(app.project_order(), ["bb", "aa", "cc"]);

    // Onto cc, and back up to the top, still before the button is up.
    left(&mut app, MouseEventKind::Drag, 12);
    assert_eq!(agent_ids(&app), ["bb", "cc", "aa", "aa1"]);
    left(&mut app, MouseEventKind::Drag, 4);
    assert_eq!(agent_ids(&app), ["aa", "aa1", "bb", "cc"]);

    left(&mut app, MouseEventKind::Drag, 11);
    assert_eq!(agent_ids(&app), ["bb", "cc", "aa", "aa1"]);
    left(&mut app, MouseEventKind::Up, 11);
    assert!(!app.project_dragging());
    assert_eq!(agent_ids(&app), ["bb", "cc", "aa", "aa1"]);
    // Dropping a project does not fold it.
    assert!(matches!(
        app.project_rows()[app.project_rows().len() - 4],
        ProjectListRow::Project { expanded: true, .. }
    ));
}

#[test]
fn test_dropping_a_dragged_project_sends_the_new_order() {
    let (tx, mut rx) = mpsc::channel(16);
    let mut app = app_with_three_projects();
    app.link.event_tx = Some(tx);
    left(&mut app, MouseEventKind::Down, 4);
    left(&mut app, MouseEventKind::Drag, 8);
    assert!(rx.try_recv().is_err(), "nothing is sent mid-drag");
    left(&mut app, MouseEventKind::Up, 8);
    assert!(matches!(
        rx.try_recv(),
        Ok(UiEvent::ReorderProjects(order)) if order == ["bb", "aa", "cc"]
    ));

    // A click that folds sends no order.
    left(&mut app, MouseEventKind::Down, 4);
    left(&mut app, MouseEventKind::Up, 4);
    assert!(rx.try_recv().is_err());
}

/// Project `site` with `a1` (which spawned `a1c`), `a2` and `a3`, then
/// project `notes` with `n1`. Rows from line 4: site 4, a1 5, a1c 6, a2 7,
/// a3 8; gap 9; notes 10, n1 11.
fn app_with_agents_to_drag() -> (App, mpsc::Receiver<UiEvent>) {
    let (tx, rx) = mpsc::channel(16);
    let mut app = App::new();
    app.link.event_tx = Some(tx);
    let at = |project: &str, id: &str, depth: usize| crate::events::AgentSummary {
        project_id: Some(project.to_string()),
        depth,
        ..agent(id, &id.to_uppercase())
    };
    app.process_command(UiCommand::SetAgents(vec![
        at("site", "a1", 0),
        at("site", "a1c", 1),
        at("site", "a2", 0),
        at("site", "a3", 0),
        at("notes", "n1", 0),
    ]));
    (with_agent_areas(app), rx)
}

#[test]
fn test_dragging_an_agent_reorders_its_project_and_sends_the_order() {
    let (mut app, mut rx) = app_with_agents_to_drag();
    left(&mut app, MouseEventKind::Down, 5);
    assert!(matches!(rx.try_recv(), Ok(UiEvent::SelectAgent(id)) if id == "a1"));

    // Onto a3: a1 and the agent it spawned move below it.
    left(&mut app, MouseEventKind::Drag, 8);
    assert_eq!(agent_ids(&app), ["a2", "a3", "a1", "a1c", "n1"]);
    assert!(app.project_dragging());
    // It never leaves its project.
    left(&mut app, MouseEventKind::Drag, 11);
    assert_eq!(agent_ids(&app), ["a2", "a3", "a1", "a1c", "n1"]);

    left(&mut app, MouseEventKind::Up, 11);
    assert!(matches!(
        rx.try_recv(),
        Ok(UiEvent::ReorderAgents { project_id, order })
            if project_id == "site" && order == ["a2", "a3", "a1"]
    ));
}

#[test]
fn test_a_spawned_agent_is_not_dragged_on_its_own() {
    let (mut app, mut rx) = app_with_agents_to_drag();
    left(&mut app, MouseEventKind::Down, 6);
    left(&mut app, MouseEventKind::Drag, 8);
    assert_eq!(agent_ids(&app), ["a1", "a1c", "a2", "a3", "n1"]);
    left(&mut app, MouseEventKind::Up, 8);
    let sent: Vec<UiEvent> = std::iter::from_fn(|| rx.try_recv().ok()).collect();
    assert!(
        !sent
            .iter()
            .any(|event| matches!(event, UiEvent::ReorderAgents { .. })),
        "{sent:?}"
    );
}

#[test]
fn test_a_project_drag_reports_the_rows_under_the_pointer() {
    use crate::app::project_drag::ProjectDragHit;

    let mut app = app_with_three_projects();
    left(&mut app, MouseEventKind::Down, 8);
    assert_eq!(app.project_drag_hit(), None, "a press is not a drag");

    // Onto aa's child: bb would not land under the pointer, so it stays,
    // and aa is the hit area. List rows: aa 0..3, gap, bb 4..6.
    let redraw = app.handle_mouse(mouse(MouseEventKind::Drag(MouseButton::Left), 5, 6));
    assert!(redraw, "a new hit area is drawn even without a move");
    assert_eq!(agent_ids(&app), ["aa", "aa1", "bb", "cc"]);
    assert_eq!(
        app.project_drag_hit(),
        Some(ProjectDragHit {
            dragged: 4..6,
            target: Some(0..3),
            edge: Some(0),
        })
    );

    // Onto aa's header: bb takes its place and is its own hit area, and
    // goes between itself and aa, which it passed.
    left(&mut app, MouseEventKind::Drag, 4);
    assert_eq!(agent_ids(&app), ["bb", "aa", "aa1", "cc"]);
    assert_eq!(
        app.project_drag_hit(),
        Some(ProjectDragHit {
            dragged: 0..2,
            target: Some(0..2),
            edge: Some(2),
        })
    );

    left(&mut app, MouseEventKind::Up, 4);
    assert_eq!(app.project_drag_hit(), None);
}

#[test]
fn test_an_agent_drag_has_no_hit_area_outside_its_project() {
    let (mut app, _rx) = app_with_agents_to_drag();
    left(&mut app, MouseEventKind::Down, 5);
    left(&mut app, MouseEventKind::Drag, 11);
    let hit = app.project_drag_hit().expect("dragging");
    // a1 and the agent it spawned, still in place.
    assert_eq!(hit.dragged, 1..3);
    assert_eq!(hit.target, None);

    left(&mut app, MouseEventKind::Drag, 8);
    let hit = app.project_drag_hit().expect("dragging");
    assert_eq!(hit.target, Some(hit.dragged));
}

#[test]
fn test_a_redraw_is_asked_for_each_move_of_a_dragged_project() {
    let mut app = app_with_three_projects();
    left(&mut app, MouseEventKind::Down, 4);
    let moved = app.handle_mouse(mouse(MouseEventKind::Drag(MouseButton::Left), 5, 8));
    assert!(moved);
    assert_eq!(agent_ids(&app), ["bb", "aa", "aa1", "cc"]);
}

#[test]
fn test_a_click_without_movement_folds_and_does_not_reorder() {
    let mut app = app_with_three_projects();
    left(&mut app, MouseEventKind::Down, 4);
    assert!(matches!(
        app.project_rows()[0],
        ProjectListRow::Project { expanded: true, .. }
    ));
    left(&mut app, MouseEventKind::Up, 4);
    assert!(matches!(
        app.project_rows()[0],
        ProjectListRow::Project {
            expanded: false,
            ..
        }
    ));
    assert_eq!(agent_ids(&app), ["aa", "aa1", "bb", "cc"]);
    assert!(!app.project_dragging());
}

#[test]
fn test_a_dragged_project_does_not_swap_back_under_a_still_pointer() {
    let mut app = app_with_three_projects();
    // cc is one row tall once folded; dragging it up over the taller aa
    // must not flip between the two on the way.
    left(&mut app, MouseEventKind::Down, 11);
    left(&mut app, MouseEventKind::Drag, 8);
    let once = agent_ids(&app).join(",");
    left(&mut app, MouseEventKind::Drag, 9);
    left(&mut app, MouseEventKind::Drag, 8);
    assert_eq!(agent_ids(&app).join(","), once);
    left(&mut app, MouseEventKind::Up, 8);
    let mut order = app.project_order();
    order.sort();
    assert_eq!(order, ["aa", "bb", "cc"]);
}

#[test]
fn test_dragging_keeps_the_highlight_on_its_agent() {
    let mut app = app_with_three_projects();
    app.process_command(UiCommand::SetActiveAgent("bb".to_string()));
    let highlighted = |app: &App| match app.project_rows()[app.selected_agent()] {
        ProjectListRow::Agent { index } => app.agents()[index].id.clone(),
        other => panic!("{other:?}"),
    };
    assert_eq!(highlighted(&app), "bb");
    left(&mut app, MouseEventKind::Down, 4);
    left(&mut app, MouseEventKind::Drag, 8);
    assert_eq!(agent_ids(&app), ["bb", "aa", "aa1", "cc"]);
    assert_eq!(highlighted(&app), "bb");
}

#[test]
fn test_click_sandbox_row_selects_and_toggles_dir() {
    use crossterm::event::{MouseButton, MouseEventKind};
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir(dir.path().join("src")).unwrap();
    std::fs::write(dir.path().join("src/lib.rs"), "").unwrap();
    std::fs::write(dir.path().join("notes.md"), "").unwrap();

    let mut app = App::new();
    app.process_command(UiCommand::SetSandboxRoot(dir.path().to_path_buf()));
    app.set_panel_areas(crate::app::PanelAreas {
        sandbox: Some(Rect::new(80, 10, 30, 10)),
        ..crate::app::PanelAreas::default()
    });
    app.set_sandbox_rows_area(Rect::new(81, 11, 28, 8), 0);

    app.handle_mouse(mouse(MouseEventKind::Down(MouseButton::Left), 85, 11));
    assert_eq!(app.focus(), PanelFocus::Sandbox);
    let names: Vec<&str> = app
        .sandbox()
        .rows()
        .iter()
        .map(|r| r.name.as_str())
        .collect();
    assert_eq!(names, ["src", "lib.rs", "notes.md"]);

    app.handle_mouse(mouse(MouseEventKind::Down(MouseButton::Left), 85, 13));
    assert_eq!(app.sandbox().selected(), 2);
    assert_eq!(app.sandbox().rows().len(), 3);

    // The wheel scrolls the tree and leaves its highlight alone.
    app.handle_mouse(mouse(MouseEventKind::ScrollUp, 85, 15));
    assert_eq!(app.sandbox().selected(), 2);
}

#[test]
fn test_click_agent_row_switches_and_new_row_creates() {
    use crossterm::event::{MouseButton, MouseEventKind};
    let (mut app, mut rx) = app_with_agent_areas();

    // Projects take lines 4 and 7, agents 5 and 8, the gap 6.
    app.handle_mouse(mouse(MouseEventKind::Down(MouseButton::Left), 5, 8));
    assert_eq!(app.focus(), PanelFocus::Swarm);
    assert!(matches!(rx.try_recv(), Ok(UiEvent::SelectAgent(id)) if id == "bb"));

    app.handle_mouse(mouse(MouseEventKind::Down(MouseButton::Left), 5, 6));
    assert_eq!(app.selected_agent(), 4);

    // Clicking a project folds it and leaves the highlighted agent alone.
    let bb = ProjectListRow::Agent { index: 1 };
    app.handle_mouse(mouse(MouseEventKind::Down(MouseButton::Left), 5, 4));
    app.handle_mouse(mouse(MouseEventKind::Up(MouseButton::Left), 5, 4));
    assert!(matches!(
        app.project_rows()[0],
        ProjectListRow::Project {
            expanded: false,
            ..
        }
    ));
    assert_eq!(app.project_rows()[app.selected_agent()], bb);
    assert_eq!(app.hover(), Some(HoverTarget::ProjectRow(0)));
    app.handle_mouse(mouse(MouseEventKind::Down(MouseButton::Left), 5, 4));
    app.handle_mouse(mouse(MouseEventKind::Up(MouseButton::Left), 5, 4));
    assert_eq!(app.selected_agent(), 4);
    assert_eq!(app.project_rows()[4], bb);
    assert!(rx.try_recv().is_err());

    // "+ New project" is pinned to the bottom, not drawn after the last agent.
    app.handle_mouse(mouse(MouseEventKind::Down(MouseButton::Left), 5, 13));
    assert_eq!(app.selected_agent(), 4);
    assert_ne!(app.state(), AppState::SelectingFolder);
    assert!(rx.try_recv().is_err());

    app.handle_mouse(mouse(MouseEventKind::Moved, 5, 14));
    assert_eq!(app.hover(), Some(HoverTarget::ProjectRow(5)));
    app.handle_mouse(mouse(MouseEventKind::Down(MouseButton::Left), 5, 14));
    assert_eq!(app.state(), AppState::SelectingFolder);
    assert!(rx.try_recv().is_err());
}

#[test]
fn test_drag_selects_chat_text_and_requests_copy() {
    use crossterm::event::{MouseButton, MouseEventKind};
    use ratatui::{buffer::Buffer, style::Style};
    let (mut app, _rx) = app_with_agent_areas();
    app.set_chat_text_area(Rect::new(23, 4, 54, 8));
    let mut buffer = Buffer::empty(Rect::new(0, 0, 80, 14));
    buffer.set_string(23, 4, "[12:00:00] <AURA> hello", Style::default());
    buffer.set_string(23, 5, "             second line", Style::default());

    app.handle_mouse(mouse(MouseEventKind::Down(MouseButton::Left), 34, 4));
    app.handle_mouse(mouse(MouseEventKind::Drag(MouseButton::Left), 200, 5));
    assert!(app.take_copy_text(&buffer).is_none());
    app.handle_mouse(mouse(MouseEventKind::Up(MouseButton::Left), 200, 5));

    assert_eq!(
        app.take_copy_text(&buffer).as_deref(),
        Some("<AURA> hello\n             second line")
    );
    assert!(app.take_copy_text(&buffer).is_none());
    assert!(app.request_copy());

    app.handle_key(KeyEvent::new(KeyCode::Char('x'), KeyModifiers::NONE));
    assert!(app.selection().is_none());
    assert!(!app.request_copy());
}

#[test]
fn test_copy_notice_sits_then_expires() {
    let mut app = App::new();
    app.notify_copied(4, &Ok(()));
    assert_eq!(
        app.notification(),
        Some(&("Copied 4 characters".to_string(), NotificationType::Success))
    );

    app.tick();
    assert!(app.notification().is_some());

    let (_, until) = app.notifications.shown.as_mut().unwrap();
    *until = std::time::Instant::now() - std::time::Duration::from_millis(1);
    app.tick();
    assert!(app.notification().is_none());
}

#[test]
fn test_keypress_clears_copy_notice() {
    let mut app = App::new();
    app.notify_copied(1, &Ok(()));
    app.handle_key(KeyEvent::new(KeyCode::Char('x'), KeyModifiers::empty()));
    assert!(app.notification().is_none());
}

#[test]
fn test_notification_expires_and_a_new_one_starts_its_own_timer() {
    let mut app = App::new();
    app.notifications.current = Some(("first".to_string(), NotificationType::Warning));
    app.tick();
    let (_, until) = app.notifications.shown.as_mut().unwrap();
    *until = std::time::Instant::now() - std::time::Duration::from_millis(1);

    app.notifications.current = Some(("second".to_string(), NotificationType::Error));
    app.tick();
    assert_eq!(app.notification().unwrap().0, "second");
    let (_, until) = app.notifications.shown.as_ref().unwrap();
    assert!(*until > std::time::Instant::now() + std::time::Duration::from_secs(5));

    let (_, until) = app.notifications.shown.as_mut().unwrap();
    *until = std::time::Instant::now() - std::time::Duration::from_millis(1);
    app.tick();
    assert!(app.notification().is_none());
}

#[test]
fn test_click_without_drag_selects_nothing() {
    use crossterm::event::{MouseButton, MouseEventKind};
    let (mut app, _rx) = app_with_agent_areas();

    app.handle_mouse(mouse(MouseEventKind::Down(MouseButton::Left), 30, 5));
    app.handle_mouse(mouse(MouseEventKind::Up(MouseButton::Left), 30, 5));

    assert!(app.selection().is_none());
    assert!(!app.request_copy());
}

#[test]
fn test_wheel_over_agents_scrolls_without_switching() {
    use crossterm::event::MouseEventKind;
    let (mut app, mut rx) = app_with_agent_areas();
    // Two lines of a five-line list: project, agent, gap, project, agent.
    app.set_agent_rows_area(Rect::new(1, 4, 18, 2));
    let selected = app.selected_agent();

    for _ in 0..5 {
        app.handle_mouse(mouse(MouseEventKind::ScrollDown, 5, 5));
    }
    assert_eq!(app.agents_scroll(), 3);
    app.sync_agents_scroll(2);
    assert_eq!(app.agents_scroll(), 3);
    assert_eq!(app.selected_agent(), selected);
    assert!(rx.try_recv().is_err());
    assert_eq!(app.focus(), PanelFocus::Chat);
    assert_eq!(app.scroll_offset(), 0);

    // Three rows a notch, as in the chat.
    app.handle_mouse(mouse(MouseEventKind::ScrollUp, 5, 5));
    assert_eq!(app.agents_scroll(), 0);
    app.handle_mouse(mouse(MouseEventKind::ScrollDown, 5, 5));
    assert_eq!(app.agents_scroll(), 3);

    // A key brings the highlight back into view. Up from the first agent
    // has no agent to reach, so the highlight stays where it is.
    app.focus = PanelFocus::Swarm;
    app.handle_key(KeyEvent::new(KeyCode::Up, KeyModifiers::empty()));
    assert_eq!(app.selected_agent(), selected);
    app.sync_agents_scroll(2);
    assert_eq!(app.agents_scroll(), 1);
}
