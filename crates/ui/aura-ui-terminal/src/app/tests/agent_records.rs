use super::*;

#[test]
fn test_records_of_background_agents_are_not_listed() {
    let (mut app, _rx) = app_with_agents();
    let record = |agent_id: &str| crate::events::RecordSummary {
        seq: 1,
        timestamp: String::new(),
        full_hash: String::new(),
        hash_suffix: String::new(),
        tx_kind: "UserPrompt".to_string(),
        sender: String::new(),
        message: String::new(),
        action_count: 0,
        effect_status: String::new(),
        status: crate::events::RecordStatus::Ok,
        info: String::new(),
        what: String::new(),
        group: String::new(),
        detail: String::new(),
        error_details: String::new(),
        tx_id: String::new(),
        agent_id: agent_id.to_string(),
        ts_ms: 0,
        policy: None,
    };

    app.process_command(UiCommand::NewRecord(record("bb")));
    app.process_command(UiCommand::NewRecord(record("aa")));

    assert_eq!(app.records().len(), 1);
}

fn record_of(agent_id: &str, seq: u64) -> crate::events::RecordSummary {
    crate::events::RecordSummary {
        seq,
        timestamp: String::new(),
        full_hash: format!("hash{seq}"),
        hash_suffix: String::new(),
        tx_kind: "UserPrompt".to_string(),
        sender: String::new(),
        message: String::new(),
        action_count: 0,
        effect_status: String::new(),
        status: crate::events::RecordStatus::Ok,
        info: String::new(),
        what: String::new(),
        group: String::new(),
        detail: String::new(),
        error_details: String::new(),
        tx_id: String::new(),
        agent_id: agent_id.to_string(),
        ts_ms: 0,
        policy: None,
    }
}

#[test]
fn test_an_agent_comes_back_with_its_records_and_files_where_it_left_them() {
    let (mut app, _rx) = app_with_agents();
    let (dir_a, dir_b) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
    std::fs::create_dir_all(dir_a.path().join("src")).unwrap();
    std::fs::write(dir_a.path().join("src/main.rs"), "").unwrap();
    std::fs::write(dir_b.path().join("notes.md"), "").unwrap();
    let records_a: Vec<_> = (1..=5).map(|seq| record_of("aa", seq)).collect();
    app.process_command(UiCommand::SetRecords(records_a.clone()));
    app.process_command(UiCommand::SetSandboxRoot(dir_a.path().to_path_buf()));
    app.records.selected = 1;
    app.records.scroll = 1;
    app.sandbox.toggle_selected();
    app.sandbox.select(1);
    app.side.sandbox_scroll = 1;
    let file = dir_a.path().join("src/main.rs");
    assert_eq!(app.sandbox.rows()[1].path, file);

    // The session sends each agent's records and workspace as it comes
    // on screen.
    app.process_command(UiCommand::SetActiveAgent("bb".to_string()));
    app.process_command(UiCommand::SetRecords(vec![record_of("bb", 1)]));
    app.process_command(UiCommand::SetSandboxRoot(dir_b.path().to_path_buf()));
    assert_eq!(app.records().len(), 1);
    assert_eq!(app.sandbox.root(), Some(dir_b.path()));
    assert_eq!((app.records.scroll, app.side.sandbox_scroll), (0, 0));

    std::fs::write(dir_a.path().join("src/lib.rs"), "").unwrap();
    app.process_command(UiCommand::SetActiveAgent("aa".to_string()));
    assert_eq!(
        app.records().len(),
        5,
        "back from memory before the session answers"
    );
    app.process_command(UiCommand::SetRecords(records_a));
    app.process_command(UiCommand::SetSandboxRoot(dir_a.path().to_path_buf()));
    assert_eq!((app.records.selected, app.records.scroll), (1, 1));
    assert_eq!(app.sandbox.rows()[app.sandbox.selected()].path, file);
    assert_eq!(app.side.sandbox_scroll, 1);

    // The tree is read again on the next tick, keeping its folds.
    app.tick();
    let names: Vec<&str> = app.sandbox.rows().iter().map(|r| r.name.as_str()).collect();
    assert_eq!(names, ["src", "lib.rs", "main.rs"]);
    assert_eq!(app.sandbox.rows()[app.sandbox.selected()].path, file);
}

#[test]
fn test_alt_digits_bring_each_app_to_the_front() {
    let (mut app, _rx) = app_with_agents();
    app.focus = PanelFocus::Chat;
    for (digit, tab) in ['1', '2', '3', '4'].into_iter().zip(AgentsTab::ALL) {
        app.handle_key(KeyEvent::new(KeyCode::Char(digit), KeyModifiers::ALT));
        assert_eq!(app.agents_tab(), tab);
    }
    assert!(app.input().is_empty(), "the digits are not typed");
    app.handle_key(KeyEvent::new(KeyCode::Char('5'), KeyModifiers::ALT));
    assert_eq!(app.agents_tab(), AgentsTab::Archive);
}

#[test]
fn test_new_records_keep_the_highlight_on_its_record() {
    let (mut app, _rx) = app_with_agents();
    app.process_command(UiCommand::SetRecords(
        (1..=3).map(|s| record_of("aa", s)).collect(),
    ));
    assert_eq!(app.records.selected, 2, "on the newest");
    app.process_command(UiCommand::SetRecords(
        (1..=4).map(|s| record_of("aa", s)).collect(),
    ));
    assert_eq!(app.records.selected, 3, "following the newest");
    app.records.selected = 1;
    app.process_command(UiCommand::SetRecords(
        (1..=5).map(|s| record_of("aa", s)).collect(),
    ));
    assert_eq!(app.records.selected, 1, "left on its record");

    // Records sent for an agent the screen has left since change nothing.
    app.process_command(UiCommand::SetRecords(vec![record_of("bb", 1)]));
    assert_eq!(app.records().len(), 5);
}

/// The session reads an agent's records for the panel after it shows the
/// agent's chat; records the agent writes meanwhile come on their own,
/// before the ones read.
#[test]
fn test_records_written_after_the_panel_was_read_stay_after_them() {
    let (mut app, _rx) = app_with_agents();
    app.process_command(UiCommand::NewRecord(record_of("aa", 1)));
    app.process_command(UiCommand::SetRecords(Vec::new()));
    assert_eq!(app.records().len(), 1, "read before its first record");

    app.process_command(UiCommand::NewRecord(record_of("aa", 7)));
    app.process_command(UiCommand::SetRecords(
        (1..=6).map(|seq| record_of("aa", seq)).collect(),
    ));
    let seqs: Vec<u64> = app.records().iter().map(|r| r.seq).collect();
    assert_eq!(seqs, [1, 2, 3, 4, 5, 6, 7]);
}

#[test]
fn test_agent_command_selects_by_number_and_name() {
    let (mut app, mut rx) = app_with_agents();

    app.handle_command("/agent 2");
    assert!(matches!(rx.try_recv(), Ok(UiEvent::SelectAgent(id)) if id == "bb"));
    assert_eq!(app.active_agent_id(), "bb", "switched on screen at once");

    // Already on screen: nothing to ask for.
    app.handle_command("/agent researcher");
    assert!(rx.try_recv().is_err());

    // The first agent's chat is held in memory, so the session is only
    // told it is in front.
    app.handle_command("/agent 1");
    assert!(matches!(rx.try_recv(), Ok(UiEvent::FocusAgent(id)) if id == "aa"));

    app.handle_command("/agent nobody");
    assert!(rx.try_recv().is_err());
}

#[test]
fn test_agent_new_command_emits_create_agent() {
    let (mut app, mut rx) = app_with_agents();

    app.handle_command("/agent new Code Reviewer");
    assert!(matches!(
        rx.try_recv(),
        Ok(UiEvent::CreateAgent(Some(name))) if name == "Code Reviewer"
    ));

    app.handle_command("/agent new");
    assert!(matches!(rx.try_recv(), Ok(UiEvent::CreateAgent(None))));
}

#[test]
fn test_project_folds_its_first_agent() {
    use super::super::ProjectListRow;

    let (mut app, mut rx) = app_with_agents();
    assert_eq!(
        app.project_rows(),
        vec![
            ProjectListRow::Project {
                root: 0,
                expanded: true,
            },
            ProjectListRow::Agent { index: 0 },
            ProjectListRow::Gap,
            ProjectListRow::Project {
                root: 1,
                expanded: true,
            },
            ProjectListRow::Agent { index: 1 },
            ProjectListRow::NewProject,
        ]
    );

    app.focus = PanelFocus::Swarm;
    app.handle_key(key(KeyCode::Char(' ')));
    assert!(rx.try_recv().is_err());
    assert_eq!(
        app.project_rows(),
        vec![
            ProjectListRow::Project {
                root: 0,
                expanded: false,
            },
            ProjectListRow::Gap,
            ProjectListRow::Project {
                root: 1,
                expanded: true,
            },
            ProjectListRow::Agent { index: 1 },
            ProjectListRow::NewProject,
        ]
    );

    app.handle_key(key(KeyCode::Enter));
    assert!(app
        .project_rows()
        .iter()
        .any(|row| matches!(row, ProjectListRow::Agent { index: 0 })));
}

#[test]
fn test_agents_of_one_project_stay_in_it_unindented() {
    use super::super::ProjectListRow;

    let (mut app, _rx) = app_with_agents();
    let in_project = |project: &str, id: &str, name: &str| crate::events::AgentSummary {
        project_id: Some(project.to_string()),
        ..agent(id, name)
    };
    let spawned = crate::events::AgentSummary {
        depth: 1,
        ..in_project("site", "dd", "explorer #1")
    };
    app.process_command(UiCommand::SetAgents(vec![
        in_project("site", "aa", "Agent"),
        in_project("site", "bb", "Agent 2"),
        spawned,
        in_project("site", "cc", "reviewer #1"),
        in_project("notes", "ee", "Other"),
    ]));

    assert_eq!(
        app.project_rows(),
        vec![
            ProjectListRow::Project {
                root: 0,
                expanded: true,
            },
            ProjectListRow::Agent { index: 0 },
            ProjectListRow::Agent { index: 1 },
            ProjectListRow::Agent { index: 2 },
            ProjectListRow::Agent { index: 3 },
            ProjectListRow::Gap,
            ProjectListRow::Project {
                root: 4,
                expanded: true,
            },
            ProjectListRow::Agent { index: 4 },
            ProjectListRow::NewProject,
        ]
    );
}
