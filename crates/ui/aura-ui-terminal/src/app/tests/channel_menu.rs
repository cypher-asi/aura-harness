use super::channel_form::{form_focus, form_project, new_project};
use super::*;
use crate::host::PlacementView;

// ========================================================================
// A channel's menu: the edit form and archiving
// ========================================================================

/// Right-click `#zode`'s row of the list, its `#` in the icon column, not
/// the chat bar's title row naming the open channel.
fn right_click_on_channel_row(app: &mut App) {
    right_click_on_where(app, "#  zode", |row| !row.contains("Zode release"));
}

/// Right-click `#zode`'s row and run "Edit channel…", the first item.
fn open_edit_form(app: &mut App) {
    right_click_on_channel_row(app);
    app.handle_key(key(KeyCode::Enter));
    assert_eq!(app.state(), AppState::ChannelForm);
}

#[test]
fn test_a_channels_menu_offers_edit_and_archive() {
    use super::super::context_menu::MenuItem;
    for (mut app, _rx) in [app_with_projects_channel(), app_with_channel()] {
        right_click_on_channel_row(&mut app);
        assert_eq!(
            menu_items(&app),
            [MenuItem::EditChannel, MenuItem::ArchiveChannel]
        );
        let text = drawn_text(&mut app);
        for label in ["Edit channel…", "Archive channel…"] {
            assert!(text.contains(label), "{label}:\n{text}");
        }
    }
}

#[test]
fn test_edit_channel_opens_the_form_filled_in_on_the_title() {
    use super::super::channel_form::ChannelField;
    let (mut app, mut rx) = app_with_projects_channel();
    open_edit_form(&mut app);
    assert!(app.context_menu().is_none());
    let form = app.channel_form().expect("the form is open");
    assert!(form.is_edit());
    assert_eq!(form.text(ChannelField::Slug), "zode");
    assert_eq!(form.text(ChannelField::Title), "Zode release");
    assert_eq!(form.text(ChannelField::Description), "Ship the zode crate");
    assert_eq!(form.focus(), ChannelField::Title);
    assert_eq!(form_project(&app), Some("p2".to_string()));
    let text = drawn_text(&mut app);
    assert!(text.contains(" EDIT #zode "), "{text}");
    assert!(rx.try_recv().is_err(), "opening sends nothing");
}

#[test]
fn test_the_edit_form_never_focuses_the_slug() {
    use super::super::channel_form::ChannelField;
    let (mut app, _rx) = app_with_projects_channel();
    open_edit_form(&mut app);
    let mut seen = Vec::new();
    for code in [KeyCode::Tab, KeyCode::Down, KeyCode::BackTab, KeyCode::Up] {
        for _ in 0..ChannelField::ALL.len() {
            app.handle_key(key(code));
            seen.extend(form_focus(&app));
        }
    }
    assert!(!seen.contains(&ChannelField::Slug), "{seen:?}");
    type_keys(&mut app, "x");
    assert_eq!(
        app.channel_form()
            .map(|form| form.text(ChannelField::Slug).to_string()),
        Some("zode".to_string())
    );
}

#[test]
fn test_the_edit_form_submits_update_channel_with_the_new_project() {
    let (mut app, mut rx) = app_with_projects_channel();
    open_edit_form(&mut app);
    type_keys(&mut app, " 2");
    app.handle_key(key(KeyCode::Tab));
    app.handle_key(key(KeyCode::Tab));
    app.handle_key(key(KeyCode::Left));
    assert_eq!(form_project(&app), Some("p1".to_string()));
    app.handle_key(key(KeyCode::Enter));
    app.handle_key(key(KeyCode::Right));
    app.handle_key(key(KeyCode::Enter));
    assert_eq!(app.state(), AppState::Idle);
    assert!(app.channel_form().is_none());
    assert!(matches!(
        rx.try_recv(),
        Ok(UiEvent::UpdateChannel {
            channel_id,
            title,
            description,
            project_id: Some(project),
            placement: PlacementView::Cloud,
        }) if channel_id == "c1"
            && title == "Zode release 2"
            && description == "Ship the zode crate"
            && project == "p1"
    ));
    assert!(rx.try_recv().is_err(), "exactly one event");
}

#[test]
fn test_choosing_none_in_the_edit_form_clears_the_project() {
    let (mut app, mut rx) = app_with_projects_channel();
    open_edit_form(&mut app);
    app.handle_key(key(KeyCode::BackTab));
    app.handle_key(key(KeyCode::BackTab));
    app.handle_key(key(KeyCode::Right));
    assert!(new_project(&app), "after the last project is New project");
    app.handle_key(key(KeyCode::Right));
    assert_eq!(form_project(&app), None, "then None");
    app.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::CONTROL));
    assert!(matches!(
        rx.try_recv(),
        Ok(UiEvent::UpdateChannel { channel_id, project_id: None, .. }) if channel_id == "c1"
    ));
}

#[test]
fn test_esc_in_the_edit_form_sends_nothing() {
    let (mut app, mut rx) = app_with_projects_channel();
    open_edit_form(&mut app);
    type_keys(&mut app, "changed");
    app.handle_key(key(KeyCode::Esc));
    assert_eq!(app.state(), AppState::Idle);
    assert!(app.channel_form().is_none());
    assert!(rx.try_recv().is_err());
}

#[test]
fn test_archive_channel_asks_first() {
    let (mut app, mut rx) = app_with_projects_channel();
    right_click_on_channel_row(&mut app);
    app.handle_key(key(KeyCode::End));
    app.handle_key(key(KeyCode::Enter));
    assert!(rx.try_recv().is_err(), "not before y");
    app.handle_key(key(KeyCode::Char('n')));
    assert!(rx.try_recv().is_err(), "anything else cancels");

    right_click_on_channel_row(&mut app);
    app.handle_key(key(KeyCode::End));
    app.handle_key(key(KeyCode::Enter));
    app.handle_key(key(KeyCode::Char('y')));
    assert!(matches!(rx.try_recv(), Ok(UiEvent::ArchiveChannel(id)) if id == "c1"));
}

#[test]
fn test_right_click_on_the_channel_side_panel_opens_its_menu() {
    use super::super::context_menu::MenuItem;
    let (mut app, _rx) = app_with_projects_channel();
    let id = app.channels()[0].id.clone();
    app.open_channel_by_id(&id);
    app.show_side_tab(SidePanelTab::ChannelInfo);
    right_click_on_where(&mut app, "PROJECT", |row| !row.contains("PROJECTS"));
    assert_eq!(
        menu_items(&app),
        [MenuItem::EditChannel, MenuItem::ArchiveChannel]
    );
    app.handle_key(key(KeyCode::Enter));
    assert!(
        app.channel_form().is_some_and(|form| form.is_edit()),
        "Edit channel… opens the edit form"
    );
}
