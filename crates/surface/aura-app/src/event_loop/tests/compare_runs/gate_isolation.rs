//! Each arm of a comparison routes on a detached view of Gate of its own:
//! what the runs teach the shared table steers no later run of either
//! arm, each arm draws from a generator seeded from its own cache salt,
//! so one arm's draws neither move nor are moved by the other's, and the
//! subtasks of an arm's runs route on the arm's view too.

use super::*;
use aura_core_types::Difficulty;
use aura_gate::compare::run_name;
use aura_gate::route::{Decision, Work};

/// Gate settings for these tests: one failure in two tries rules a model
/// out, exploring `explore_rate` of routed work, the lead delegating when
/// `delegate`, and no logged review sampled, so routing alone draws.
fn settings(delegate: bool, explore_rate: f64) -> aura_config::GateConfig {
    aura_config::GateConfig {
        delegate,
        min_tries: 2,
        explore_rate,
        review_sample: 0.0,
        ..aura_config::GateConfig::defaults()
    }
}

/// The prompts `agent_id` recorded, oldest first.
fn prompts_of(state: &LoopState<'_>, agent_id: AgentId) -> Vec<PromptPayload> {
    entries(state, agent_id)
        .iter()
        .filter_map(|e| e.tx.prompt_payload())
        .collect()
}

/// The runs of `arm` in `payload`, first run first.
fn runs_of(payload: &ComparePayload, arm: CompareArm) -> Vec<&CompareRun> {
    (1..)
        .map_while(|n| payload.run(&run_name(arm, n)))
        .collect()
}

/// The route a compare run records for its turn routed by `decision`.
fn compare_route(decision: &Decision) -> Route {
    Route {
        explored: decision.explored,
        propensity: decision.propensity,
        effort: (decision.effort.reasoning_effort_wire()).map(str::to_string),
        ..Route::turn(decision.model.id.clone(), RouteReason::Compare)
    }
}

/// The seed of the view of Gate `run`'s arm routed on: the arm's cache
/// salt, which every request of the run carries, read as hex.
fn seed_of(fixture: &Fixture, run: &CompareRun) -> u64 {
    let sent = fixture.sent_by(run.agent_id);
    let salt = (sent.first().and_then(|s| s.cache_salt.clone()))
        .unwrap_or_else(|| panic!("{} sent no salted request", run.name));
    assert!(
        sent.iter().all(|s| s.cache_salt.as_ref() == Some(&salt)),
        "{}: {sent:#?}",
        run.name
    );
    u64::from_str_radix(&salt, 16).unwrap()
}

/// Easy debugging, the work [`one_arms_outcomes_do_not_steer_the_other_arms_routing`]
/// routes.
fn easy_debugging() -> Work<'static> {
    Work::turn(PromptCategory::Debugging).of(Some(Difficulty::Easy))
}

/// Where the table `state` starts the comparison from sends easy
/// debugging, after checking it is Haiku and that one failure there is
/// enough to move the work off it.
fn starting_route(state: &LoopState<'_>) -> Decision {
    let candidates = state.models.gate_candidates();
    let work = easy_debugging();
    let start = state.hub.gate.snapshot(0);
    let starts_on = start.redecide(&work, &candidates).unwrap();
    assert_eq!(starts_on.model.id, HAIKU, "easy debugging starts cheap");
    let after_one_failure = start.snapshot(0);
    after_one_failure.learn(
        RouteLevel::Turn,
        (PromptCategory::Debugging, Some(Difficulty::Easy)),
        HAIKU,
        Some(false),
        (1, false),
    );
    assert_ne!(
        after_one_failure
            .redecide(&work, &candidates)
            .unwrap()
            .model
            .id,
        HAIKU,
        "one failure on Haiku is enough to move the work off it"
    );
    starts_on
}

/// Check the shared table of `state` learned every failure of the
/// comparison's Haiku runs and so now routes easy debugging elsewhere.
fn assert_shared_table_moved_off_haiku(state: &LoopState<'_>) {
    let haiku = state.hub.gate.with_stats(|stats| {
        (stats.row(RouteLevel::Turn, PromptCategory::Debugging, HAIKU))
            .map(|r| (r.tries, r.successes))
    });
    assert!(
        haiku.is_some_and(|(tries, successes)| tries >= 3 && successes == 0),
        "the shared table learned every failure on Haiku: {haiku:?}"
    );
    let candidates = state.models.gate_candidates();
    assert_ne!(
        state
            .hub
            .gate
            .redecide(&easy_debugging(), &candidates)
            .unwrap()
            .model
            .id,
        HAIKU,
        "so it now routes the work elsewhere, which no Gate run read"
    );
}

#[tokio::test]
async fn one_arms_outcomes_do_not_steer_the_other_arms_routing() {
    let (fixture, mut commands) = fixture_with_gate(GateLive::new(settings(false, 0.0)));
    let mut b = borrowed(&fixture);
    let links = b.links.clone();
    let mut state = loop_state(&fixture, &mut b);
    state.models = gate_on_zos(&fixture);
    state.sync_turn_base();
    // One run at a time, so each run starts after the last one's failures
    // were committed and learned by the shared table.
    state.compare = compare::CompareState::new(fixture._dirs[0].path().join("compare"), 1);
    let starts_on = starting_route(&state);

    let payload = run_comparison(
        &mut state,
        &links,
        &mut commands,
        "fix it (easy): run failing check --repeats 3 --against haiku",
    )
    .await;

    assert_eq!(payload.status, CompareStatus::Done, "{payload:#?}");
    let gate_runs = runs_of(&payload, CompareArm::A);
    assert_eq!(gate_runs.len(), 3);
    for run in gate_runs {
        assert_eq!(run.error, None, "{}", run.name);
        let first = prompts_of(&state, run.agent_id).remove(0);
        assert_eq!(
            first.route,
            compare_route(&starts_on),
            "{} starts where the table the comparison started from sends it",
            run.name
        );
        assert_eq!(
            signals_of(&state, run.agent_id)[0],
            OutcomeSignal::ChecksFailed,
            "{} failed there",
            run.name
        );
    }
    assert_shared_table_moved_off_haiku(&state);
}

/// Runs per arm in [`explored_against`]'s comparisons.
const RUNS: u32 = 4;

/// Run, one at a time, a comparison in learn mode of Gate against
/// `against` on a question, [`RUNS`] times per arm, on Gate exploring
/// half its routed work, and check that each Gate arm's routes are those
/// of a fresh view of the table it started from, seeded with the arm's
/// seed, replaying that arm's work alone.
async fn explored_against(against: &str) {
    let (fixture, mut commands) = fixture_with_gate(GateLive::new(settings(true, 0.5)));
    let mut b = borrowed(&fixture);
    let links = b.links.clone();
    let mut state = loop_state(&fixture, &mut b);
    state.models = gate_on_zos(&fixture);
    state.sync_turn_base();
    state.compare = compare::CompareState::new(fixture._dirs[0].path().join("compare"), 1);
    let candidates = state.models.gate_candidates();
    let start = state.hub.gate.snapshot(0);

    let args = format!("what does gate do? --repeats {RUNS} --against {against} --learn");
    let payload = run_comparison(&mut state, &links, &mut commands, &args).await;

    assert_eq!(payload.status, CompareStatus::Done, "{payload:#?}");
    let gate_arms: &[CompareArm] = if against.starts_with("gate") {
        &CompareArm::BOTH
    } else {
        &[CompareArm::A]
    };
    let mut seeds = Vec::new();
    for &arm in gate_arms {
        let runs = runs_of(&payload, arm);
        assert_eq!(runs.len(), RUNS as usize, "{against}: arm {arm}");
        let seed = seed_of(&fixture, runs[0]);
        seeds.push(seed);
        let replay = start.snapshot(seed);
        for run in runs {
            assert_eq!(run.error, None, "{against}: {}", run.name);
            assert_eq!(seed_of(&fixture, run), seed, "one arm, one seed");
            let prompt = prompts_of(&state, run.agent_id).remove(0);
            let work = Work::turn(prompt.category.unwrap()).of(prompt.difficulty);
            let replayed = replay.decide(&work, &candidates).unwrap();
            assert_eq!(
                prompt.route.propensity, 0.5,
                "{against}: {} drew on whether to explore",
                run.name
            );
            assert_eq!(
                prompt.route,
                compare_route(&replayed),
                "{against}: {} routes as its arm's own draws alone decide",
                run.name
            );
        }
    }
    seeds.dedup();
    assert_eq!(seeds.len(), gate_arms.len(), "the arms' seeds differ");
}

#[tokio::test]
async fn each_arms_random_draws_are_its_own() {
    // Against a pinned arm, which draws nothing, and against another Gate
    // arm, which draws as often as A does: A's draws are the same
    // function of its seed either way, and so are B's of its own.
    explored_against("opus").await;
    explored_against("gate:nodelegate").await;
}

/// Dispatch a reviewer asked `prompt` from `parent`; returns the
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
    (fixture.dispatcher.dispatch(request).await.unwrap())
        .child_agent_id
        .unwrap()
}

#[tokio::test]
async fn a_compare_subtask_routes_on_its_arms_view() {
    let (fixture, _commands) = fixture();
    let mut b = borrowed(&fixture);
    let first = fixture.first;
    let mut state = loop_state(&fixture, &mut b);
    state.sync_turn_base();
    let candidates = gate_on_zos(&fixture).gate_candidates();
    let route_of = |child: AgentId| prompts_of(&state, child).remove(0).route;
    let gate_arm = || subagents::ArmRouting::Gate {
        candidates: candidates.clone(),
        delegate: true,
    };
    let view = Arc::new(fixture.hub.gate.snapshot(7));
    // Learned after the arm's view was taken: Sonnet fails these subtasks.
    fixture.hub.gate.learn(
        RouteLevel::Subtask,
        (PromptCategory::Question, Some(Difficulty::Medium)),
        SONNET,
        None,
        (3, false),
    );
    let work = Work::new(
        RouteLevel::Subtask,
        PromptCategory::Question,
        Some("reviewer"),
    )
    .of(Some(Difficulty::Medium));
    let shared = fixture.hub.gate.redecide(&work, &candidates).unwrap();
    assert_ne!(shared.model.id, SONNET, "the shared table moved on");

    fixture
        .hub
        .begin_compare_run(first, gate_arm(), Arc::clone(&view));
    let routed = dispatch(&fixture, first, "what changed?").await;
    assert_eq!(
        route_of(routed),
        Route::subtask(SONNET, RouteReason::Default).at_effort("medium"),
        "the run's subtask routes on its arm's view, which never learned that"
    );
    let nested = dispatch(&fixture, routed, "what changed?").await;
    assert_eq!(route_of(nested).model, SONNET, "and so does its own");

    fixture
        .hub
        .begin_compare_run(first, gate_arm(), Arc::clone(&fixture.hub.gate));
    let steered = dispatch(&fixture, first, "what changed?").await;
    assert_eq!(
        route_of(steered).model,
        shared.model.id,
        "on the shared table, the same subtask goes where it learned to"
    );
    fixture.hub.end_compare_run(first);
}
