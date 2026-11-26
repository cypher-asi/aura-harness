use super::*;

fn setup() -> (tempfile::TempDir, FileCheckpoints, AgentId, PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    let files = FileCheckpoints::new(&dir.path().join("data"));
    let workspace = dir.path().join("ws");
    std::fs::create_dir_all(&workspace).unwrap();
    (dir, files, AgentId::generate(), workspace)
}

/// Change `path` to `content` (`None` deletes it) as a tool would.
async fn change(hook: &Arc<dyn FileChangeHook>, path: &Path, content: Option<&str>) {
    hook.before_change(path, Some("t1")).await;
    match content {
        Some(text) => std::fs::write(path, text).unwrap(),
        None => std::fs::remove_file(path).unwrap(),
    }
    hook.after_change(path, Some("t1"), true).await;
}

#[tokio::test]
async fn rewind_puts_files_back_as_they_were_before_the_turn() {
    let (_dir, files, agent, ws) = setup();
    let hook = files.hook(vec![agent], None);
    let kept = ws.join("kept.txt");
    let created = ws.join("new/created.txt");
    std::fs::write(&kept, "original\n").unwrap();

    files.begin_turn(agent, 1, 0, "first");
    change(&hook, &kept, Some("turn one\n")).await;
    files.end_turn(agent);
    files.begin_turn(agent, 1, 2, "second");
    change(&hook, &kept, Some("turn two\n")).await;
    std::fs::create_dir_all(created.parent().unwrap()).unwrap();
    change(&hook, &created, Some("hello\n")).await;
    files.end_turn(agent);

    let turns = files.turns(agent, 1);
    assert_eq!(turns.len(), 2);
    assert_eq!(turns[1].manifest.files.len(), 2);
    assert_eq!(
        turns[1].manifest.files[1].blob, None,
        "created file was absent"
    );

    let report = files.restore(agent, &turns[1..]);
    assert!(report.failed.is_empty(), "{report:?}");
    assert_eq!(std::fs::read_to_string(&kept).unwrap(), "turn one\n");
    assert!(!created.exists());

    let report = files.restore(agent, &turns);
    assert!(report.failed.is_empty(), "{report:?}");
    assert_eq!(std::fs::read_to_string(&kept).unwrap(), "original\n");
}

#[tokio::test]
async fn an_attempt_and_its_subagents_changes_can_be_put_back() {
    let (_dir, files, attempt, ws) = setup();
    let nested = AgentId::generate();
    let outsider = files.hook(vec![AgentId::generate()], None);
    let kept = ws.join("kept.txt");
    let created = ws.join("created.txt");
    let other = ws.join("other.txt");
    std::fs::write(&kept, "original\n").unwrap();

    files.begin_attempt(attempt);
    change(&files.hook(vec![attempt], None), &kept, Some("one\n")).await;
    change(
        &files.hook(vec![nested, attempt], None),
        &kept,
        Some("two\n"),
    )
    .await;
    change(
        &files.hook(vec![nested, attempt], None),
        &created,
        Some("new\n"),
    )
    .await;
    change(&outsider, &other, Some("not ours\n")).await;
    let changed = files.take_attempt(attempt);
    assert_eq!(
        changed.paths().collect::<Vec<_>>(),
        [kept.as_path(), created.as_path()]
    );

    let report = changed.restore();
    assert!(report.failed.is_empty(), "{report:?}");
    assert_eq!(std::fs::read_to_string(&kept).unwrap(), "original\n");
    assert!(!created.exists());
    assert!(other.exists(), "files of other work are left alone");
    assert!(files.take_attempt(attempt).is_empty());
}

#[test]
fn a_turn_keeps_the_record_hash_of_its_prompt() {
    let (_dir, files, agent, _ws) = setup();
    let prompt_tx = Hash::from_content(b"prompt");
    files.note_prompt_tx(agent, prompt_tx);
    files.begin_turn(agent, 1, 0, "first");
    files.note_prompt_tx(agent, prompt_tx);
    files.end_turn(agent);
    files.note_prompt_tx(agent, Hash::from_content(b"later"));

    let turns = files.turns(agent, 1);
    assert_eq!(turns.len(), 1);
    assert_eq!(turns[0].manifest.prompt_tx, Some(prompt_tx.to_hex()));
}

#[tokio::test]
async fn only_the_first_change_in_a_turn_is_saved_and_deletes_come_back() {
    let (_dir, files, agent, ws) = setup();
    let hook = files.hook(vec![agent], None);
    let doomed = ws.join("doomed.txt");
    std::fs::write(&doomed, "keep me\n").unwrap();

    files.begin_turn(agent, 1, 0, "clean up");
    change(&hook, &doomed, Some("edited\n")).await;
    change(&hook, &doomed, None).await;
    files.end_turn(agent);

    let turns = files.turns(agent, 1);
    assert_eq!(turns[0].manifest.files.len(), 1);
    let report = files.restore(agent, &turns);
    assert_eq!(report.restored, std::slice::from_ref(&doomed));
    assert_eq!(std::fs::read_to_string(&doomed).unwrap(), "keep me\n");
}

#[tokio::test]
async fn changes_outside_a_turn_go_to_the_spawning_agents_turn_or_nowhere() {
    let (_dir, files, parent, ws) = setup();
    let child = AgentId::generate();
    let hook = files.hook(vec![child, parent], None);
    let path = ws.join("a.txt");

    change(&hook, &path, Some("no turn\n")).await;
    assert!(files.turns(parent, 1).is_empty());

    files.begin_turn(parent, 1, 0, "delegate");
    change(&hook, &path, Some("by the child\n")).await;
    files.end_turn(parent);
    let turns = files.turns(parent, 1);
    assert_eq!(turns[0].manifest.files.len(), 1);
    assert!(files.turns(child, 1).is_empty());
}

#[tokio::test]
async fn diff_compares_now_with_before_the_turns() {
    let (_dir, files, agent, ws) = setup();
    let hook = files.hook(vec![agent], None);
    let a = ws.join("a.txt");
    let b = ws.join("b.txt");
    std::fs::write(&a, "one\ntwo\n").unwrap();
    std::fs::write(&b, "same\n").unwrap();

    files.begin_turn(agent, 1, 0, "edit");
    change(&hook, &a, Some("one\nTWO\n")).await;
    change(&hook, &b, Some("changed\n")).await;
    std::fs::write(&b, "same\n").unwrap();
    files.end_turn(agent);

    let manifests = files.session_manifests(agent, 1);
    let diffs = files.diff(agent, &manifests, Some(&ws));
    assert_eq!(diffs.len(), 1, "b.txt is back as it was: {diffs:?}");
    assert_eq!(diffs[0].path, "a.txt");
    assert!(diffs[0].diff.contains("-two\n+TWO\n"), "{}", diffs[0].diff);
}

#[tokio::test]
async fn rewound_turns_drop_out_of_the_list_but_not_the_session() {
    let (_dir, files, agent, _ws) = setup();
    files.begin_turn(agent, 1, 0, "one");
    files.end_turn(agent);
    files.begin_turn(agent, 1, 2, "two\nmore detail");
    files.end_turn(agent);
    files.begin_turn(agent, 9, 0, "other session");
    files.end_turn(agent);

    let turns = files.turns(agent, 1);
    assert_eq!(turns.len(), 2);
    assert_eq!(turns[1].manifest.prompt, "two…");
    assert!(turns[1].manifest.is_prompt("two\nmore detail"));

    files.mark_rewound(agent, &turns[1..]);
    assert_eq!(files.turns(agent, 1).len(), 1);
    assert_eq!(files.session_manifests(agent, 1).len(), 2);
}

#[tokio::test]
async fn the_hook_shows_each_change_in_its_tool_block() {
    let (_dir, files, agent, ws) = setup();
    let (tx, mut rx) = mpsc::channel(8);
    let hook = files.hook(vec![agent], Some(tx));
    let path = ws.join("x.txt");
    std::fs::write(&path, "a\n").unwrap();

    change(&hook, &path, Some("b\n")).await;

    let Some(UiCommand::ForAgent { agent_id, command }) = rx.recv().await else {
        panic!("expected a command for the agent's chat");
    };
    assert_eq!(agent_id, agent.to_hex());
    let UiCommand::ToolDiff { id, diff } = *command else {
        panic!("expected a tool diff");
    };
    assert_eq!(id, "t1");
    assert!(diff.contains("-a\n+b\n"), "{diff}");
}
