//! How each arm routes its turns and subtasks: on a pinned model, by
//! difficulty, or by Gate, with or without enforced delegation, exploring
//! only in learn mode.

use super::*;
use aura_core_types::Difficulty;

/// The first prompt `agent_id` recorded.
fn first_prompt(state: &LoopState<'_>, agent_id: AgentId) -> PromptPayload {
    entries(state, agent_id)
        .iter()
        .find_map(|e| e.tx.prompt_payload())
        .unwrap()
}

/// Dispatches a reviewer asked `prompt` from `parent`; returns the
/// reviewer's id.
async fn dispatch(fixture: &Fixture, parent: AgentId, prompt: &str) -> AgentId {
    let request: aura_core_types::SubagentDispatchRequest =
        serde_json::from_value(serde_json::json!({
            "parent_agent_id": parent,
            "subagent_type": "reviewer",
            "prompt": prompt,
            "parent_permissions": aura_core_types::AgentPermissions::full_access(),
            "user_tool_defaults": aura_core_types::UserToolDefaults::default(),
        }))
        .unwrap();
    fixture
        .dispatcher
        .dispatch(request)
        .await
        .unwrap()
        .child_agent_id
        .unwrap()
}

#[tokio::test]
async fn subtasks_of_a_run_follow_its_arm_not_the_users_selection() {
    let (fixture, _commands) = fixture();
    let mut b = borrowed(&fixture);
    let first = fixture.first;
    let mut state = loop_state(&fixture, &mut b);
    state.sync_turn_base();
    let candidates = gate_on_zos(&fixture).gate_candidates();
    let route_of = |child: AgentId| first_prompt(&state, child).route;

    fixture.hub.begin_compare_run(
        first,
        subagents::ArmRouting::Pinned(OPUS.to_string()),
        Arc::clone(&fixture.hub.gate),
    );
    let pinned = dispatch(&fixture, first, "what changed?").await;
    assert_eq!(
        route_of(pinned),
        Route::subtask(OPUS, RouteReason::Pinned),
        "the other arm's subtasks run on its model"
    );

    fixture.hub.begin_compare_run(
        first,
        subagents::ArmRouting::Gate {
            candidates,
            delegate: true,
        },
        Arc::clone(&fixture.hub.gate),
    );
    let routed = dispatch(&fixture, first, "what changed?").await;
    assert_eq!(
        route_of(routed),
        Route::subtask(SONNET, RouteReason::Default).at_effort("medium"),
        "the Gate arm's subtasks are routed though the user pinned a model"
    );
    let nested = dispatch(&fixture, routed, "what changed?").await;
    assert_eq!(route_of(nested).model, SONNET, "and so are theirs");

    fixture.hub.end_compare_run(first);
    let plain = dispatch(&fixture, first, "what changed?").await;
    assert_eq!(
        route_of(plain),
        Route::subtask("mock-model", RouteReason::Pinned)
    );
}

#[tokio::test]
async fn a_by_difficulty_arm_runs_each_turn_and_subtask_on_its_difficultys_model() {
    let (fixture, mut commands) = fixture();
    let mut b = borrowed(&fixture);
    let links = b.links.clone();
    let mut state = loop_state(&fixture, &mut b);
    state.models = gate_on_zos(&fixture);
    state.sync_turn_base();

    compare(
        &mut state,
        &links,
        "what does gate do? (hard) --repeats 1 --a sonnet \
         --against difficulty:easy=haiku,medium=sonnet,hard=opus",
    )
    .await;
    let estimate = system_texts(&drain(&mut commands));
    for part in [
        format!("B             by difficulty: easy → {HAIKU} (cloud)"),
        format!("medium →\n                {SONNET} (cloud)"),
        format!("hard → {OPUS}"),
    ] {
        assert!(estimate.contains(&part), "{part:?} in {estimate}");
    }
    let payload = go(&mut state, &links).await;

    assert_eq!(
        payload.arm_b,
        ArmSpec::ByDifficulty {
            easy: HAIKU.to_string(),
            medium: SONNET.to_string(),
            hard: OPUS.to_string(),
        }
    );
    let run = payload.run("b-1").unwrap();
    assert_eq!(run.error, None);
    assert_eq!(run.model, OPUS, "hard work runs on the hard model");
    let prompt = first_prompt(&state, run.agent_id);
    assert_eq!(prompt.route.model, OPUS);
    assert_eq!(prompt.route.reason, RouteReason::Compare);
    assert_eq!(prompt.difficulty, Some(Difficulty::Hard));
    assert_eq!(payload.run("a-1").unwrap().model, SONNET);

    // Its subtasks are classified and go to their difficulty's model too.
    assert_subtasks_go_by_difficulty(&fixture, &state, &payload).await;
}

/// Checks that the subtasks of a run of `payload`'s by-difficulty arm run
/// on their difficulty's model.
async fn assert_subtasks_go_by_difficulty(
    fixture: &Fixture,
    state: &LoopState<'_>,
    payload: &ComparePayload,
) {
    let runnable = state.models.runnable_models();
    let routes = aura_gate::compare::DifficultyRoutes::of(&payload.arm_b, &runnable).unwrap();
    let first = fixture.first;
    fixture.hub.begin_compare_run(
        first,
        subagents::ArmRouting::ByDifficulty(routes),
        Arc::clone(&fixture.hub.gate),
    );
    for (prompt, model) in [("what changed? (easy)", HAIKU), ("what changed?", SONNET)] {
        let child = dispatch(fixture, first, prompt).await;
        let route = first_prompt(state, child).route;
        assert_eq!(
            (route.model.as_str(), route.reason),
            (model, RouteReason::Compare),
            "{prompt}"
        );
    }
    fixture.hub.end_compare_run(first);
}

#[tokio::test]
async fn a_pinned_arm_runs_on_unclassified_when_its_prompt_cannot_be_classified() {
    let (fixture, mut commands) = fixture();
    let mut b = borrowed(&fixture);
    let links = b.links.clone();
    let mut state = loop_state(&fixture, &mut b);
    state.models = gate_on_zos(&fixture);
    state.sync_turn_base();

    let payload = run_comparison(
        &mut state,
        &links,
        &mut commands,
        "classifier down: what does gate do? --repeats 1 --a sonnet --against gate",
    )
    .await;

    let pinned = payload.run("a-1").unwrap();
    assert_eq!(pinned.error, None, "the pinned run goes on");
    assert_eq!(pinned.model, SONNET);
    let prompt = first_prompt(&state, pinned.agent_id);
    assert_eq!(prompt.category, None);
    assert_eq!(prompt.difficulty, None);
    assert_eq!(prompt.route.model, SONNET);
    assert!(
        signals_of(&state, pinned.agent_id).is_empty(),
        "no category to count it under, so no outcome for Gate"
    );

    let routed = payload.run("b-1").unwrap();
    assert!(
        routed
            .error
            .as_deref()
            .is_some_and(|e| e.contains("could not classify")),
        "{:?}",
        routed.error
    );
    assert_eq!(routed.model, "gate", "named for its arm");
}

/// Teaches Gate how Sonnet and GPT did at changes across several files:
/// Sonnet failed, and GPT succeeded where it led and failed where it did
/// not.
fn learn_who_leads_changes_across_files(state: &LoopState<'_>) {
    for (model, led, outcomes) in [
        (SONNET, false, (3, false)),
        (GPT, true, (10, true)),
        (GPT, false, (3, false)),
    ] {
        state.hub.gate.learn(
            RouteLevel::Turn,
            (PromptCategory::MultiFileChange, Some(Difficulty::Medium)),
            model,
            Some(led),
            outcomes,
        );
    }
}

#[tokio::test]
async fn gate_is_compared_with_itself_without_enforced_delegation() {
    let (fixture, mut commands) = fixture();
    let mut b = borrowed(&fixture);
    let links = b.links.clone();
    let mut state = loop_state(&fixture, &mut b);
    state.models = gate_on_zos(&fixture);
    state.sync_turn_base();
    learn_who_leads_changes_across_files(&state);

    compare(
        &mut state,
        &links,
        "write the answer across files --repeats 1 --against gate:nodelegate",
    )
    .await;
    let estimate = system_texts(&drain(&mut commands));
    assert!(
        estimate.contains("\n  B             gate without enforced delegation (gate:nodelegate)"),
        "{estimate}"
    );
    let payload = go(&mut state, &links).await;

    assert_eq!(payload.against, "gate:nodelegate");
    let gate = payload.run("a-1").unwrap();
    let other = payload.run("b-1").unwrap();
    // Sonnet failed at changes across several files itself, so this one
    // starts on the frontier, at gpt-5.4, and is led there, where leading
    // is proven.
    for run in [gate, other] {
        assert_eq!(run.error, None, "{}", run.name);
        assert_eq!(run.model, GPT, "both arms are routed by Gate");
        assert_eq!(
            first_prompt(&state, run.agent_id).route,
            Route::turn(GPT, RouteReason::Compare).at_effort("medium")
        );
    }
    assert_eq!(gate.files_changed, 0, "the enforced lead may not write");
    assert!(!gate.copy.workdir.join("answer.txt").exists());
    assert_eq!(other.files_changed, 1, "the other lead writes itself");
    assert_eq!(
        std::fs::read_to_string(other.copy.workdir.join("answer.txt")).unwrap(),
        GPT
    );
    let shown = system_texts(&drain(&mut commands));
    assert!(
        shown.contains("A gate vs B gate without enforced delegation"),
        "{shown}"
    );
}

#[tokio::test]
async fn gate_arms_explore_only_in_learn_mode() {
    for (args, explored) in [("", false), (" --learn", true)] {
        let gate = GateLive::with_rng(
            aura_config::GateConfig {
                explore_rate: 1.0,
                ..aura_config::GateConfig::defaults()
            },
            aura_gate::route::SplitMix64::seeded(3),
        );
        let (fixture, mut commands) = fixture_with_gate(gate);
        let mut b = borrowed(&fixture);
        let links = b.links.clone();
        let mut state = loop_state(&fixture, &mut b);
        state.models = gate_on_zos(&fixture);
        state.sync_turn_base();

        let payload = run_comparison(
            &mut state,
            &links,
            &mut commands,
            &format!("what does gate do? --repeats 1 --against opus{args}"),
        )
        .await;

        assert_eq!(payload.status, CompareStatus::Done, "{payload:#?}");
        let route = first_prompt(&state, payload.run("a-1").unwrap().agent_id).route;
        assert_eq!(
            (route.reason, route.explored),
            (RouteReason::Compare, explored),
            "{args}"
        );
        assert_eq!(
            payload.notes.iter().any(|n| n.starts_with("Learn mode")),
            explored
        );
    }
}

#[tokio::test]
async fn a_runs_arm_decides_whether_its_lead_delegates() {
    let (fixture, _commands) = fixture();
    let first = fixture.first;
    let candidates = gate_on_zos(&fixture).gate_candidates();
    assert_eq!(
        fixture.hub.leads(first, None),
        None,
        "the user pinned a model"
    );
    for (delegate, leads) in [(true, Some(true)), (false, Some(false))] {
        fixture.hub.begin_compare_run(
            first,
            subagents::ArmRouting::Gate {
                candidates: candidates.clone(),
                delegate,
            },
            Arc::clone(&fixture.hub.gate),
        );
        assert_eq!(fixture.hub.leads(first, None), leads);
        assert_eq!(
            fixture.hub.leads(first, Some("implement")),
            None,
            "an instance of a permanent agent never leads"
        );
    }
    fixture.hub.begin_compare_run(
        first,
        subagents::ArmRouting::Pinned(OPUS.to_string()),
        Arc::clone(&fixture.hub.gate),
    );
    assert_eq!(fixture.hub.leads(first, None), None);
}
