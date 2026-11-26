use super::support::*;
use super::*;

#[tokio::test]
async fn unknown_permanent_agent_is_rejected() {
    let fixture = fixture("unused");
    let parent = lock(&fixture.hub.registry).active().agent_id();

    let result = fixture
        .dispatcher
        .dispatch(request(parent, "nobody", "hi"))
        .await
        .unwrap();

    assert!(
        matches!(&result.exit, SubagentExit::Rejected { reason } if reason.contains("explorer, frontend-engineer"))
    );
    assert_eq!(lock(&fixture.hub.registry).agents().len(), 1);
}

#[tokio::test]
async fn spawn_is_recorded_as_a_run_linked_to_its_parent() {
    let fixture = fixture("Looks good.");
    let parent = lock(&fixture.hub.registry).active().agent_id();
    let parent_run = fixture
        .hub
        .traces
        .start(parent, RunMeta::new("mock-model", RunSource::Tui))
        .unwrap()
        .run_id();

    let result = fixture
        .dispatcher
        .dispatch(request(parent, "reviewer", "Review the diff"))
        .await
        .unwrap();

    let child = result.child_agent_id.unwrap();
    let trace = aura_agent_trace::db::latest_of(&fixture.trace_store, &child.to_hex())
        .unwrap()
        .expect("the spawn's run is in the database");
    assert_eq!(trace.meta.spawned_by.as_deref(), Some(parent_run.as_str()));
    assert_eq!(trace.meta.title, "Review the diff");
    assert_eq!(trace.meta.mode, "reviewer");
    assert!(trace.scorecard().turns >= 1);
}

#[tokio::test]
async fn spawn_runs_an_instance_under_its_parent() {
    let mut fixture = fixture("Looks good.");
    let parent = lock(&fixture.hub.registry).active().clone();

    let result = fixture
        .dispatcher
        .dispatch(request(parent.agent_id(), "reviewer", "Review the diff"))
        .await
        .unwrap();

    assert!(matches!(result.exit, SubagentExit::Completed));
    assert_eq!(result.final_message, "Looks good.");
    let child_id = result.child_agent_id.unwrap();
    {
        let registry = lock(&fixture.hub.registry);
        let child = registry.find(child_id).unwrap();
        assert_eq!(child.name, "reviewer #1");
        assert_eq!(child.definition.as_deref(), Some("reviewer"));
        assert_eq!(child.parent.as_deref(), Some(parent.zns_id.as_str()));
        assert_eq!(registry.active().agent_id(), parent.agent_id());
    }

    let finished = fixture.hub.take_finished();
    assert_eq!(finished.len(), 1);
    assert_eq!(finished[0].agent_id(), child_id);
    assert_eq!(finished[0].definition.as_deref(), Some("reviewer"));
    assert_eq!(finished[0].messages.len(), 2);
    assert!(fixture.hub.take_finished().is_empty());

    let summaries = fixture.hub.agent_summaries();
    let child_hex = child_id.to_hex();
    let child_row = summaries.iter().find(|s| s.id == child_hex).unwrap();
    assert_eq!(child_row.depth, 1);
    assert!(!child_row.running);
    assert_child_ran_and_chatted(&mut fixture, child_id);
}

/// The agents list showed `child_id` running, and its chat got the
/// instructions, the prompt and the end of the turn.
fn assert_child_ran_and_chatted(fixture: &mut Fixture, child_id: AgentId) {
    let child_hex = child_id.to_hex();
    let mut saw_running = false;
    let mut child_chat = Vec::new();
    while let Ok(command) = fixture.commands.try_recv() {
        match command {
            UiCommand::SetAgents(rows) => {
                saw_running |= rows.iter().any(|r| r.id == child_hex && r.running);
            }
            UiCommand::ForAgent { agent_id, command } if agent_id == child_id.to_hex() => {
                child_chat.push(*command);
            }
            _ => {}
        }
    }
    assert!(saw_running);
    assert!(matches!(
        child_chat.first(),
        Some(UiCommand::SetInstructions(_))
    ));
    assert!(matches!(
        child_chat.get(1),
        Some(UiCommand::TurnStarted { prompt: Some(p) }) if p == "Review the diff"
    ));
    assert!(matches!(child_chat.last(), Some(UiCommand::Complete)));
}

/// The row `SetAgents` lists for `child` once it has a subtitle, waiting
/// for the background job that names it.

#[tokio::test]
async fn cancelling_a_spawn_fails_its_task() {
    let fixture = slow_fixture("unused", 60_000);
    let parent = lock(&fixture.hub.registry).active().agent_id();
    let hub = Arc::clone(&fixture.hub);

    let dispatch = tokio::spawn({
        let dispatcher = Arc::clone(&fixture.dispatcher);
        async move {
            dispatcher
                .dispatch(request(parent, "reviewer", "Review it"))
                .await
                .unwrap()
        }
    });
    let child = loop {
        let running = lock(&hub.running).keys().next().copied();
        if let Some(child) = running {
            break child;
        }
        tokio::task::yield_now().await;
    };
    assert!(hub.cancel(child));

    let result = tokio::time::timeout(std::time::Duration::from_secs(10), dispatch)
        .await
        .expect("cancel stops the spawn")
        .unwrap();
    assert!(
        matches!(&result.exit, SubagentExit::Failed { reason } if reason == "cancelled by the user")
    );
    assert!(!hub.is_running(child));
    assert_eq!(hub.take_finished().len(), 1);
}

#[tokio::test]
async fn a_writer_waiting_for_the_workspace_tells_its_chat_it_is_queued() {
    let mut fixture = slow_fixture("unused", 60_000);
    let parent = lock(&fixture.hub.registry).active().agent_id();
    let hub = Arc::clone(&fixture.hub);
    for prompt in ["first edit", "second edit"] {
        let dispatcher = Arc::clone(&fixture.dispatcher);
        let asked = request(parent, "engineer", prompt);
        tokio::spawn(async move { dispatcher.dispatch(asked).await });
    }

    let mut queued = false;
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    while !queued && std::time::Instant::now() < deadline {
        while let Ok(command) = fixture.commands.try_recv() {
            if let UiCommand::ForAgent { command, .. } = command {
                queued |= matches!(
                    *command,
                    UiCommand::ShowMessage(ref m) if m.content.starts_with("Queued:")
                );
            }
        }
        tokio::task::yield_now().await;
    }
    assert!(queued, "the waiting agent's chat says why it is empty");

    let running: Vec<AgentId> = lock(&hub.running).keys().copied().collect();
    for child in running {
        hub.cancel(child);
    }
}

#[tokio::test]
async fn waiting_spawn_is_listed_as_a_finished_task() {
    let fixture = fixture("Looks good.");
    let parent = lock(&fixture.hub.registry).active().agent_id();

    let result = fixture
        .dispatcher
        .dispatch(request(parent, "reviewer", "Review the diff"))
        .await
        .unwrap();

    let rows = fixture.hub.processes.snapshot();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].id, task_id(result.child_agent_id.unwrap()));
    assert_eq!(rows[0].kind, ProcessKind::Task);
    assert_eq!(rows[0].status, ProcessStatus::Exited);
    assert_eq!(rows[0].owner_agent_id, parent.to_hex());
    assert_eq!(rows[0].detail, "Review the diff");
    assert_eq!(
        rows[0].jump_agent_id,
        result.child_agent_id.unwrap().to_hex()
    );
}

#[tokio::test]
async fn detached_spawn_returns_at_once_and_replies_with_a_notice() {
    let fixture = fixture("Looks good.");
    let parent = lock(&fixture.hub.registry).active().agent_id();
    let mut notices = fixture.hub.take_notices();
    let mut detached = request(parent, "reviewer", "Review the diff");
    detached.spawn_mode = Some(SpawnMode::Detached);

    let result = fixture.dispatcher.dispatch(detached).await.unwrap();
    assert!(matches!(result.exit, SubagentExit::Completed));
    assert!(result.final_message.contains("in the background"));
    let child = result.child_agent_id.unwrap();

    let notice = tokio::time::timeout(std::time::Duration::from_secs(10), notices.recv())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(notice.to, parent);
    assert_eq!(notice.shown, "reviewer #1 finished its background task");
    assert!(notice.prompt.ends_with("Its reply:\n\nLooks good."));

    assert!(!fixture.hub.is_running(child));
    let finished = fixture.hub.take_finished();
    assert_eq!(finished.len(), 1);
    assert_eq!(finished[0].agent_id(), child);
    let rows = fixture.hub.processes.snapshot();
    assert_eq!(rows[0].kind, ProcessKind::Agent);
    assert_eq!(rows[0].status, ProcessStatus::Exited);
}

#[tokio::test]
async fn spawned_instance_works_in_its_projects_folder() {
    let fixture = fixture("Done.");
    let folder = TempDir::new().unwrap();
    let (parent, _) = lock(&fixture.hub.registry)
        .open_project(folder.path())
        .unwrap();

    let result = fixture
        .dispatcher
        .dispatch(request(parent.agent_id(), "reviewer", "Review it"))
        .await
        .unwrap();

    assert!(matches!(result.exit, SubagentExit::Completed));
    let finished = fixture.hub.take_finished();
    assert_eq!(
        finished[0].kernel.agent_workspace(),
        folder.path().canonicalize().unwrap()
    );
}
