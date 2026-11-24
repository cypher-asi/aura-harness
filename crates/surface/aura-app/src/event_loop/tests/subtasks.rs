//! Gate on `task` spawns: routing, pinning, and the outcomes that fill
//! the subtask rows.

use super::*;
use aura_core_types::{Difficulty, SubagentDispatchRequest, SubagentExit, SubagentResult};

const SONNET: &str = "aura-claude-sonnet-4-6";
const GPT: &str = "aura-gpt-5-4";
const HAIKU: &str = "aura-claude-haiku-4-5";
const OPUS: &str = "aura-claude-opus-4-7";

fn task(parent: AgentId, kind: &str, prompt: &str) -> SubagentDispatchRequest {
    serde_json::from_value(serde_json::json!({
        "parent_agent_id": parent,
        "subagent_type": kind,
        "prompt": prompt,
        "parent_permissions": aura_core_types::AgentPermissions::full_access(),
        "user_tool_defaults": aura_core_types::UserToolDefaults::default(),
    }))
    .unwrap()
}

async fn dispatch(fixture: &Fixture, request: SubagentDispatchRequest) -> SubagentResult {
    fixture.dispatcher.dispatch(request).await.unwrap()
}

/// Dispatch `request` and return the child it ran on.
async fn spawned(fixture: &Fixture, request: SubagentDispatchRequest) -> AgentId {
    dispatch(fixture, request).await.child_agent_id.unwrap()
}

/// The prompts and outcomes in `agent_id`'s record, oldest first.
fn record(fixture: &Fixture, agent_id: AgentId) -> (Vec<PromptPayload>, Vec<OutcomePayload>) {
    let entries = fixture
        .hub
        .factory
        .store
        .scan_record(agent_id, 1, 10_000)
        .unwrap();
    let prompts = entries.iter().filter_map(|e| e.tx.prompt_payload());
    let outcomes = entries.iter().filter_map(|e| e.tx.outcome_payload());
    (prompts.collect(), outcomes.collect())
}

fn signals(fixture: &Fixture, agent_id: AgentId) -> Vec<OutcomeSignal> {
    record(fixture, agent_id)
        .1
        .into_iter()
        .map(|o| o.signal)
        .collect()
}

/// Select gate (signed in to zOS) and publish it to the hub, as the start
/// of a turn does.
fn select_gate(fixture: &Fixture, state: &mut LoopState<'_>) {
    state.models = gate_on_zos(fixture);
    state.sync_turn_base();
}

/// Have `agent_id` start a turn, so the subtasks it dispatches belong to it.
fn start_turn(fixture: &Fixture, state: &LoopState<'_>, agent_id: AgentId) {
    fixture.hub.start_running(
        agent_id,
        state.kernel_for(agent_id).unwrap(),
        CancellationToken::new(),
        None,
    );
}

fn subtask_row(fixture: &Fixture, model: &str) -> Option<(u32, u32)> {
    fixture
        .hub
        .gate
        .stats()
        .row(RouteLevel::Subtask, PromptCategory::Question, model)
        .map(|row| (row.tries, row.successes))
}

#[tokio::test]
async fn a_task_under_gate_runs_on_the_model_gate_picks_for_it() {
    let (fixture, mut commands) = fixture();
    let mut b = borrowed(&fixture);
    let first = fixture.first;
    let mut state = loop_state(&fixture, &mut b);
    select_gate(&fixture, &mut state);

    let result = dispatch(
        &fixture,
        task(first, "reviewer", "fix the flaky parser test"),
    )
    .await;

    assert!(
        matches!(result.exit, SubagentExit::Completed),
        "{:?}",
        result.exit
    );
    let child = result.child_agent_id.unwrap();
    let (prompts, outcomes) = record(&fixture, child);
    assert_eq!(
        prompts,
        [PromptPayload {
            category: Some(PromptCategory::Debugging),
            classifier: state.models.classifier_model(),
            route: Route::subtask(SONNET, RouteReason::Default).at_effort("medium"),
            kind: Some("reviewer".to_string()),
            difficulty: Some(Difficulty::Medium),
            assessment: Some(mock_assessment("debugging", Difficulty::Medium)),
            ..PromptPayload::new("fix the flaky parser test")
        }],
        "medium debugging starts on the standard tier, as for turns"
    );
    assert_eq!(outcomes.len(), 1);
    assert_eq!(outcomes[0].model, SONNET);
    assert_eq!(outcomes[0].signal, OutcomeSignal::Unavailable);
    assert!(
        prompts_of(&state, first).is_empty(),
        "the subtask is recorded on the child"
    );
    let recent = fixture.hub.gate.recent();
    assert_eq!(recent.len(), 1);
    assert_eq!(recent[0].level, RouteLevel::Subtask);
    let rows = fixture.hub.processes.snapshot();
    let row = rows
        .iter()
        .find(|r| r.id == processes::task_id(child))
        .unwrap();
    assert_eq!(row.model.as_deref(), Some(SONNET));
    assert_eq!(
        route_lines(&mut commands),
        ["gate → aura-claude-sonnet-4-6 (cloud) at medium effort · subtask, debugging, default"]
    );
}

#[tokio::test]
async fn subtask_outcomes_fill_the_subtask_rows_and_show_in_slash_gate() {
    let (fixture, mut commands) = fixture();
    let mut b = borrowed(&fixture);
    let links = b.links.clone();
    let first = fixture.first;
    let mut state = loop_state(&fixture, &mut b);
    select_gate(&fixture, &mut state);
    fixture.hub.set_permissions(first, yolo());

    let passed = spawned(&fixture, task(first, "explorer", "run passing check")).await;
    let failed = spawned(&fixture, task(first, "explorer", "run failing check")).await;
    let errored = dispatch(&fixture, task(first, "explorer", "explode")).await;

    assert_eq!(signals(&fixture, passed), [OutcomeSignal::ChecksPassed]);
    assert_eq!(signals(&fixture, failed), [OutcomeSignal::ChecksFailed]);
    assert!(matches!(errored.exit, SubagentExit::Failed { .. }));
    let errored = errored.child_agent_id.unwrap();
    assert_eq!(signals(&fixture, errored), [OutcomeSignal::Error]);
    assert_eq!(subtask_row(&fixture, SONNET), Some((3, 1)));
    assert!(
        fixture
            .hub
            .gate
            .stats()
            .rows()
            .iter()
            .all(|(key, _)| key.level == RouteLevel::Subtask),
        "turn rows learn nothing from subtasks"
    );

    let agents: Vec<AgentId> = subagents::lock(&fixture.hub.registry)
        .agents()
        .iter()
        .map(|a| a.agent_id())
        .collect();
    let rebuilt = steady_gate();
    rebuilt.rebuild(fixture.hub.factory.store.as_ref(), &agents);
    assert_eq!(
        rebuilt
            .stats()
            .row(RouteLevel::Subtask, PromptCategory::Question, SONNET)
            .map(|row| (row.tries, row.successes)),
        Some((3, 1)),
        "the startup scan reads the children's records too"
    );

    drain(&mut commands);
    handle_ui_event(&mut state, UiEvent::ShowGate, &links).await;
    let text = system_texts(&drain(&mut commands));
    let (_, subtasks) = text.split_once("\n\nSubtasks\n").expect(&text);
    let sonnet = subtasks
        .lines()
        .find(|line| line.contains("aura-claude-sonnet-4-6 (cloud)"))
        .expect(subtasks);
    assert!(sonnet.contains("33%"), "{text}");
    let question = subtasks
        .lines()
        .find(|line| line.trim_start().starts_with("question"))
        .expect(subtasks);
    assert!(
        question.contains("aura-gpt-5-4 (cloud)") && question.contains("pick (default)"),
        "one success in three rules Sonnet out for questions: {text}"
    );
}

#[tokio::test]
async fn a_subtask_dispatched_again_in_the_same_turn_failed_the_first_time() {
    let (fixture, _commands) = fixture();
    let mut b = borrowed(&fixture);
    let first = fixture.first;
    let mut state = loop_state(&fixture, &mut b);
    select_gate(&fixture, &mut state);
    start_turn(&fixture, &state, first);

    let prompt = "Find where the parser handles comments.";
    let first_try = spawned(&fixture, task(first, "explorer", prompt)).await;
    let other_kind = spawned(&fixture, task(first, "reviewer", prompt)).await;
    let unrelated = spawned(&fixture, task(first, "explorer", "List the crates.")).await;
    let second_try = spawned(
        &fixture,
        task(first, "explorer", "Find where the parser handles comments!"),
    )
    .await;

    assert_eq!(
        signals(&fixture, first_try),
        [OutcomeSignal::Unavailable, OutcomeSignal::Redone]
    );
    let (_, outcomes) = record(&fixture, first_try);
    assert_eq!(outcomes[1].success, Some(false));
    assert_eq!(outcomes[1].prompt_tx, outcomes[0].prompt_tx);
    for other in [other_kind, unrelated, second_try] {
        assert_eq!(signals(&fixture, other), [OutcomeSignal::Unavailable]);
    }
    assert_eq!(
        subtask_row(&fixture, SONNET),
        Some((1, 0)),
        "only the redone attempt has a verdict"
    );

    fixture.hub.stop_running(first);
    start_turn(&fixture, &state, first);
    spawned(&fixture, task(first, "explorer", prompt)).await;
    assert_eq!(
        signals(&fixture, second_try),
        [OutcomeSignal::Unavailable],
        "a later turn may ask again"
    );
    fixture.hub.stop_running(first);
}

#[tokio::test]
async fn pinned_subtasks_keep_their_model_and_gate_records_no_outcome() {
    let (fixture, _commands) = fixture();
    let mut b = borrowed(&fixture);
    let first = fixture.first;
    let mut state = loop_state(&fixture, &mut b);
    select_gate(&fixture, &mut state);
    std::fs::write(
        fixture._dirs[0].path().join("agent_defs/pinned.md"),
        format!("---\nname: pinned\ndescription: Runs on opus.\nmodel: {OPUS}\n---\nBe brief.\n"),
    )
    .unwrap();
    assert!(fixture.hub.reload_definitions().is_empty());

    let by_definition = task(first, "pinned", "what changed?");
    let mut in_the_call = task(first, "reviewer", "what changed?");
    in_the_call.model_override = Some(HAIKU.to_string());
    let mut council = task(first, "reviewer", "what changed?");
    council.model_override = Some(GPT.to_string());
    council.council_index = Some(0);

    for (request, model) in [(by_definition, OPUS), (in_the_call, HAIKU), (council, GPT)] {
        let kind = request.subagent_type.clone();
        let child = spawned(&fixture, request).await;
        let (prompts, outcomes) = record(&fixture, child);
        assert_eq!(prompts.len(), 1, "{kind}");
        assert_eq!(
            prompts[0].route,
            Route::subtask(model, RouteReason::Pinned),
            "{kind}"
        );
        assert_eq!(prompts[0].category, None, "{kind}");
        assert_eq!(prompts[0].kind.as_deref(), Some(kind.as_str()));
        assert!(outcomes.is_empty(), "{kind}");
    }
    assert!(fixture.hub.gate.recent().is_empty(), "Gate decided nothing");
}

#[tokio::test]
async fn every_subtask_keeps_the_main_model_the_user_pinned() {
    let (fixture, _commands) = fixture();
    let mut b = borrowed(&fixture);
    let first = fixture.first;
    let mut state = loop_state(&fixture, &mut b);
    state.sync_turn_base();

    let child = spawned(&fixture, task(first, "reviewer", "fix the parser")).await;

    let (prompts, outcomes) = record(&fixture, child);
    assert_eq!(
        prompts[0].route,
        Route::subtask("mock-model", RouteReason::Pinned)
    );
    assert_eq!(prompts[0].category, None);
    assert!(prompts[0].classifier.is_empty());
    assert!(outcomes.is_empty());
    assert!(fixture.hub.gate.recent().is_empty());
}
