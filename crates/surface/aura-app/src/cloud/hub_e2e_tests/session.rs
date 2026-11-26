//! What moves with a cloud agent between here and its machine: its
//! conversation and the changes saved when the machine slept.

use super::*;

#[tokio::test]
async fn placement_hands_the_node_the_conversation() {
    let p = placed(CloudConfig::defaults()).await;
    append(
        hooks::store(&p.hub).as_ref(),
        1,
        Transaction::user_prompt(p.agent, "read notes.txt"),
    );
    append(
        hooks::store(&p.hub).as_ref(),
        2,
        reply(p.agent, "it lists three tasks"),
    );
    let sessions = Checkpoints::new(p.hub_dir.path());
    let messages = vec![
        Message::user("read notes.txt"),
        Message::assistant("it lists three tasks"),
    ];
    sessions
        .save(
            p.agent,
            &SessionCheckpoint::new(
                1_000,
                2,
                messages.clone(),
                Vec::new(),
                SessionUsage::default(),
            ),
        )
        .await
        .expect("saved");
    p.hub.attach_sessions(sessions);

    p.import().await;

    let node = hooks::node(&p.hub, &p.agent.to_hex()).expect("node");
    assert_eq!(node.head(p.agent).await.expect("head"), 2);
    let taken = Checkpoints::new(p.node_dir.path())
        .load_current(p.agent)
        .expect("the node holds the session");
    assert_eq!((taken.started_ms, taken.record_seq), (1_000, 2));
    assert_eq!(texts(&taken.messages), texts(&messages));
}

#[tokio::test]
async fn bringing_an_agent_back_keeps_what_it_said_on_its_machine() {
    let p = placed(CloudConfig::defaults()).await;
    p.import().await;
    append(
        p.node.store.as_ref(),
        1,
        Transaction::user_prompt(p.agent, "read notes.txt"),
    );
    append(
        p.node.store.as_ref(),
        2,
        reply(p.agent, "it lists three tasks"),
    );

    p.hub
        .recall(p.agent, "test", &BootReporter::none())
        .await
        .expect("brought back");

    assert_eq!(p.local_head(), 2);
    assert!(!p.hub.is_placed(p.agent));
    assert!(p.fake.machine(MACHINE).is_none(), "its machine is gone");
    let history = aura_agent::history::model_history(hooks::store(&p.hub).as_ref(), p.agent)
        .expect("history");
    assert_eq!(
        texts(&history),
        [
            (Role::User, "read notes.txt".to_string()),
            (Role::Assistant, "it lists three tasks".to_string()),
        ]
    );
}

#[tokio::test]
async fn the_session_is_kept_here_before_sleep_unless_it_is_ahead_of_the_record() {
    let p = placed(CloudConfig::defaults()).await;
    let sessions = Checkpoints::new(p.hub_dir.path());
    p.hub.attach_sessions(sessions.clone());
    p.write_on_node(1, 2);
    p.save_on_node(3, vec![Message::user("p1")]).await;

    hooks::copy_before_sleep(&p.hub, p.agent).await;

    assert_eq!(p.local_head(), 2);
    assert!(
        sessions.load_current(p.agent).is_none(),
        "it counts entries the record here doesn't hold"
    );

    p.save_on_node(2, vec![Message::user("p1"), Message::user("p2")])
        .await;
    p.hub.sleep(p.agent).await.expect("slept");

    let kept = sessions
        .load_current(p.agent)
        .expect("kept before the machine slept");
    assert_eq!((kept.record_seq, kept.messages.len()), (2, 2));
    let status = p.hub.sync_status(p.agent).expect("placed");
    assert_eq!(status.copied_seq, 2);
    assert!(status.session_ms.is_some());
    assert!(!status.behind());
}

/// The conversation of a turn that read `notes.txt` with a tool call.
fn read_notes_with_a_tool() -> Vec<Message> {
    vec![
        Message::user("read notes.txt"),
        Message::new(
            Role::Assistant,
            vec![ContentBlock::tool_use(
                "t1",
                "read_file",
                serde_json::json!({ "path": "notes.txt" }),
            )],
        ),
        Message::tool_results(vec![(
            "t1".into(),
            ToolResultContent::Text("three tasks".into()),
            false,
        )]),
        Message::assistant("it lists three tasks"),
    ]
}

#[tokio::test]
async fn after_bring_back_the_session_resumes_with_its_tool_calls() {
    let p = placed(CloudConfig::defaults()).await;
    let sessions = Checkpoints::new(p.hub_dir.path());
    p.hub.attach_sessions(sessions.clone());
    p.import().await;
    append(
        p.node.store.as_ref(),
        1,
        Transaction::user_prompt(p.agent, "read notes.txt"),
    );
    append(
        p.node.store.as_ref(),
        2,
        reply(p.agent, "it lists three tasks"),
    );
    p.save_on_node(2, read_notes_with_a_tool()).await;
    append(
        p.node.store.as_ref(),
        3,
        Transaction::user_prompt(p.agent, "and the first?"),
    );

    p.hub
        .recall(p.agent, "test", &BootReporter::none())
        .await
        .expect("brought back");

    let resumed = aura_agent::history::restore(hooks::store(&p.hub).as_ref(), &sessions, p.agent)
        .expect("resumes from the copied session");
    assert!(resumed.messages[1].has_tool_use(), "with its tool call");
    assert!(
        texts(&resumed.messages).contains(&(Role::User, "and the first?".to_string())),
        "and what the record holds past it"
    );
}

#[tokio::test]
async fn bring_back_uses_the_changes_saved_at_sleep_when_the_machine_is_gone() {
    let p = placed(CloudConfig::defaults()).await;
    let project = p.hub_dir.path().join("project");
    let workspace = p.node_dir.path().join("ws");
    repo(&project, "one\n");
    repo(&workspace, "one\n");
    std::fs::write(workspace.join("notes.txt"), "one\ntwo\n").expect("changed");

    p.hub.sleep(p.agent).await.expect("slept");
    let hex = p.agent.to_hex();
    assert!(
        hooks::has_saved_patch(&p.hub, &hex),
        "saved before it slept"
    );
    p.hub
        .fleet()
        .destroy(&hex, "lost")
        .await
        .expect("destroyed");
    assert!(p.hub.fleet().view(&hex).is_none());

    let outcome = p
        .hub
        .recall(p.agent, "test", &BootReporter::none())
        .await
        .expect("brought back");

    assert!(
        matches!(&outcome.patch, PatchOutcome::Applied { files } if files == &["notes.txt"]),
        "{:?}",
        outcome.patch
    );
    assert_eq!(
        std::fs::read_to_string(project.join("notes.txt")).expect("notes"),
        "one\ntwo\n"
    );
    assert!(!p.hub.is_placed(p.agent));
    assert!(!hooks::has_saved_patch(&p.hub, &hex), "dropped once used");
}

#[tokio::test]
async fn a_pull_drops_the_changes_saved_at_sleep() {
    let p = placed(CloudConfig::defaults()).await;
    let project = p.hub_dir.path().join("project");
    let workspace = p.node_dir.path().join("ws");
    repo(&project, "one\n");
    repo(&workspace, "one\n");
    std::fs::write(workspace.join("notes.txt"), "one\ntwo\n").expect("changed");
    p.hub.sleep(p.agent).await.expect("slept");
    let hex = p.agent.to_hex();
    assert!(
        hooks::has_saved_patch(&p.hub, &hex),
        "saved before it slept"
    );

    let outcome = p.hub.pull(p.agent).await.expect("pulled");

    assert!(
        matches!(&outcome.patch, PatchOutcome::Applied { .. }),
        "{:?}",
        outcome.patch
    );
    assert!(!hooks::has_saved_patch(&p.hub, &hex), "in the project now");
    let sync = p.hub.sync_status(p.agent).expect("placed");
    assert!(sync.patch_ms.is_none(), "the next sleep saves a new one");
}
