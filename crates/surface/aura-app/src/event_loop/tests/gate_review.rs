//! Gate's judge and review of work.

use super::*;

/// Run `text`, easy debugging, as a turn under Gate, which starts it on
/// Haiku, and return the routes of its prompts, its outcomes, the chat's
/// system lines, and the tries and successes of the debugging rows of
/// Haiku and of Sonnet, the cheapest model of the next tier up.
async fn debugging_turn_under_gate(
    text: &str,
) -> (
    Vec<Route>,
    Vec<(String, OutcomeSignal)>,
    String,
    [Option<(u32, u32)>; 2],
) {
    debugging_turn_under(steady_gate(), text).await
}

/// Append to `log` attempt `n` of `agent`: checked work that changed no
/// file, wrong for the first five, right after.
fn seed_attempt(log: &aura_gate::attempts::AttemptLog, agent: &str, n: u8) {
    use aura_gate::attempts::{Attempt, Line, Terms, Truth, TruthSource};
    use aura_gate::evidence::Evidence;
    let agent = agent.to_string();
    let unchanged = n < 5;
    let ended = OutcomePayload {
        prompt_tx: Hash::from_content(&[n]),
        category: PromptCategory::Debugging,
        model: "aura-claude-haiku-4-5".to_string(),
        success: Some(true),
        signal: OutcomeSignal::ChecksPassed,
        cost_usd: 0.01,
        subtask_cost_usd: 0.0,
        latency_ms: 10,
    };
    let terms = Terms {
        difficulty: None,
        led: Some(false),
        routed: true,
    };
    let evidence = Evidence {
        checks_run: 1,
        files_changed: Some(u32::from(!unchanged)),
        reply: "Done.".to_string(),
        ..Evidence::default()
    };
    let attempt = Attempt::new(u64::from(n), agent.clone(), &ended, terms, evidence);
    let truth = Truth {
        at_ms: u64::from(n),
        agent: agent.clone(),
        prompt_tx: attempt.prompt_tx.clone(),
        source: TruthSource::TaskCheck,
        passed: !unchanged,
        compare: None,
        run: None,
        task: None,
        copy: None,
    };
    log.append(&Line::Attempt(attempt)).unwrap();
    log.append(&Line::Truth(truth)).unwrap();
}

/// [`steady_gate`] that reviews work about to stand as `review` says,
/// every turn a review it only logs, with an attempt log (in the folder
/// returned) in which checked work that changed no file has been wrong
/// often enough to be reviewed.
fn reviewing_gate(review: aura_config::ReviewMode) -> (GateLive, TempDir) {
    use aura_gate::attempts::AttemptLog;
    let dir = TempDir::new().unwrap();
    let log = AttemptLog::in_data_dir(dir.path());
    let agent = AgentId::generate().to_hex();
    for n in 0..10u8 {
        seed_attempt(&log, &agent, n);
    }
    let gate = GateLive::new(aura_config::GateConfig {
        review,
        review_sample: 1.0,
        ..aura_config::GateConfig::defaults().without_exploring()
    });
    gate.log_attempts_to(log);
    assert_eq!(
        gate.tiers().of(aura_gate::evidence::Flag::NoChanges),
        aura_gate::attempts::Tier::Review
    );
    (gate, dir)
}

/// [`debugging_turn_under_gate`] with Gate's table `gate`.
async fn debugging_turn_under(
    gate: GateLive,
    text: &str,
) -> (
    Vec<Route>,
    Vec<(String, OutcomeSignal)>,
    String,
    [Option<(u32, u32)>; 2],
) {
    let (fixture, mut commands) = fixture_with_gate(gate);
    let mut b = borrowed(&fixture);
    let links = b.links.clone();
    let first = fixture.first;
    let mut done = std::mem::replace(&mut b.done, mpsc::channel(1).1);
    let mut state = loop_state(&fixture, &mut b);
    state.models = gate_on_zos(&fixture);
    unrestricted(&mut state, &links, first).await;
    drain(&mut commands);

    let finished = turn(&mut state, &mut done, &links, first, text).await;
    assert_eq!(finished.failure, None);

    let routes = prompts_of(&state, first)
        .into_iter()
        .map(|(_, _, p)| p.route)
        .collect();
    let outcomes = outcomes_of(&state, first)
        .into_iter()
        .map(|(_, o)| (o.model, o.signal))
        .collect();
    let stats = fixture.hub.gate.stats();
    let row = |model: &str| {
        stats
            .row(RouteLevel::Turn, PromptCategory::Debugging, model)
            .map(|row| (row.tries, row.successes))
    };
    let rows = [row("aura-claude-haiku-4-5"), row("aura-claude-sonnet-4-6")];
    (routes, outcomes, system_texts(&drain(&mut commands)), rows)
}

#[tokio::test]
async fn a_failed_turn_the_judge_blames_on_the_model_carries_on_on_a_stronger_one() {
    const HAIKU: &str = "aura-claude-haiku-4-5";
    const SONNET: &str = "aura-claude-sonnet-4-6";
    let (routes, outcomes, shown, rows) =
        debugging_turn_under_gate("fix it (easy): run failing check (judge: blame)").await;

    assert_eq!(
        routes,
        [
            Route::turn(HAIKU, RouteReason::Default).at_effort("medium"),
            Route::turn(SONNET, RouteReason::Escalate).at_effort("medium"),
        ]
    );
    assert_eq!(
        outcomes,
        [
            (HAIKU.to_string(), OutcomeSignal::ChecksFailed),
            (HAIKU.to_string(), OutcomeSignal::JudgedFail),
            (SONNET.to_string(), OutcomeSignal::Unavailable),
        ]
    );
    assert!(
        shown.contains("the failure is aura-claude-haiku-4-5 (cloud)'s. The change is wrong."),
        "{shown}"
    );
    assert!(
        shown.contains(
            "gate → aura-claude-sonnet-4-6 (cloud) at medium effort · debugging, escalate"
        ),
        "{shown}"
    );
    assert_eq!(
        rows,
        [Some((1, 0)), None],
        "the failure counts once for Haiku; the stronger model's work is not counted"
    );
}

#[tokio::test]
async fn a_passed_check_the_reply_disowns_is_reviewed_and_carries_on_on_a_stronger_model() {
    const HAIKU: &str = "aura-claude-haiku-4-5";
    const SONNET: &str = "aura-claude-sonnet-4-6";
    let (routes, outcomes, shown, rows) = debugging_turn_under_gate(
        "fix it (easy): run passing check, then reply unverified (review: blame)",
    )
    .await;

    assert_eq!(
        routes,
        [
            Route::turn(HAIKU, RouteReason::Default).at_effort("medium"),
            Route::turn(SONNET, RouteReason::Escalate).at_effort("medium"),
        ],
        "{shown}"
    );
    assert_eq!(
        outcomes,
        [
            (HAIKU.to_string(), OutcomeSignal::JudgedFail),
            (SONNET.to_string(), OutcomeSignal::Unavailable),
        ],
        "the check that passed does not count for Haiku"
    );
    assert!(
        shown.contains("The change is untested and wrong."),
        "{shown}"
    );
    assert_eq!(rows, [Some((1, 0)), None]);
}

#[tokio::test]
async fn a_passed_check_the_reply_disowns_stands_when_the_judge_finds_the_work_done() {
    const HAIKU: &str = "aura-claude-haiku-4-5";
    let (routes, outcomes, shown, rows) = debugging_turn_under_gate(
        "fix it (easy): run passing check, then reply unverified (review: fine)",
    )
    .await;

    assert_eq!(
        routes,
        [Route::turn(HAIKU, RouteReason::Default).at_effort("medium")],
        "{shown}"
    );
    assert_eq!(outcomes, [(HAIKU.to_string(), OutcomeSignal::ChecksPassed)]);
    assert!(shown.contains("The change is right."), "{shown}");
    assert_eq!(rows, [Some((1, 1)), None]);
}

#[tokio::test]
async fn hard_work_a_review_finds_unfinished_goes_back_to_its_model_once() {
    let (gate, _log_dir) = reviewing_gate(aura_config::ReviewMode::On);
    let (fixture, mut commands) = fixture_with_gate(gate);
    let mut b = borrowed(&fixture);
    let links = b.links.clone();
    let first = fixture.first;
    let mut done = std::mem::replace(&mut b.done, mpsc::channel(1).1);
    let mut state = loop_state(&fixture, &mut b);
    state.models = gate_on_zos(&fixture);
    unrestricted(&mut state, &links, first).await;
    drain(&mut commands);

    let text = "fix it: run passing check (inspect: once)";
    let finished = turn(&mut state, &mut done, &links, first, text).await;
    assert_eq!(finished.failure, None);
    let shown = system_texts(&drain(&mut commands));

    let prompt_tx = assert_sent_back_once(&state, first, &shown);
    assert_both_reviews_logged(&fixture, &prompt_tx);
}

/// Asserts that `first`'s work under review stayed on its model and went
/// back to it once, then stood; returns its prompt's transaction.
fn assert_sent_back_once(state: &LoopState<'_>, first: AgentId, shown: &str) -> String {
    let prompts = prompts_of(state, first);
    let model = prompts[0].2.route.model.clone();
    assert!(
        prompts
            .iter()
            .all(|(_, _, p)| p.route.reason != RouteReason::Escalate),
        "the work stays on its model: {shown}"
    );
    let sent_back: Vec<&str> = prompts
        .iter()
        .map(|(_, _, p)| p.text.as_str())
        .filter(|text| text.contains(aura_gate::followup::REPAIR_OPENING))
        .collect();
    assert_eq!(sent_back.len(), 1, "{shown}");
    assert!(
        sent_back[0].contains("1. The fix misses the empty case."),
        "{}",
        sent_back[0]
    );
    let outcomes: Vec<(String, OutcomeSignal)> = outcomes_of(state, first)
        .into_iter()
        .map(|(_, o)| (o.model, o.signal))
        .collect();
    assert_eq!(
        outcomes,
        [(model.clone(), OutcomeSignal::ChecksPassed)],
        "one attempt, which stands once the review finds nothing missing: {shown}"
    );
    for part in [
        "work is not finished.\n1. The fix misses the empty case.",
        "gate: sending the review's findings back to",
        "found nothing missing in",
    ] {
        assert!(shown.contains(part), "{part}: {shown}");
    }

    prompts[0].1.to_hex()
}

/// Asserts that both reviews of the work whose prompt is `prompt_tx` are
/// in `fixture`'s attempt log, with why they were asked.
fn assert_both_reviews_logged(fixture: &Fixture, prompt_tx: &str) {
    use aura_gate::attempts::Line;
    use aura_gate::judge::turn::Answer;
    let lines = fixture
        .hub
        .gate
        .attempt_log()
        .unwrap()
        .read()
        .unwrap()
        .lines;
    let reviews: Vec<_> = lines
        .iter()
        .filter_map(|line| match line {
            Line::Review(review) => Some(review),
            _ => None,
        })
        .collect();
    assert_eq!(
        reviews
            .iter()
            .map(|review| review.verdict)
            .collect::<Vec<_>>(),
        [Answer::Model, Answer::Ok]
    );
    for review in &reviews {
        assert_eq!(review.why, ["no_changes"]);
        assert!(review.acted && !review.replayed);
        assert_eq!(review.variant, "plain");
        assert_eq!(review.prompt_tx, prompt_tx);
    }
    assert!(
        lines
            .iter()
            .any(|line| matches!(line, Line::Attempt(attempt) if attempt.prompt_tx == prompt_tx)),
        "{lines:?}"
    );
}

#[tokio::test]
async fn work_a_review_finds_unfinished_twice_moves_to_a_stronger_model() {
    let (gate, _log_dir) = reviewing_gate(aura_config::ReviewMode::On);
    let (routes, outcomes, shown, _) =
        debugging_turn_under(gate, "fix it (easy): run passing check (inspect: blame)").await;

    let first = routes[0].model.clone();
    assert_eq!(
        outcomes[0],
        (first.clone(), OutcomeSignal::JudgedFail),
        "the check that passed does not count for the model: {shown}"
    );
    let moved: Vec<&Route> = routes
        .iter()
        .filter(|route| route.reason == RouteReason::Escalate)
        .collect();
    assert!(!moved.is_empty(), "{shown}");
    assert_ne!(moved[0].model, first);
    assert_eq!(
        shown
            .matches("gate: sending the review's findings back to")
            .count(),
        1,
        "sent back once, on the model it started on: {shown}"
    );
    assert_eq!(
        outcomes
            .iter()
            .filter(|(_, signal)| *signal == OutcomeSignal::JudgedFail)
            .count(),
        2,
        "what the next model leaves unfinished moves on again: {outcomes:?}"
    );
}

#[tokio::test]
async fn a_logged_review_changes_nothing_about_the_turn() {
    let text = "fix it: run passing check (inspect: blame)";
    let (gate, _log_dir) = reviewing_gate(aura_config::ReviewMode::Log);
    let (routes, outcomes, shown, _) = debugging_turn_under(gate, text).await;
    assert_eq!(routes.len(), 1, "{shown}");
    assert_eq!(
        outcomes,
        [(routes[0].model.clone(), OutcomeSignal::ChecksPassed)]
    );
    assert!(
        shown.contains("work looks unfinished (logged only; AURA_GATE_REVIEW=on acts on it)."),
        "{shown}"
    );

    let (gate, _log_dir) = reviewing_gate(aura_config::ReviewMode::Off);
    let (routes, outcomes, shown, _) = debugging_turn_under(gate, text).await;
    assert_eq!(routes.len(), 1);
    assert_eq!(
        outcomes,
        [(routes[0].model.clone(), OutcomeSignal::ChecksPassed)]
    );
    assert!(!shown.contains("Gate's review"), "{shown}");
}

#[tokio::test]
async fn a_failed_turn_the_judge_excuses_ends_where_it_is() {
    const HAIKU: &str = "aura-claude-haiku-4-5";
    let (routes, outcomes, shown, rows) =
        debugging_turn_under_gate("fix it (easy): run failing check (judge: excuse)").await;

    assert_eq!(
        routes,
        [Route::turn(HAIKU, RouteReason::Default).at_effort("medium")]
    );
    assert_eq!(
        outcomes,
        [
            (HAIKU.to_string(), OutcomeSignal::ChecksFailed),
            (HAIKU.to_string(), OutcomeSignal::JudgedExcused),
        ]
    );
    assert!(
        shown.contains("the failure is not aura-claude-haiku-4-5 (cloud)'s doing"),
        "{shown}"
    );
    assert!(!shown.contains("escalate"), "{shown}");
    assert_eq!(rows, [None, None], "an excused failure counts for nothing");
}

#[tokio::test]
async fn a_failed_turn_with_no_verdict_carries_on_on_a_stronger_model() {
    const HAIKU: &str = "aura-claude-haiku-4-5";
    const SONNET: &str = "aura-claude-sonnet-4-6";
    let (routes, outcomes, _, rows) =
        debugging_turn_under_gate("fix it (easy): run failing check").await;

    assert_eq!(
        routes,
        [
            Route::turn(HAIKU, RouteReason::Default).at_effort("medium"),
            Route::turn(SONNET, RouteReason::Escalate).at_effort("medium"),
        ]
    );
    assert_eq!(
        outcomes,
        [
            (HAIKU.to_string(), OutcomeSignal::ChecksFailed),
            (SONNET.to_string(), OutcomeSignal::Unavailable),
        ],
        "the turn's own signal stands and is recorded once"
    );
    assert_eq!(rows, [Some((1, 0)), None]);
}
