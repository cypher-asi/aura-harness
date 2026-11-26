
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
        Some(UiEvent::CreateChannel { slug, title, description, project_id: None })
            if slug == "zode" && title == "zode" && description.is_empty()
    ));
}

#[test]
fn tab_order_wraps_both_ways_through_the_project() {
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
            ChannelField::Slug
        ]
    );
    form.move_focus(false);
    assert_eq!(form.focus(), ChannelField::Project);
}

#[test]
fn left_and_right_cycle_none_and_every_project_both_ways() {
    let projects = [project("p1", "alpha"), project("p2", "beta")];
    let mut form = ChannelForm::create(&projects, None);
    form.focus = ChannelField::Project;
    let mut forward = Vec::new();
    for _ in 0..3 {
        assert!(form.edit_key(KeyEvent::from(KeyCode::Right)));
        forward.push(chosen(&form).map(str::to_string));
    }
    assert_eq!(
        forward,
        [Some("p1".to_string()), Some("p2".to_string()), None]
    );
    assert!(form.edit_key(KeyEvent::from(KeyCode::Left)));
    assert_eq!(chosen(&form), Some("p2"));
    assert!(form.edit_key(KeyEvent::from(KeyCode::Char(' '))));
    assert_eq!(chosen(&form), None, "Space goes on, wrapping to None");
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
    form.focus = ChannelField::Project;
    form.move_focus(true);
    assert_eq!(form.focus(), ChannelField::Title, "Tab skips the slug");
    form.move_focus(false);
    assert_eq!(form.focus(), ChannelField::Project);
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
        Some(UiEvent::UpdateChannel { channel_id, title, project_id: None, .. })
            if channel_id == "c1" && title == "Zode release"
    ));
}
