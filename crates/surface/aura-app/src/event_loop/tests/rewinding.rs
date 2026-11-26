//! Rewinding turns and diffs.

use super::*;

/// Run turn "one" (edits notes.txt) and turn "two" (edits notes.txt and
/// adds added.txt).
async fn two_checkpointed_turns(fixture: &Fixture, state: &mut LoopState<'_>, first: AgentId) {
    checkpointed_turn(
        fixture,
        state,
        first,
        "one",
        &[("notes.txt", Some("one\n"))],
    )
    .await;
    checkpointed_turn(
        fixture,
        state,
        first,
        "two",
        &[("notes.txt", Some("two\n")), ("added.txt", Some("new\n"))],
    )
    .await;
}

fn head_seq(state: &LoopState<'_>, agent: AgentId) -> u64 {
    state.idle[&agent]
        .kernel
        .store()
        .get_head_seq(agent)
        .unwrap()
}

#[tokio::test]
async fn rewind_restores_files_and_the_conversation() {
    let (fixture, mut commands) = fixture();
    let mut b = borrowed(&fixture);
    let first = fixture.first;
    let mut state = loop_state(&fixture, &mut b);
    let notes = fixture.workspace.join("notes.txt");
    std::fs::write(&notes, "original\n").unwrap();

    two_checkpointed_turns(&fixture, &mut state, first).await;
    let head_before = head_seq(&state, first);

    rewind::handle_rewind(&mut state, first, None, RewindScope::Both).await;
    let listed = system_texts(&drain(&mut commands));
    assert!(
        listed.contains("2. ") && listed.contains("two · 2 files"),
        "{listed}"
    );
    assert!(
        listed.contains("1. ") && listed.contains("one · 1 file"),
        "{listed}"
    );
    assert!(listed.contains("not undone"), "{listed}");

    rewind::handle_rewind(&mut state, first, Some(2), RewindScope::Both).await;
    assert_eq!(std::fs::read_to_string(&notes).unwrap(), "one\n");
    assert!(!fixture.workspace.join("added.txt").exists());
    assert_eq!(user_texts(&state, first), ["one"]);
    assert_eq!(state.idle[&first].messages.len(), 2);

    let commands_after = drain(&mut commands);
    assert!(commands_after
        .iter()
        .any(|(_, c)| matches!(c, UiCommand::LoadConversation(chat) if chat.len() == 2)));
    let said = system_texts(&commands_after);
    assert!(
        said.contains("Rewound to before turn 2: restored 2 files and the conversation"),
        "{said}"
    );
    let head_after = head_seq(&state, first);
    assert!(head_after > head_before, "the rewind is recorded");

    let reopened = open_again(&fixture, first);
    assert_eq!(reopened.messages.len(), 2, "the checkpoint holds the cut");
    assert_eq!(fixture.files.turns(first, reopened.started_ms).len(), 1);
}

#[tokio::test]
async fn rewinding_only_code_keeps_the_conversation() {
    let (fixture, _commands) = fixture();
    let mut b = borrowed(&fixture);
    let first = fixture.first;
    let mut state = loop_state(&fixture, &mut b);
    let file = fixture.workspace.join("a.txt");
    std::fs::write(&file, "before\n").unwrap();

    checkpointed_turn(&fixture, &mut state, first, "edit", &[("a.txt", None)]).await;
    rewind::handle_rewind(&mut state, first, Some(1), RewindScope::Code).await;

    assert_eq!(std::fs::read_to_string(&file).unwrap(), "before\n");
    assert_eq!(state.idle[&first].messages.len(), 2);
    assert_eq!(
        fixture
            .files
            .turns(first, state.idle[&first].started_ms)
            .len(),
        1
    );
}

#[tokio::test]
async fn a_summarized_turn_cannot_be_rewound_in_the_chat() {
    let (fixture, mut commands) = fixture();
    let mut b = borrowed(&fixture);
    let first = fixture.first;
    let mut state = loop_state(&fixture, &mut b);
    let file = fixture.workspace.join("a.txt");
    std::fs::write(&file, "before\n").unwrap();

    checkpointed_turn(
        &fixture,
        &mut state,
        first,
        "edit",
        &[("a.txt", Some("x\n"))],
    )
    .await;
    state.idle.get_mut(&first).unwrap().messages =
        vec![aura_model::Message::user("summary of earlier work")];
    rewind::handle_rewind(&mut state, first, Some(1), RewindScope::Both).await;

    let said = system_texts(&drain(&mut commands));
    assert!(said.contains("/rewind 1 code"), "{said}");
    assert_eq!(
        std::fs::read_to_string(&file).unwrap(),
        "x\n",
        "files stay when the chat can't follow"
    );
}

#[tokio::test]
async fn rewinding_a_busy_agent_waits() {
    let (fixture, mut commands) = fixture();
    let mut b = borrowed(&fixture);
    let first = fixture.first;
    let mut state = loop_state(&fixture, &mut b);
    let session = state.idle.remove(&first).unwrap();

    rewind::handle_rewind(&mut state, first, Some(1), RewindScope::Both).await;
    let said = system_texts(&drain(&mut commands));
    assert!(said.contains("Wait for this agent"), "{said}");
    state.idle.insert(first, session);
}

#[tokio::test]
async fn diff_shows_the_latest_turn_or_the_whole_session() {
    let (fixture, mut commands) = fixture();
    let mut b = borrowed(&fixture);
    let first = fixture.first;
    let mut state = loop_state(&fixture, &mut b);
    std::fs::write(fixture.workspace.join("a.txt"), "a\n").unwrap();

    rewind::handle_diff(&state, first, false).await;
    assert!(system_texts(&drain(&mut commands)).contains("No turns"));

    checkpointed_turn(
        &fixture,
        &mut state,
        first,
        "one",
        &[("a.txt", Some("A\n"))],
    )
    .await;
    checkpointed_turn(
        &fixture,
        &mut state,
        first,
        "two",
        &[("b.txt", Some("b\n"))],
    )
    .await;

    let shown = |commands: Vec<(Option<String>, UiCommand)>| {
        commands.into_iter().find_map(|(_, c)| match c {
            UiCommand::ShowDiff { title, files } => Some((title, files)),
            _ => None,
        })
    };
    rewind::handle_diff(&state, first, false).await;
    let (title, files) = shown(drain(&mut commands)).expect("a diff");
    assert_eq!(title, "Files changed by turn 2 · two");
    assert_eq!(files.len(), 1);
    assert_eq!(files[0].path, "b.txt");
    assert!(files[0].diff.contains("+b"), "{}", files[0].diff);

    rewind::handle_diff(&state, first, true).await;
    let (_, files) = shown(drain(&mut commands)).expect("a diff");
    let paths: Vec<&str> = files.iter().map(|f| f.path.as_str()).collect();
    assert_eq!(paths, ["a.txt", "b.txt"]);
    assert!(files[0].diff.contains("-a\n+A\n"), "{}", files[0].diff);
}
