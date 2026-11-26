//! Background commands, re-routed turns and cancelling.

use super::*;

/// A table whose process manager runs `true` as a background command of
/// `agent_id`, and the completion it reported.
async fn finished_command(
    agent_id: AgentId,
    stopped_by_owner: bool,
) -> (Arc<ProcessTable>, aura_core_types::ProcessId, Transaction) {
    let (tx, mut completions) = mpsc::channel(4);
    let manager = Arc::new(aura_agent::ProcessManager::new(
        tx,
        aura_agent::ProcessManagerConfig::default(),
    ));
    let table = ProcessTable::without_os_scan(None, Some(Arc::clone(&manager)));
    let process_id = aura_core_types::ProcessId::generate();
    manager.register_with(
        agent_id,
        aura_core_types::Hash::from_content(b"proposal"),
        aura_core_types::ActionId::generate(),
        process_id,
        std::process::Command::new("true").spawn().unwrap(),
        "true".to_string(),
        aura_agent::ProcessMeta::default(),
    );
    let info = manager
        .snapshot()
        .into_iter()
        .find(|info| info.process_id == process_id)
        .unwrap();
    table.command_started(&info);
    if stopped_by_owner {
        table.stopped_by_owner(process_id);
    }
    let completion = tokio::time::timeout(Duration::from_secs(10), completions.recv())
        .await
        .unwrap()
        .unwrap();
    (table, process_id, completion)
}

#[tokio::test]
async fn finished_command_starts_a_turn_for_an_idle_agent() {
    let (fixture, _commands) = fixture();
    let mut b = borrowed(&fixture);
    let first = fixture.first;
    let mut done = std::mem::replace(&mut b.done, mpsc::channel(1).1);
    let mut state = loop_state(&fixture, &mut b);
    let (table, process_id, completion) = finished_command(first, false).await;
    state.processes = table;

    handle_completion(&mut state, completion).await;
    assert!(state.hub.is_running(first));

    let finished = next_done(&mut done).await;
    handle_work_done(&mut state, finished).await;
    let prompts = user_texts(&state, first);
    assert_eq!(prompts.len(), 1);
    assert!(prompts[0].starts_with(SteerSource::Harness.prefix()));
    assert!(
        prompts[0].contains("`true`") && prompts[0].contains(&process_id.to_hex()),
        "{prompts:?}"
    );
}

#[tokio::test]
async fn a_command_its_agent_stopped_needs_no_reply() {
    let (fixture, _commands) = fixture();
    let mut b = borrowed(&fixture);
    let first = fixture.first;
    let mut state = loop_state(&fixture, &mut b);
    let (table, _, completion) = finished_command(first, true).await;
    state.processes = table;

    handle_completion(&mut state, completion).await;

    assert!(!state.hub.is_running(first));
    assert!(state.queued.is_empty());
}

#[tokio::test]
async fn gate_re_routes_a_turn_whose_route_refuses_the_sign_in() {
    let (fixture, mut commands) = fixture();
    let mut b = borrowed(&fixture);
    let links = b.links.clone();
    let first = fixture.first;
    let mut done = std::mem::replace(&mut b.done, mpsc::channel(1).1);
    let mut state = loop_state(&fixture, &mut b);
    state.models = ModelSelection::signed_in(
        &fixture.config,
        aura_model::model_catalog::GATE_MODEL_ID,
        aura_auth::AuthSummary {
            subscriptions: vec![aura_auth::ProviderKind::Claude],
            ..aura_auth::AuthSummary::default()
        },
    );
    assert_eq!(state.models.turn_model(), REFUSING_MODEL);

    handle_ui_event(&mut state, message(first, "hello", true), &links).await;
    let finished = next_done(&mut done).await;
    assert_eq!(
        finished.failure, None,
        "a refused route is not a failed turn"
    );
    assert_eq!(finished.refused_models, [REFUSING_MODEL]);
    handle_work_done(&mut state, finished).await;

    assert_eq!(
        state.models.turn_model(),
        "claude/claude-opus-4-7",
        "the refused model stays out of the next turns"
    );
    let replies: Vec<String> = state.idle[&first]
        .messages
        .iter()
        .map(|m| m.text_content())
        .collect();
    assert_eq!(replies, ["hello", "Done."]);
    let shown: Vec<String> = drain(&mut commands)
        .into_iter()
        .filter_map(|(_, c)| match c {
            UiCommand::ShowMessage(m) if m.content.contains("gate → ") => Some(m.content),
            _ => None,
        })
        .collect();
    assert_eq!(shown.len(), 2, "{shown:?}");
    assert!(shown[0].contains("gate → claude-sonnet-4-6 (claude plan)"));
    assert!(shown[1].contains("gate → claude-opus-4-7 (claude plan)"));
}

#[tokio::test]
async fn messages_the_turn_never_saw_run_next() {
    let (fixture, _commands) = fixture();
    let mut b = borrowed(&fixture);
    let first = fixture.first;
    let mut done = std::mem::replace(&mut b.done, mpsc::channel(1).1);
    let mut state = loop_state(&fixture, &mut b);
    let session = state.idle.remove(&first).unwrap();

    handle_work_done(
        &mut state,
        WorkDone {
            agent_id: first,
            session: Some(session),
            failure: None,
            unseen: vec![(SteerSource::User, "late".to_string())],
            refused_models: Vec::new(),
        },
    )
    .await;
    assert!(state.hub.is_running(first), "the late message started");

    let finished = next_done(&mut done).await;
    handle_work_done(&mut state, finished).await;
    assert_eq!(user_texts(&state, first), ["late"]);
}

#[tokio::test]
async fn cancel_stops_only_the_named_agent() {
    let (fixture, mut commands) = fixture();
    let mut b = borrowed(&fixture);
    let links = b.links.clone();
    let (first, second) = (fixture.first, fixture.second);
    let mut done = std::mem::replace(&mut b.done, mpsc::channel(1).1);
    let mut state = loop_state(&fixture, &mut b);

    handle_ui_event(&mut state, message(first, "slow one", true), &links).await;
    handle_ui_event(&mut state, message(second, "slow two", true), &links).await;
    handle_ui_event(&mut state, UiEvent::Cancel(first.to_hex()), &links).await;

    let finished = next_done(&mut done).await;
    assert_eq!(finished.agent_id, first);
    handle_work_done(&mut state, finished).await;
    assert!(state.hub.is_running(second));

    fixture.gate.add_permits(1);
    let finished = next_done(&mut done).await;
    assert_eq!(finished.agent_id, second);
    handle_work_done(&mut state, finished).await;
    let _ = drain(&mut commands);
}
