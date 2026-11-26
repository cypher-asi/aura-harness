use super::*;
use aura_ui_terminal::events::BootStageState;

fn boot(id: &str, update: BootUpdate) -> UiCommand {
    UiCommand::Boot {
        id: id.into(),
        update,
    }
}

fn stage(key: &str, state: BootStageState) -> BootUpdate {
    BootUpdate::Stage {
        key: key.into(),
        label: key.into(),
        state,
    }
}

fn log(range: std::ops::Range<usize>) -> BootUpdate {
    BootUpdate::Log(
        range
            .map(|i| BootLogLine {
                stderr: false,
                text: format!("line {i}"),
            })
            .collect(),
    )
}

fn at(text: &str, ms: u64) -> MessageData {
    MessageData {
        timestamp_ms: Some(ms),
        ..MessageData::new(MessageRole::User, text)
    }
}

#[test]
fn notices_and_consoles_come_back_as_they_were_sent() {
    let dir = tempfile::tempdir().unwrap();
    let agent = AgentId::generate();
    let notes = ChatNotes::new(dir.path());
    notes.record(agent, &UiCommand::ShowError("claim failed".into()));
    notes.record(
        agent,
        &boot(
            "b1",
            BootUpdate::Begin {
                title: "Moving to the cloud".into(),
            },
        ),
    );
    notes.record(agent, &boot("b1", stage("create", BootStageState::Running)));
    notes.record(agent, &boot("b1", log(0..3)));
    notes.record(
        agent,
        &boot(
            "b1",
            BootUpdate::Progress {
                key: "create".into(),
                done: 1,
                total: 2,
            },
        ),
    );
    notes.record(
        agent,
        &boot(
            "b1",
            BootUpdate::Waiting {
                key: "create".into(),
                what: Some("boxd is creating aura-1".into()),
                limit_ms: Some(180_000),
            },
        ),
    );
    notes.record(
        agent,
        &boot("b1", stage("create", BootStageState::Done { ms: 5 })),
    );
    notes.record(
        agent,
        &boot(
            "b1",
            BootUpdate::End {
                ok: true,
                summary: "Now running in the cloud".into(),
            },
        ),
    );
    notes.record(agent, &UiCommand::SetStatus("Ready".into()));

    let chat = ChatNotes::new(dir.path()).chat(agent);
    assert_eq!(chat.len(), 2, "{chat:?}");
    assert_eq!(chat[0].content, "error: claim failed");
    assert_eq!(chat[0].role, MessageRole::System);
    let history = chat[1].boot.as_ref().expect("a console");
    assert_eq!(history.id, "b1");
    assert!(history.elapsed_ms.is_some());
    assert!(
        !history
            .updates
            .iter()
            .any(|u| matches!(u, BootUpdate::Progress { .. })),
        "progress is not kept"
    );
    assert!(
        !history
            .updates
            .iter()
            .any(|u| matches!(u, BootUpdate::Waiting { .. })),
        "waiting lines are not kept"
    );
    let logged: Vec<&str> = history
        .updates
        .iter()
        .filter_map(|u| match u {
            BootUpdate::Log(lines) => Some(lines.iter().map(|l| l.text.as_str())),
            _ => None,
        })
        .flatten()
        .collect();
    assert_eq!(logged, ["line 0", "line 1", "line 2"]);
    assert!(matches!(
        history.updates.last(),
        Some(BootUpdate::End { ok: true, .. })
    ));
}

#[test]
fn a_console_left_running_by_another_process_shows_as_stopped() {
    let dir = tempfile::tempdir().unwrap();
    let agent = AgentId::generate();
    let notes = ChatNotes::new(dir.path());
    notes.record(agent, &boot("b1", stage("create", BootStageState::Running)));
    notes.record(agent, &boot("b1", log(0..2)));

    let live = notes.chat(agent);
    let updates = &live[0].boot.as_ref().unwrap().updates;
    assert!(
        matches!(updates.last(), Some(BootUpdate::Log(lines)) if lines.len() == 2),
        "running here, it goes on with the lines not written yet: {updates:?}"
    );
    assert_eq!(live[0].boot.as_ref().unwrap().elapsed_ms, None);

    let after_restart = ChatNotes::new(dir.path()).chat(agent);
    let history = after_restart[0].boot.as_ref().unwrap();
    assert!(matches!(
        history.updates.last(),
        Some(BootUpdate::End { ok: false, summary }) if summary == INTERRUPTED
    ));
}

#[test]
fn a_console_keeps_its_newest_log_lines() {
    let dir = tempfile::tempdir().unwrap();
    let agent = AgentId::generate();
    let notes = ChatNotes::new(dir.path());
    notes.record(agent, &boot("b1", log(0..KEPT_LOG_LINES + 10)));
    notes.record(
        agent,
        &boot(
            "b1",
            BootUpdate::End {
                ok: false,
                summary: "build failed".into(),
            },
        ),
    );
    let chat = notes.chat(agent);
    let Some(BootUpdate::Log(lines)) = chat[0].boot.as_ref().unwrap().updates.first().cloned()
    else {
        panic!("expected the log first");
    };
    assert_eq!(lines.len(), KEPT_LOG_LINES + 1);
    assert_eq!(lines[0].text, "… 10 earlier lines not kept");
    assert_eq!(lines[1].text, "line 10");
}

#[test]
fn a_long_file_is_cut_back_to_the_newest_notes() {
    let dir = tempfile::tempdir().unwrap();
    let agent = AgentId::generate();
    let notes = ChatNotes::new(dir.path());
    for i in 0..=COMPACT_AFTER {
        notes.record(agent, &UiCommand::ShowWarning(format!("w{i}")));
    }
    let chat = notes.chat(agent);
    assert_eq!(chat.len(), MAX_NOTES);
    assert_eq!(
        chat.last().map(|m| m.content.as_str()),
        Some(format!("warning: w{COMPACT_AFTER}").as_str())
    );
    let lines = std::fs::read_to_string(notes.file(agent)).unwrap();
    assert_eq!(lines.lines().count(), MAX_NOTES);
}

#[test]
fn notes_go_among_the_history_by_time() {
    let merged = interleave(
        vec![at("a", 10), at("b", 20), at("c", 30)],
        vec![at("n1", 5), at("n2", 20), at("n3", 40)],
        5,
    );
    let order: Vec<&str> = merged.iter().map(|m| m.content.as_str()).collect();
    assert_eq!(
        order,
        ["a", "b", "n2", "c", "n3"],
        "newest 5 of n1 a b n2 c n3"
    );
}
