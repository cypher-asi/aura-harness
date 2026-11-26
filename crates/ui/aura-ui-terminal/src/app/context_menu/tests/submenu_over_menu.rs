use super::*;

#[test]
fn the_submenus_top_border_over_the_menu_runs_nothing() {
    let (mut app, mut rx) = workspace_app();
    let (_, sub) = open_submenu_over_the_menu(&mut app);
    let border = (sub.x + 1, sub.y);
    assert_eq!(
        item_under(&app, border),
        Some(MenuItem::Archive),
        "the border hides the item above"
    );

    press(&mut app, MouseButton::Left, border);
    assert_still_open(&app, &mut rx, "click");
    hover(&mut app, border);
    assert_still_open(&app, &mut rx, "hover");
}

#[test]
fn the_submenus_bottom_border_over_the_menu_runs_nothing() {
    // One workspace to go to: the bottom border is on the next item.
    let (mut app, mut rx) = one_project_app();
    app.set_workspaces(vec!["A".into(), "B".into()], 0);
    let (_, sub) = open_submenu_over_the_menu(&mut app);
    let border = (sub.x + 1, sub.bottom() - 1);
    assert_eq!(item_under(&app, border), Some(MenuItem::RemoveProject));

    hover(&mut app, border);
    assert_still_open(&app, &mut rx, "hover");
    press(&mut app, MouseButton::Left, border);
    assert_still_open(&app, &mut rx, "click");
}

#[test]
fn an_entry_over_the_menu_still_picks_its_workspace() {
    let (mut app, _rx) = workspace_app();
    let (_, sub) = open_submenu_over_the_menu(&mut app);
    // The second entry sits on "Remove project…".
    let entry = (sub.x + 1, sub.y + 2);
    assert_eq!(item_under(&app, entry), Some(MenuItem::RemoveProject));
    hover(&mut app, entry);
    assert_eq!(submenu(&app).map(|s| s.selected), Some(1));
    press(&mut app, MouseButton::Left, entry);
    assert!(app.context_menu().is_none());
    assert_eq!(
        app.take_workspace_move(),
        Some(WorkspaceMove {
            project: "aa".into(),
            to: WorkspaceTarget::Existing(2),
        })
    );
    assert_eq!(app.state(), AppState::Idle, "no removal asked");
}
