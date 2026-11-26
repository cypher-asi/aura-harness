use super::support::*;
use super::*;

fn instance(fixture: &Fixture, definition: &str) -> AgentSession {
    let agent = lock(&fixture.hub.registry)
        .create_instance(definition, None)
        .unwrap();
    fixture.hub.factory.open(&agent, Vec::new(), None).unwrap()
}

#[tokio::test]
async fn unstarted_spawns_are_archived_but_started_ones_are_left_alone() {
    let fixture = fixture("Looks good.");
    let parent = lock(&fixture.hub.registry).active().agent_id();
    let parent_zns = lock(&fixture.hub.registry)
        .find(parent)
        .unwrap()
        .zns_id
        .clone();

    // Never opened: the app closed before a `task` call got anywhere.
    let unstarted = lock(&fixture.hub.registry)
        .create_instance("reviewer", Some(parent_zns.clone()))
        .unwrap();
    // Opened and ran a turn: there is a record to show and resume.
    let started = lock(&fixture.hub.registry)
        .create_instance("reviewer", Some(parent_zns))
        .unwrap();
    let session = fixture
        .hub
        .factory
        .open(&started, Vec::new(), None)
        .unwrap();
    crate::event_loop::handlers::mark_session_start(&session)
        .await
        .unwrap();

    fixture.hub.archive_unstarted_spawns();

    let registry = lock(&fixture.hub.registry);
    assert!(registry.find(unstarted.agent_id()).unwrap().archived);
    assert_eq!(
        registry
            .find(unstarted.agent_id())
            .unwrap()
            .title
            .as_deref(),
        Some("Did not start before the app closed")
    );
    assert!(!registry.find(started.agent_id()).unwrap().archived);
    // A project the user created, not a `task` spawn, is never touched
    // even though it has no record yet.
    assert!(!registry.find(parent).unwrap().archived);
}

fn attempt(session: &AgentSession, kind: &str, prompt: &str) -> Attempt {
    Attempt {
        kind: kind.to_string(),
        prompt: prompt.to_string(),
        child: session.agent_id(),
        kernel: Arc::clone(&session.kernel),
        ended: None,
        redone: false,
    }
}

fn unavailable_outcome() -> OutcomePayload {
    OutcomePayload {
        prompt_tx: Hash::from_content(b"prompt"),
        category: PromptCategory::Research,
        model: "m".to_string(),
        success: None,
        signal: OutcomeSignal::Unavailable,
        cost_usd: 0.0,
        subtask_cost_usd: 0.0,
        latency_ms: 1,
    }
}

fn recorded_outcomes(fixture: &Fixture, agent: AgentId) -> Vec<OutcomePayload> {
    fixture
        .hub
        .factory
        .store
        .scan_record(agent, 1, 100)
        .unwrap()
        .iter()
        .filter_map(|e| e.tx.outcome_payload())
        .collect()
}

#[tokio::test]
async fn a_subtask_redone_while_still_running_fails_once_it_ends() {
    let fixture = fixture("unused");
    let parent = lock(&fixture.hub.registry).active().agent_id();
    let parent_session = fixture
        .hub
        .factory
        .open(
            &lock(&fixture.hub.registry).active().clone(),
            Vec::new(),
            None,
        )
        .unwrap();
    let (first, second) = (
        instance(&fixture, "explorer"),
        instance(&fixture, "explorer"),
    );
    let prompt = "Find the config loader";
    assert!(fixture
        .hub
        .begin_attempt(parent, attempt(&first, "explorer", prompt))
        .is_empty());
    fixture.hub.start_running(
        parent,
        Arc::clone(&parent_session.kernel),
        CancellationToken::new(),
        None,
    );
    assert!(fixture
        .hub
        .begin_attempt(parent, attempt(&first, "explorer", prompt))
        .is_empty());
    assert!(fixture
        .hub
        .begin_attempt(parent, attempt(&second, "explorer", prompt))
        .is_empty());

    let ended = unavailable_outcome();
    let redo = fixture
        .hub
        .end_attempt(parent, first.agent_id(), Some(ended.clone()))
        .expect("the first attempt was redone while it ran");
    redo.record().await;
    assert!(fixture
        .hub
        .end_attempt(parent, second.agent_id(), Some(ended))
        .is_none());
    let outcomes = recorded_outcomes(&fixture, first.agent_id());
    assert_eq!(outcomes.len(), 1);
    assert_eq!(outcomes[0].signal, OutcomeSignal::Redone);
    assert_eq!(outcomes[0].success, Some(false));
}

/// The status of the Processes tab row of `child`'s task, once it has one.
fn task_row_status(hub: &AgentHub, child: AgentId) -> Option<ProcessStatus> {
    let id = task_id(child);
    hub.processes
        .snapshot()
        .into_iter()
        .find(|row| row.id == id)
        .map(|row| row.status)
}

#[tokio::test]
async fn dropping_a_waiting_spawn_mid_run_stops_it_running() {
    let fixture = slow_fixture("unused", 60_000);
    let parent = lock(&fixture.hub.registry).active().agent_id();
    let hub = Arc::clone(&fixture.hub);

    // Stands in for the parent's tool call, which cancelling the parent's
    // turn drops without cancelling the spawn's own token.
    let dispatch = tokio::spawn({
        let dispatcher = Arc::clone(&fixture.dispatcher);
        async move {
            dispatcher
                .dispatch(request(parent, "reviewer", "Review it"))
                .await
        }
    });
    let child = loop {
        let running = lock(&hub.running).keys().next().copied();
        if let Some(child) = running.filter(|child| task_row_status(&hub, *child).is_some()) {
            break child;
        }
        tokio::task::yield_now().await;
    };
    dispatch.abort();
    let joined = dispatch.await;
    assert!(joined.is_err_and(|e| e.is_cancelled()));

    assert!(!hub.is_running(child));
    assert_eq!(hub.take_abandoned(), [child]);
    assert!(hub.take_finished().is_empty(), "its session went with it");
    assert_eq!(task_row_status(&hub, child), Some(ProcessStatus::Killed));
}

#[tokio::test]
async fn a_spawn_that_ends_normally_is_cleaned_up_once() {
    let fixture = fixture("Looks good.");
    let parent = lock(&fixture.hub.registry).active().agent_id();

    let result = fixture
        .dispatcher
        .dispatch(request(parent, "reviewer", "Review the diff"))
        .await
        .unwrap();

    let child = result.child_agent_id.unwrap();
    assert!(!fixture.hub.is_running(child));
    assert_eq!(fixture.hub.take_finished().len(), 1);
    assert!(
        fixture.hub.take_abandoned().is_empty(),
        "the disarmed guard did nothing on drop"
    );
    assert_eq!(
        task_row_status(&fixture.hub, child),
        Some(ProcessStatus::Exited)
    );
}

#[tokio::test]
async fn an_armed_guard_ends_only_the_running_it_started() {
    let fixture = fixture("unused");
    let session = instance(&fixture, "explorer");
    let agent = session.agent_id();
    let kernel = || Arc::clone(&session.kernel);

    let guard = fixture
        .hub
        .start_guarded(agent, kernel(), CancellationToken::new());
    drop(guard);
    assert!(!fixture.hub.is_running(agent));
    assert_eq!(fixture.hub.take_abandoned(), [agent]);

    // A stale guard whose agent started running again since.
    let stale = fixture
        .hub
        .start_guarded(agent, kernel(), CancellationToken::new());
    fixture
        .hub
        .start_running(agent, kernel(), CancellationToken::new(), None);
    drop(stale);
    assert!(fixture.hub.is_running(agent), "the newer turn survives");
    assert!(fixture.hub.take_abandoned().is_empty());
    fixture.hub.stop_running(agent);

    // A disarmed guard leaves ending the running to its owner.
    let disarmed = fixture
        .hub
        .start_guarded(agent, kernel(), CancellationToken::new());
    disarmed.disarm();
    assert!(fixture.hub.is_running(agent));
    assert!(fixture.hub.take_abandoned().is_empty());
}

#[tokio::test]
async fn only_a_typed_prompt_to_an_agent_nobody_named_titles_its_chat() {
    let fixture = fixture("done");
    let unnamed = lock(&fixture.hub.registry).active().agent_id();
    let named = lock(&fixture.hub.registry)
        .create(Some("Researcher".to_string()))
        .unwrap()
        .agent_id();

    assert!(fixture.hub.titles_turn(unnamed, true));
    assert!(
        !fixture.hub.titles_turn(unnamed, false),
        "a notice or plan build"
    );
    assert!(!fixture.hub.titles_turn(named, true));

    assert!(!fixture.hub.apply_title(named, "Build fix").await);
    assert_eq!(lock(&fixture.hub.registry).find(named).unwrap().title, None);
    assert!(fixture.hub.apply_title(unnamed, "Build fix").await);
    assert!(!fixture.hub.titles_turn(unnamed, true), "titled once");
    assert!(!fixture.hub.apply_title(unnamed, "Other").await);
}
