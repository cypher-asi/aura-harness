use super::*;

// ========================================================================
// Channel form and the CHANNELS tab's menu
// ========================================================================

/// `app_with_channel` with "+ New channel" chosen.
fn app_with_channel_form() -> (App, mpsc::Receiver<UiEvent>) {
    let (mut app, rx) = app_with_channel();
    open_new_channel_form(&mut app);
    (app, rx)
}

/// Choose "+ New channel" on the CHANNELS list.
fn open_new_channel_form(app: &mut App) {
    app.handle_key(key(KeyCode::Down));
    assert_eq!(app.selected_channel(), app.channels().len());
    app.handle_key(key(KeyCode::Enter));
    assert_eq!(app.state(), AppState::ChannelForm);
}

/// `app_with_channel_form` with projects `alpha` and `beta` listed.
fn app_with_channel_form_and_projects() -> (App, mpsc::Receiver<UiEvent>) {
    let (mut app, rx) = app_with_channel();
    app.process_command(alpha_and_beta());
    open_new_channel_form(&mut app);
    (app, rx)
}

fn form_focus(app: &App) -> Option<super::super::channel_form::ChannelField> {
    app.channel_form()
        .map(super::super::channel_form::ChannelForm::focus)
}

fn form_project(app: &App) -> Option<String> {
    app.channel_form()
        .and_then(super::super::channel_form::ChannelForm::project)
        .map(|project| project.id.clone())
}

#[test]
fn test_new_channel_row_opens_the_form() {
    let (mut app, mut rx) = app_with_channel_form();
    assert_eq!(app.state(), AppState::ChannelForm);
    assert_eq!(
        form_focus(&app),
        Some(super::super::channel_form::ChannelField::Slug)
    );
    let text = drawn_text(&mut app);
    for label in [
        "NEW CHANNEL",
        "Slug",
        "Title",
        "Description",
        "Project",
        "‹ None ›",
    ] {
        assert!(text.contains(label), "{label}:\n{text}");
    }
    assert!(!text.contains("Esc cancel"), "no key hints:\n{text}");
    assert!(rx.try_recv().is_err(), "opening sends nothing");
}

#[test]
fn test_an_invalid_slug_shows_the_hint_and_blocks_submit() {
    let (mut app, mut rx) = app_with_channel_form();
    type_keys(&mut app, "bad slug!");
    let text = drawn_text(&mut app);
    assert!(
        text.contains(super::super::channel_form::SLUG_HINT),
        "{text}"
    );

    app.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::CONTROL));
    assert_eq!(app.state(), AppState::ChannelForm, "still open");
    assert!(rx.try_recv().is_err(), "nothing sent");

    // Enter on the last field submits, but not this slug either.
    app.handle_key(key(KeyCode::BackTab));
    assert_eq!(
        form_focus(&app),
        Some(super::super::channel_form::ChannelField::Project)
    );
    app.handle_key(key(KeyCode::Enter));
    assert_eq!(app.state(), AppState::ChannelForm);
    assert_eq!(
        form_focus(&app),
        Some(super::super::channel_form::ChannelField::Slug),
        "the cursor goes back to the slug"
    );
    assert!(rx.try_recv().is_err());
}

#[test]
fn test_a_valid_form_submits_create_channel_and_closes() {
    let (mut app, mut rx) = app_with_channel_form();
    type_keys(&mut app, "Zode-2");
    let text = drawn_text(&mut app);
    assert!(
        !text.contains(super::super::channel_form::SLUG_HINT),
        "uppercase is lowercased, so the slug is good:\n{text}"
    );
    app.handle_key(key(KeyCode::Tab));
    type_keys(&mut app, "Zode two");
    app.handle_key(key(KeyCode::Down));
    type_keys(&mut app, "Ship it");
    app.handle_key(key(KeyCode::Enter));
    assert_eq!(
        form_focus(&app),
        Some(super::super::channel_form::ChannelField::Project),
        "Enter on the description goes to the project"
    );
    assert!(rx.try_recv().is_err(), "not submitted yet");
    app.handle_key(key(KeyCode::Enter));

    assert_eq!(app.state(), AppState::Idle);
    assert!(app.channel_form().is_none());
    assert!(matches!(
        rx.try_recv(),
        Ok(UiEvent::CreateChannel { slug, title, description, project_id: None })
            if slug == "zode-2" && title == "Zode two" && description == "Ship it"
    ));
    assert!(rx.try_recv().is_err(), "exactly one event");
}

#[test]
fn test_tab_in_the_form_walks_every_field_through_the_project_and_wraps() {
    use super::super::channel_form::ChannelField;
    let (mut app, _rx) = app_with_channel_form();
    let mut forward = Vec::new();
    for _ in 0..ChannelField::ALL.len() {
        app.handle_key(key(KeyCode::Tab));
        forward.extend(form_focus(&app));
    }
    assert_eq!(
        forward,
        [
            ChannelField::Title,
            ChannelField::Description,
            ChannelField::Project,
            ChannelField::Slug
        ]
    );
    app.handle_key(key(KeyCode::Up));
    assert_eq!(form_focus(&app), Some(ChannelField::Project), "Up wraps");
}

#[test]
fn test_left_and_right_on_the_project_cycle_none_and_each_project() {
    let (mut app, _rx) = app_with_channel_form_and_projects();
    app.handle_key(key(KeyCode::BackTab));
    assert_eq!(form_project(&app), None);
    let mut forward = Vec::new();
    for _ in 0..3 {
        app.handle_key(key(KeyCode::Right));
        forward.push(form_project(&app));
    }
    assert_eq!(
        forward,
        [Some("p1".to_string()), Some("p2".to_string()), None]
    );
    let mut back = Vec::new();
    for _ in 0..3 {
        app.handle_key(key(KeyCode::Left));
        back.push(form_project(&app));
    }
    assert_eq!(back, [Some("p2".to_string()), Some("p1".to_string()), None]);
}

#[test]
fn test_enter_on_the_project_submits_the_chosen_project() {
    let (mut app, mut rx) = app_with_channel_form_and_projects();
    type_keys(&mut app, "zode-2");
    app.handle_key(key(KeyCode::BackTab));
    app.handle_key(key(KeyCode::Right));
    app.handle_key(key(KeyCode::Right));
    app.handle_key(key(KeyCode::Enter));
    assert_eq!(app.state(), AppState::Idle);
    assert!(matches!(
        rx.try_recv(),
        Ok(UiEvent::CreateChannel { slug, title, project_id: Some(project), .. })
            if slug == "zode-2" && title == "zode-2" && project == "p2"
    ));
    assert!(rx.try_recv().is_err());
}

#[test]
fn test_a_new_channel_starts_on_the_project_of_the_agent_on_screen_before_channels() {
    let in_project = |id: &str, project: &str| crate::events::AgentSummary {
        project_id: Some(project.to_string()),
        ..agent(id, id)
    };
    let (mut app, _rx) = app_with_agents();
    app.process_command(alpha_and_beta());
    app.process_command(UiCommand::SetAgents(vec![
        in_project("aa", "p1"),
        in_project("bb", "p2"),
    ]));
    app.process_command(UiCommand::SetActiveAgent("bb".to_string()));
    app.process_command(UiCommand::SetChannels(vec![zode("aa")]));
    app.focus = PanelFocus::Swarm;
    app.handle_key(key(KeyCode::Left));
    assert_eq!(app.agents_tab(), AgentsTab::Channels);
    open_new_channel_form(&mut app);
    assert_eq!(form_project(&app), Some("p2".to_string()));
    let text = drawn_text(&mut app);
    assert!(text.contains("‹ beta ›"), "{text}");
    assert!(text.contains("/src/beta"), "the folder shows:\n{text}");
}

#[test]
fn test_a_new_channel_from_an_agent_without_a_project_starts_on_none() {
    let (app, _rx) = app_with_channel_form_and_projects();
    assert_eq!(form_project(&app), None);
}

#[test]
fn test_a_chosen_project_that_leaves_the_projects_list_falls_back_to_none() {
    let (mut app, _rx) = app_with_channel_form_and_projects();
    app.handle_key(key(KeyCode::BackTab));
    app.handle_key(key(KeyCode::Right));
    assert_eq!(form_project(&app), Some("p1".to_string()));
    app.process_command(UiCommand::SetProjects(vec![
        project_summary("p1", "alpha"),
        project_summary("p3", "gamma"),
    ]));
    assert_eq!(form_project(&app), Some("p1".to_string()), "still listed");
    app.process_command(UiCommand::SetProjects(vec![project_summary("p2", "beta")]));
    assert_eq!(form_project(&app), None);
    assert_eq!(app.state(), AppState::ChannelForm, "the form stays open");
}

#[test]
fn test_esc_cancels_the_form_without_an_event() {
    let (mut app, mut rx) = app_with_channel_form();
    type_keys(&mut app, "zode");
    app.handle_key(key(KeyCode::Esc));
    assert_eq!(app.state(), AppState::Idle);
    assert!(app.channel_form().is_none());
    assert!(rx.try_recv().is_err());
}

/// Right-click `#zode`'s row of the list, which shows only its slug, not
/// the chat bar's title row naming the open channel.
fn right_click_on_channel_row(app: &mut App) {
    right_click_on_where(app, "#zode", |row| !row.contains("Zode release"));
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
    assert_eq!(app.state(), AppState::Idle);
    assert!(app.channel_form().is_none());
    assert!(matches!(
        rx.try_recv(),
        Ok(UiEvent::UpdateChannel { channel_id, title, description, project_id: Some(project) })
            if channel_id == "c1"
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
    app.handle_key(key(KeyCode::Right));
    assert_eq!(form_project(&app), None, "after the last project is None");
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

#[test]
fn test_kernel_panel_commands_leave_the_layout_alone_while_a_channel_is_open() {
    let (_dir, mut app) = app_in_channel("co");
    let before = (
        app.panels.record_visible,
        app.panels.side_expanded,
        app.panels.sandbox_visible,
    );
    app.show_members();
    app.show_policies();
    app.show_inbox();
    assert_eq!(
        (
            app.panels.record_visible,
            app.panels.side_expanded,
            app.panels.sandbox_visible,
        ),
        before
    );
    assert!(
        !app.record_panel_visible(),
        "the channel still fills the column"
    );
}
