use super::*;

#[test]
fn without_workspaces_the_menu_offers_no_move() {
    let (mut app, _rx) = app();
    let rows = screen(&mut app, 30);
    for name in ["CC", "BB"] {
        press(&mut app, MouseButton::Right, cell_of(&rows, name));
        let menu = app.context_menu().expect("the menu opened");
        assert!(!menu.items.contains(&MenuItem::MoveToWorkspace), "{name}");
    }
    assert!(app.take_workspace_move().is_none());
}

#[test]
fn projects_and_agents_offer_a_move_before_removing() {
    let (mut app, _rx) = workspace_app();
    let rows = screen(&mut app, 30);
    for name in ["CC", "BB"] {
        press(&mut app, MouseButton::Right, cell_of(&rows, name));
        let items = &app.context_menu().expect("the menu opened").items;
        let remove = items.iter().position(|&i| i == MenuItem::RemoveProject);
        let at = items.iter().position(|&i| i == MenuItem::MoveToWorkspace);
        assert_eq!(at.map(|at| at + 1), remove, "{name}: {items:?}");
    }
    let rows = screen(&mut app, 30);
    let item = rows
        .iter()
        .find(|row| row.contains("Move to workspace"))
        .expect("drawn");
    assert!(item.contains('›'), "{item}");

    // A project with every agent archived, too.
    app.process_command(UiCommand::SetProjects(vec![
        crate::events::ProjectSummary {
            id: "ee".into(),
            name: "empty".into(),
            ..Default::default()
        },
    ]));
    let rows = screen(&mut app, 30);
    press(&mut app, MouseButton::Right, cell_of(&rows, "EMPTY"));
    let menu = app.context_menu().expect("the menu opened");
    assert_eq!(menu.target, MenuTarget::Project("ee".into()));
    assert!(menu.items.contains(&MenuItem::MoveToWorkspace));
}

#[test]
fn the_archive_offers_no_move() {
    let (mut app, _rx) = archive_app();
    app.set_workspaces(vec!["A".into(), "B".into()], 0);
    let rows = screen(&mut app, 30);
    press(&mut app, MouseButton::Right, cell_of(&rows, "EE"));
    let menu = app.context_menu().expect("the menu opened");
    assert_eq!(menu.items, vec![MenuItem::Restore]);
}

#[test]
fn the_keys_move_an_agents_project_to_another_workspace() {
    let (mut app, mut rx) = workspace_app();
    let rows = screen(&mut app, 30);
    press(&mut app, MouseButton::Right, cell_of(&rows, "BB"));
    // Open, Archive, then Move to workspace.
    app.handle_key(key(KeyCode::Down));
    app.handle_key(key(KeyCode::Down));
    assert!(submenu(&app).is_none());
    app.handle_key(key(KeyCode::Right));
    let entries = &submenu(&app).expect("Right opens it").entries;
    assert_eq!(
        entries,
        &[
            SubmenuEntry::Workspace(WorkspaceTarget::Existing(0)),
            SubmenuEntry::Workspace(WorkspaceTarget::Existing(2)),
            SubmenuEntry::Workspace(WorkspaceTarget::New)
        ],
        "every workspace but this one, then a new one"
    );

    let rows = screen(&mut app, 30);
    for label in ["Workspace 1  SITE / AA", "Workspace 3", "New workspace"] {
        assert!(
            rows.iter().any(|row| row.contains(label)),
            "{label}:\n{}",
            rows.join("\n")
        );
    }
    assert!(!rows.iter().any(|row| row.contains("Workspace 2")));

    app.handle_key(key(KeyCode::Down));
    app.handle_key(key(KeyCode::Enter));
    assert!(app.context_menu().is_none());
    assert_eq!(
        app.take_workspace_move(),
        Some(WorkspaceMove {
            project: "aa".into(),
            to: WorkspaceTarget::Existing(2),
        }),
        "the agent's whole project moves"
    );
    assert!(app.take_workspace_move().is_none(), "once");
    assert!(rx.try_recv().is_err(), "nothing goes to the session");
}

#[test]
fn enter_opens_the_submenu_even_with_one_workspace() {
    let (mut app, mut rx) = app();
    app.set_workspaces(vec![String::new()], 0);
    let rows = screen(&mut app, 30);
    press(&mut app, MouseButton::Right, cell_of(&rows, "CC"));
    app.handle_key(key(KeyCode::End));
    app.handle_key(key(KeyCode::Up));
    app.handle_key(key(KeyCode::Enter));
    let entries = &submenu(&app).expect("Enter opens it").entries;
    assert_eq!(entries, &[SubmenuEntry::Workspace(WorkspaceTarget::New)]);
    app.handle_key(key(KeyCode::Enter));
    assert_eq!(
        app.take_workspace_move(),
        Some(WorkspaceMove {
            project: "cc".into(),
            to: WorkspaceTarget::New,
        })
    );
    assert!(rx.try_recv().is_err());
}

#[test]
fn the_pointer_opens_the_submenu_and_picks_a_workspace() {
    let (mut app, mut rx) = workspace_app();
    let rows = screen(&mut app, 30);
    press(&mut app, MouseButton::Right, cell_of(&rows, "CC"));
    let rows = screen(&mut app, 30);
    let item = cell_of(&rows, "Move to workspace");
    assert!(hover(&mut app, item), "redraws");
    assert!(submenu(&app).is_some(), "hovering opens it");

    let rows = screen(&mut app, 30);
    let first = cell_of(&rows, "Workspace 1");
    assert_eq!(first.1, item.1, "its first entry is on the item's row");
    let new = cell_of(&rows, "New workspace");
    assert!(hover(&mut app, new), "the hover moves, so it redraws");
    assert_eq!(submenu(&app).map(|s| s.selected), Some(2));
    let buffer = draw(&mut app, 30);
    let lit = Some(crate::Theme::cyber().colors.surface_hover);
    assert_eq!(buffer[new].style().bg, lit);
    assert_ne!(buffer[first].style().bg, lit);

    // Another item closes it, and the item opens it again.
    let rows = screen(&mut app, 30);
    hover(&mut app, cell_of(&rows, "Project info"));
    assert!(submenu(&app).is_none());
    press(&mut app, MouseButton::Left, item);
    assert!(app.context_menu().is_some(), "a click on it runs nothing");
    assert!(submenu(&app).is_some(), "a click opens it");

    let rows = screen(&mut app, 30);
    press(&mut app, MouseButton::Left, cell_of(&rows, "New workspace"));
    assert!(app.context_menu().is_none());
    assert_eq!(
        app.take_workspace_move(),
        Some(WorkspaceMove {
            project: "cc".into(),
            to: WorkspaceTarget::New,
        })
    );
    assert!(rx.try_recv().is_err(), "nothing goes to the session");
}

#[test]
fn left_or_esc_closes_only_the_submenu() {
    let (mut app, mut rx) = workspace_app();
    let rows = screen(&mut app, 30);
    press(&mut app, MouseButton::Right, cell_of(&rows, "BB"));
    app.handle_key(key(KeyCode::Down));
    app.handle_key(key(KeyCode::Down));
    for close in [KeyCode::Left, KeyCode::Esc] {
        app.handle_key(key(KeyCode::Right));
        assert!(submenu(&app).is_some());
        app.handle_key(key(close));
        assert!(submenu(&app).is_none(), "{close:?}");
        let menu = app.context_menu().expect("the menu stays");
        assert_eq!(menu.items[menu.selected], MenuItem::MoveToWorkspace);
    }

    // A click outside both closes everything.
    app.handle_key(key(KeyCode::Right));
    screen(&mut app, 30);
    press(&mut app, MouseButton::Left, (110, 1));
    assert!(app.context_menu().is_none());
    assert!(app.take_workspace_move().is_none());
    assert!(rx.try_recv().is_err());
}

#[test]
fn the_submenu_stays_on_screen_near_the_edges() {
    let (mut app, _rx) = workspace_app();
    let rows = screen(&mut app, 12);
    press(&mut app, MouseButton::Right, cell_of(&rows, "BB"));
    app.handle_key(key(KeyCode::End));
    app.handle_key(key(KeyCode::Up));
    app.handle_key(key(KeyCode::Right));
    screen(&mut app, 12);
    let menu = app.context_menu().and_then(|m| m.area).expect("drawn");
    let sub = submenu(&app).and_then(|s| s.area).expect("drawn");
    assert!(sub.bottom() <= 12, "{sub:?}");
    assert_eq!(sub.x, menu.right(), "right of the menu");

    // Opened near the right edge, it goes left of the menu.
    if let Some(open) = app.context_menu.as_mut() {
        open.anchor = Position::new(118, 2);
    }
    screen(&mut app, 30);
    let menu = app.context_menu().and_then(|m| m.area).expect("drawn");
    let sub = submenu(&app).and_then(|s| s.area).expect("drawn");
    assert!(menu.right() <= 120, "{menu:?}");
    assert_eq!(sub.right(), menu.x, "left of the menu: {sub:?} {menu:?}");
    assert!(sub.bottom() <= 30);
}

#[test]
fn new_workspaces_refresh_an_open_submenu() {
    let (mut app, _rx) = workspace_app();
    let rows = screen(&mut app, 30);
    press(&mut app, MouseButton::Right, cell_of(&rows, "CC"));
    app.handle_key(key(KeyCode::End));
    app.handle_key(key(KeyCode::Up));
    app.handle_key(key(KeyCode::Right));
    app.handle_key(key(KeyCode::End));
    app.set_workspaces(vec!["A".into(), "B".into()], 0);
    let open = submenu(&app).expect("still open");
    assert_eq!(
        open.entries,
        [
            SubmenuEntry::Workspace(WorkspaceTarget::Existing(1)),
            SubmenuEntry::Workspace(WorkspaceTarget::New)
        ]
    );
    assert_eq!(open.selected, 1, "kept in range");

    // No workspaces any more: nothing to move to.
    app.set_workspaces(Vec::new(), 0);
    let menu = app.context_menu().expect("the menu stays");
    assert!(menu.submenu.is_none());
    assert!(!menu.items.contains(&MenuItem::MoveToWorkspace));
}

#[test]
fn the_only_project_listed_is_offered_no_new_workspace() {
    let (mut app, _rx) = one_project_app();
    app.set_workspaces(vec!["A".into(), "B".into()], 0);
    let open_submenu = |app: &mut App| {
        let rows = screen(app, 30);
        press(app, MouseButton::Right, cell_of(&rows, "BB"));
        // Open, Archive, Move to workspace, Remove project…
        app.handle_key(key(KeyCode::End));
        app.handle_key(key(KeyCode::Up));
        app.handle_key(key(KeyCode::Right));
        submenu(app).expect("Right opens it").entries.clone()
    };
    assert_eq!(
        open_submenu(&mut app),
        [SubmenuEntry::Workspace(WorkspaceTarget::Existing(1))]
    );
    let rows = screen(&mut app, 30);
    assert!(rows.iter().any(|row| row.contains("Workspace 2")));
    assert!(!rows.iter().any(|row| row.contains("New workspace")));
    app.handle_key(key(KeyCode::Esc));
    app.handle_key(key(KeyCode::Esc));

    // A project with every agent archived is listed too.
    app.process_command(empty_project("ee"));
    assert_eq!(
        open_submenu(&mut app),
        [
            SubmenuEntry::Workspace(WorkspaceTarget::Existing(1)),
            SubmenuEntry::Workspace(WorkspaceTarget::New)
        ]
    );
}

#[test]
fn one_project_and_one_workspace_offer_no_move() {
    let (mut app, _rx) = one_project_app();
    app.set_workspaces(vec![String::new()], 0);
    let rows = screen(&mut app, 30);
    // The project's header, then its agents.
    for name in ["AA", "BB"] {
        press(&mut app, MouseButton::Right, cell_of(&rows, name));
        let menu = app.context_menu().expect("the menu opened");
        assert!(!menu.items.contains(&MenuItem::MoveToWorkspace), "{name}");
        assert!(menu.items.contains(&MenuItem::RemoveProject), "{name}");
    }
    app.handle_key(key(KeyCode::Esc));

    // A second project gives the first one somewhere to go.
    app.process_command(empty_project("ee"));
    let rows = screen(&mut app, 30);
    press(&mut app, MouseButton::Right, cell_of(&rows, "BB"));
    let menu = app.context_menu().expect("the menu opened");
    assert!(menu.items.contains(&MenuItem::MoveToWorkspace));
}
