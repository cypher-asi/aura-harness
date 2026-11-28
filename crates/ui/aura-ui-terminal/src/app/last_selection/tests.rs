use super::*;

fn pick(project: &str, agent: &str) -> ProjectPick {
    ProjectPick {
        project: project.to_string(),
        agent: agent.to_string(),
    }
}

fn sample() -> LastSelection {
    LastSelection {
        channels_in_front: true,
        channel: Some("c1".to_string()),
        projects: vec![pick("p1", "a1"), pick("p2", "b2")],
    }
}

#[test]
fn a_saved_selection_loads_back_the_same() {
    let dir = tempfile::tempdir().expect("temp dir");
    let path = LastSelection::file_in(dir.path());
    sample().save(&path).expect("temp dir is writable");
    assert_eq!(LastSelection::load(&path), sample());
    assert!(
        !path.with_extension("json.tmp").exists(),
        "the temporary file is moved into place"
    );
}

#[test]
fn saving_again_replaces_the_selection() {
    let dir = tempfile::tempdir().expect("temp dir");
    let path = LastSelection::file_in(dir.path());
    sample().save(&path).expect("temp dir is writable");
    LastSelection::default()
        .save(&path)
        .expect("temp dir is writable");
    assert_eq!(LastSelection::load(&path), LastSelection::default());
}

#[test]
fn a_missing_file_loads_as_nothing_selected() {
    let dir = tempfile::tempdir().expect("temp dir");
    let path = LastSelection::file_in(dir.path());
    assert_eq!(LastSelection::load(&path), LastSelection::default());
}

#[test]
fn a_file_that_is_not_a_selection_loads_as_nothing_selected() {
    let dir = tempfile::tempdir().expect("temp dir");
    let path = LastSelection::file_in(dir.path());
    for text in [
        "",
        "{",
        "not json",
        "[1, 2]",
        "{\"projects\": 3}",
        "\u{0}\u{1}",
    ] {
        std::fs::write(&path, text).expect("temp dir is writable");
        assert_eq!(
            LastSelection::load(&path),
            LastSelection::default(),
            "{text:?}"
        );
    }
}

#[test]
fn a_directory_in_place_of_the_file_loads_as_nothing_selected() {
    let dir = tempfile::tempdir().expect("temp dir");
    let path = LastSelection::file_in(dir.path());
    std::fs::create_dir(&path).expect("temp dir is writable");
    assert_eq!(LastSelection::load(&path), LastSelection::default());
}

#[test]
fn a_huge_file_is_not_read() {
    let dir = tempfile::tempdir().expect("temp dir");
    let path = LastSelection::file_in(dir.path());
    let padding = " ".repeat(usize::try_from(MAX_FILE_BYTES).expect("fits") + 1);
    std::fs::write(&path, format!("{{\"channel\": \"c1\"}}{padding}")).expect("writable");
    assert_eq!(LastSelection::load(&path), LastSelection::default());
}

#[test]
fn fields_left_out_or_unknown_still_read() {
    let read = LastSelection::parse(r#"{"channel": "c1", "from_a_later_build": 1}"#);
    assert_eq!(
        read,
        LastSelection {
            channel: Some("c1".to_string()),
            ..LastSelection::default()
        }
    );
}

#[test]
fn reading_drops_empty_ids_and_repeated_projects() {
    let read = LastSelection::parse(
        r#"{
            "channel": "",
            "projects": [
                {"project": "p1", "agent": "a1"},
                {"project": "p1", "agent": "older"},
                {"project": "", "agent": "x"},
                {"project": "p2", "agent": ""}
            ]
        }"#,
    );
    assert_eq!(read.channel, None);
    assert_eq!(read.projects, vec![pick("p1", "a1")]);
}

#[test]
fn a_new_pick_goes_first_and_replaces_its_project() {
    let mut selection = sample();
    selection.note_pick("p2", "b3");
    assert_eq!(selection.projects, vec![pick("p2", "b3"), pick("p1", "a1")]);
}

#[test]
fn picks_are_capped_dropping_the_oldest() {
    let mut selection = LastSelection::default();
    for n in 0..=MAX_PICKS {
        selection.note_pick(&format!("p{n}"), "a");
    }
    assert_eq!(selection.projects.len(), MAX_PICKS);
    assert_eq!(selection.projects[0].project, format!("p{MAX_PICKS}"));
    assert!(selection.projects.iter().all(|pick| pick.project != "p0"));
}
