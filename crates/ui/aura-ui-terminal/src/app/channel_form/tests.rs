use super::*;

fn project(id: &str, name: &str) -> ProjectSummary {
    ProjectSummary {
        id: id.into(),
        name: name.into(),
        folder: format!("/src/{name}"),
        ..ProjectSummary::default()
    }
}

fn view(id: &str, name: &str) -> ChannelProjectView {
    ChannelProjectView {
        id: id.into(),
        name: name.into(),
        folder: format!("/src/{name}"),
    }
}

fn chosen(form: &ChannelForm) -> Option<&str> {
    form.project().map(|project| project.id.as_str())
}

#[test]
fn slugs_are_lowercase_letters_digits_and_dashes_up_to_40() {
    for (slug, valid) in [
        ("release", true),
        ("v2-launch", true),
        ("-", true),
        (&"a".repeat(MAX_SLUG), true),
        ("", false),
        (&"a".repeat(MAX_SLUG + 1), false),
        ("Release", false),
        ("two words", false),
        ("under_score", false),
        ("café", false),
    ] {
        assert_eq!(is_valid_slug(slug), valid, "{slug:?}");
    }
}

#[test]
fn typed_uppercase_is_lowercased_in_the_slug_only() {
    let mut form = ChannelForm::default();
    form.insert("Zode");
    form.focus = ChannelField::Title;
    form.insert("Zode\nrelease");
    assert_eq!(form.text(ChannelField::Slug), "zode");
    assert_eq!(form.text(ChannelField::Title), "Zoderelease");
}

#[test]
fn an_empty_title_becomes_the_slug() {
    let mut form = ChannelForm::default();
    form.insert("zode");
    form.focus = ChannelField::Title;
    form.insert("   ");
    assert!(matches!(
        form.submission(),
        Ok(UiEvent::CreateChannel {
            slug,
            title,
            description,
            project_id: None,
            placement: PlacementView::Local,
        }) if slug == "zode" && title == "zode" && description.is_empty()
    ));
}

#[test]
fn tab_order_wraps_both_ways_through_the_project_and_where_agents_run() {
    for field in ChannelField::ALL {
        assert_eq!(field.next().previous(), field);
    }
    let mut form = ChannelForm::default();
    let mut order = Vec::new();
    for _ in 0..ChannelField::ALL.len() {
        form.move_focus(true);
        order.push(form.focus());
    }
    assert_eq!(
        order,
        [
            ChannelField::Title,
            ChannelField::Description,
            ChannelField::Project,
            ChannelField::Placement,
            ChannelField::Slug
        ]
    );
    form.move_focus(false);
    assert_eq!(form.focus(), ChannelField::Placement);
}

#[test]
fn where_agents_run_flips_both_ways_and_the_cloud_needs_a_project() {
    let mut form = ChannelForm::create(&[project("p1", "alpha")], None);
    form.insert("zode");
    form.focus = ChannelField::Placement;
    assert_eq!(form.placement(), PlacementView::Local, "agents start local");
    for (code, placement) in [
        (KeyCode::Right, PlacementView::Cloud),
        (KeyCode::Left, PlacementView::Local),
        (KeyCode::Char(' '), PlacementView::Cloud),
    ] {
        assert!(form.edit_key(KeyEvent::from(code)));
        assert_eq!(form.placement(), placement, "{code:?}");
    }
    assert!(!form.edit_key(KeyEvent::from(KeyCode::Char('a'))));
    form.insert("pasted");
    assert!(form.fields.iter().all(|field| field.text != "pasted"));

    assert!(form.cloud_without_project());
    assert_eq!(form.submission().err(), Some(ChannelField::Project));
    form.focus = ChannelField::Project;
    assert!(form.edit_key(KeyEvent::from(KeyCode::Right)));
    assert!(!form.cloud_without_project());
    assert!(matches!(
        form.submission(),
        Ok(UiEvent::CreateChannel {
            project_id: Some(project),
            placement: PlacementView::Cloud,
            ..
        }) if project == "p1"
    ));
}

#[test]
fn a_bad_slug_is_fixed_before_the_project() {
    let mut form = ChannelForm::create(&[], None);
    form.placement = PlacementView::Cloud;
    assert_eq!(form.submission().err(), Some(ChannelField::Slug));
    form.insert("zode");
    assert_eq!(form.submission().err(), Some(ChannelField::Project));
}

#[test]
fn left_and_right_cycle_none_every_project_and_new_project() {
    let projects = [project("p1", "alpha"), project("p2", "beta")];
    let mut form = ChannelForm::create(&projects, None);
    form.insert("zode");
    form.focus = ChannelField::Project;
    let mut forward = Vec::new();
    for _ in 0..4 {
        assert!(form.edit_key(KeyEvent::from(KeyCode::Right)));
        forward.push(chosen(&form).map(str::to_string));
    }
    assert_eq!(
        forward,
        [Some("p1".to_string()), Some("p2".to_string()), None, None]
    );
    assert!(!form.is_new_project(), "the last step wraps to None");
    assert!(form.edit_key(KeyEvent::from(KeyCode::Left)));
    assert!(
        form.is_new_project(),
        "None's previous choice is New project"
    );
    assert_eq!(form.submission().err(), Some(ChannelField::Project));
    assert!(form.edit_key(KeyEvent::from(KeyCode::Left)));
    assert_eq!(chosen(&form), Some("p2"));
    assert!(form.edit_key(KeyEvent::from(KeyCode::Char(' '))));
    assert!(form.is_new_project(), "Space goes on to New project");
}

#[test]
fn typing_on_the_project_is_ignored() {
    let mut form = ChannelForm::create(&[project("p1", "alpha")], None);
    form.focus = ChannelField::Project;
    assert!(!form.edit_key(KeyEvent::from(KeyCode::Char('a'))));
    form.insert("pasted");
    assert_eq!(chosen(&form), None);
    assert!(form.fields.iter().all(|field| field.text.is_empty()));
}

#[test]
fn a_chosen_project_that_drops_out_of_the_list_falls_back_to_none() {
    let mut form = ChannelForm::create(
        &[project("p1", "alpha"), project("p2", "beta")],
        Some("p2".into()),
    );
    assert_eq!(chosen(&form), Some("p2"));
    form.set_projects(&[project("p2", "beta"), project("p3", "gamma")]);
    assert_eq!(chosen(&form), Some("p2"), "still listed");
    form.set_projects(&[project("p1", "alpha")]);
    assert_eq!(chosen(&form), None);
}

#[test]
fn a_default_project_that_is_not_listed_is_none() {
    let form = ChannelForm::create(&[project("p1", "alpha")], Some("gone".into()));
    assert_eq!(chosen(&form), None);
}

#[test]
fn editing_keeps_the_channels_own_project_and_skips_the_slug() {
    let channel = ChannelSummary {
        id: "c1".into(),
        slug: "zode".into(),
        title: "Zode".into(),
        project: Some(view("p9", "own")),
        ..ChannelSummary::default()
    };
    let mut form = ChannelForm::edit(&channel, &[project("p1", "alpha")]);
    assert_eq!(form.focus(), ChannelField::Title);
    assert_eq!(chosen(&form), Some("p9"), "kept though not listed");
    form.set_projects(&[]);
    assert_eq!(chosen(&form), Some("p9"));
    form.focus = ChannelField::Placement;
    form.move_focus(true);
    assert_eq!(form.focus(), ChannelField::Title, "Tab skips the slug");
    form.move_focus(false);
    assert_eq!(form.focus(), ChannelField::Placement);
}

#[test]
fn editing_starts_on_where_the_channels_agents_run_and_sends_it_back() {
    let channel = ChannelSummary {
        id: "c1".into(),
        slug: "zode".into(),
        title: "Zode".into(),
        project: Some(view("p9", "own")),
        placement: PlacementView::Cloud,
        ..ChannelSummary::default()
    };
    let mut form = ChannelForm::edit(&channel, &[]);
    assert_eq!(form.placement(), PlacementView::Cloud);
    assert!(matches!(
        form.submission(),
        Ok(UiEvent::UpdateChannel {
            placement: PlacementView::Cloud,
            ..
        })
    ));

    // Clearing the project of a cloud channel is refused until it runs
    // its agents locally. "None" is the choice before the channel's
    // project; the one after is "New project".
    form.focus = ChannelField::Project;
    assert!(form.edit_key(KeyEvent::from(KeyCode::Left)));
    assert_eq!(chosen(&form), None);
    assert!(!form.is_new_project());
    assert_eq!(form.submission().err(), Some(ChannelField::Project));
    form.focus = ChannelField::Placement;
    assert!(form.edit_key(KeyEvent::from(KeyCode::Left)));
    assert!(matches!(
        form.submission(),
        Ok(UiEvent::UpdateChannel {
            project_id: None,
            placement: PlacementView::Local,
            ..
        })
    ));
}

#[test]
fn a_picked_folder_becomes_the_chosen_project_once_it_is_listed() {
    let dir = tempfile::tempdir().unwrap();
    let folder = dir.path().canonicalize().unwrap();
    let mut form = ChannelForm::create(&[], None);
    form.insert("zode");
    form.focus = ChannelField::Project;
    assert!(form.edit_key(KeyEvent::from(KeyCode::Left)));
    assert!(form.is_new_project());
    form.await_created_folder(dir.path().to_path_buf());
    assert_eq!(form.submission().err(), Some(ChannelField::Project));
    form.set_projects(&[ProjectSummary {
        id: "p9".into(),
        name: "site".into(),
        folder: folder.display().to_string(),
        ..ProjectSummary::default()
    }]);
    assert_eq!(chosen(&form), Some("p9"));
    assert!(form.pending_folder().is_none());
    assert!(matches!(
        form.submission(),
        Ok(UiEvent::CreateChannel { project_id: Some(project), .. }) if project == "p9"
    ));
}

#[test]
fn leaving_new_project_forgets_the_folder_it_was_waiting_on() {
    let mut form = ChannelForm::create(&[project("p1", "alpha")], None);
    form.focus = ChannelField::Project;
    assert!(form.edit_key(KeyEvent::from(KeyCode::Left)));
    form.await_created_folder(std::path::PathBuf::from("/src/gamma"));
    assert!(form.edit_key(KeyEvent::from(KeyCode::Right)));
    assert!(!form.is_new_project());
    form.set_projects(&[project("p1", "alpha"), project("p9", "gamma")]);
    assert_eq!(chosen(&form), None, "the folder was forgotten");
}

#[test]
fn an_empty_title_in_an_edit_keeps_the_old_one() {
    let channel = ChannelSummary {
        id: "c1".into(),
        slug: "zode".into(),
        title: "Zode release".into(),
        ..ChannelSummary::default()
    };
    let mut form = ChannelForm::edit(&channel, &[]);
    form.fields[1] = FieldText::default();
    assert!(matches!(
        form.submission(),
        Ok(UiEvent::UpdateChannel { channel_id, title, project_id: None, .. })
            if channel_id == "c1" && title == "Zode release"
    ));
}
