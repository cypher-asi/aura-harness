//! The one retry of a failed subtask on a stronger model: what each
//! attempt records and what Gate counts of them, what the judge's review
//! of the failure changes, the tier a frontier lead's subtasks stay
//! under, and the strongest model's failed checks, which are not retried.

use super::*;
use aura_core_types::SubagentExit;

#[tokio::test]
async fn a_failed_check_undoes_the_attempt_and_retries_once_on_a_stronger_model() {
    let (fixture, _commands) = fixture();
    let mut b = borrowed(&fixture);
    let mut state = loop_state(&fixture, &mut b);
    let (result, _) = retried_once(&fixture, &mut state).await;

    assert!(matches!(result.exit, SubagentExit::Completed));
    let attempts = children(&fixture, fixture.first, "implement");
    assert_eq!(attempts.len(), 2, "one retry");
    let notes: Vec<String> = std::fs::read_dir(fixture.workspace.join("notes"))
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    assert_eq!(
        notes,
        [format!("{GPT}.txt")],
        "the first attempt's note was removed before the retry"
    );
    let (prompts, outcomes) = record(&fixture, attempts[0]);
    assert_eq!(
        prompts[0].route,
        Route::subtask(SONNET, RouteReason::Default).at_effort("medium")
    );
    assert_eq!(outcomes[0].signal, OutcomeSignal::ChecksFailed);
}

#[tokio::test]
async fn the_retry_is_recorded_as_an_escalation_told_how_the_first_attempt_failed() {
    let (fixture, _commands) = fixture();
    let mut b = borrowed(&fixture);
    let mut state = loop_state(&fixture, &mut b);
    let (_, check) = retried_once(&fixture, &mut state).await;

    let attempts = children(&fixture, fixture.first, "implement");
    let (prompts, outcomes) = record(&fixture, attempts[1]);
    assert_eq!(
        prompts[0].route,
        Route::subtask(GPT, RouteReason::Escalate).at_effort("medium")
    );
    assert_eq!(prompts[0].category, Some(PromptCategory::Question));
    assert!(prompts[0]
        .text
        .starts_with("write a note\n\n---\nA first attempt"));
    assert!(
        prompts[0]
            .text
            .contains(&format!("Check `{check}`: failed")),
        "the retry sees the failure: {}",
        prompts[0].text
    );
    assert_eq!(outcomes[0].signal, OutcomeSignal::ChecksPassed);
    assert_eq!(outcomes[0].model, GPT);
}

#[tokio::test]
async fn gate_counts_the_first_attempt_of_a_retried_subtask_and_not_the_retry() {
    let (fixture, _commands) = fixture();
    let mut b = borrowed(&fixture);
    let mut state = loop_state(&fixture, &mut b);
    retried_once(&fixture, &mut state).await;

    let stats = fixture.hub.gate.stats();
    let row = |model| {
        stats
            .row(RouteLevel::Subtask, PromptCategory::Question, model)
            .map(|r| (r.tries, r.successes))
    };
    assert_eq!(row(SONNET), Some((1, 0)), "the first attempt counts");
    assert_eq!(row(GPT), None, "the retry does not");
}

#[tokio::test]
async fn the_lead_is_told_why_the_subtask_was_retried_and_how_the_retry_went() {
    let (fixture, mut commands) = fixture();
    let mut b = borrowed(&fixture);
    let mut state = loop_state(&fixture, &mut b);
    let (result, _) = retried_once(&fixture, &mut state).await;

    assert!(
        result.final_message.contains(&format!(
            "Retried once: the first attempt on {SONNET} failed"
        )),
        "{}",
        result.final_message
    );
    assert!(result
        .final_message
        .contains("Check `ls notes/aura-gpt-5-4.txt`: passed"));
    let attempts = children(&fixture, fixture.first, "implement");
    assert_eq!(result.child_agent_id, Some(attempts[1]));
    assert!(route_lines(&mut commands)
        .iter()
        .any(|line| line.ends_with("subtask, question, escalate")));
}

/// Dispatch a subtask asking `prompt` whose check fails on its first
/// attempt (on Sonnet), and return its attempts, oldest first.
async fn failing_first_attempt(fixture: &Fixture, prompt: &str) -> (SubagentResult, Vec<AgentId>) {
    let first = fixture.first;
    let check = format!("ls notes/{GPT}.txt");
    let result = dispatch(fixture, task(first, "implement", prompt, Some(&check))).await;
    (result, children(fixture, first, "implement"))
}

#[tokio::test]
async fn a_failed_subtask_the_judge_excuses_is_not_retried_or_counted() {
    let (fixture, _commands) = fixture();
    let mut b = borrowed(&fixture);
    let first = fixture.first;
    let mut state = loop_state(&fixture, &mut b);
    select_gate(&fixture, &mut state);
    fixture.hub.set_permissions(first, yolo());

    let (result, attempts) = failing_first_attempt(&fixture, "write a note (judge: excuse)").await;

    assert!(matches!(result.exit, SubagentExit::Completed));
    assert_eq!(attempts.len(), 1, "no retry on a stronger model");
    let (_, outcomes) = record(&fixture, attempts[0]);
    let signals: Vec<OutcomeSignal> = outcomes.iter().map(|o| o.signal).collect();
    assert_eq!(
        signals,
        [OutcomeSignal::ChecksFailed, OutcomeSignal::JudgedExcused]
    );
    assert_eq!(
        fixture
            .hub
            .gate
            .stats()
            .row(RouteLevel::Subtask, PromptCategory::Question, SONNET),
        None,
        "an excused failure counts for nothing"
    );
}

#[tokio::test]
async fn a_failed_subtask_the_judge_blames_is_retried_with_what_the_judge_found() {
    let (fixture, _commands) = fixture();
    let mut b = borrowed(&fixture);
    let first = fixture.first;
    let mut state = loop_state(&fixture, &mut b);
    select_gate(&fixture, &mut state);
    fixture.hub.set_permissions(first, yolo());

    let (_, attempts) = failing_first_attempt(&fixture, "write a note (judge: blame)").await;

    assert_eq!(attempts.len(), 2, "one retry");
    let (_, outcomes) = record(&fixture, attempts[0]);
    let signals: Vec<OutcomeSignal> = outcomes.iter().map(|o| o.signal).collect();
    assert_eq!(
        signals,
        [OutcomeSignal::ChecksFailed, OutcomeSignal::JudgedFail]
    );
    let (prompts, _) = record(&fixture, attempts[1]);
    assert_eq!(
        prompts[0].route,
        Route::subtask(GPT, RouteReason::Escalate).at_effort("medium")
    );
    assert!(
        prompts[0]
            .text
            .contains("A reviewer of that attempt found: The change is wrong."),
        "{}",
        prompts[0].text
    );
    let row = fixture
        .hub
        .gate
        .stats()
        .row(RouteLevel::Subtask, PromptCategory::Question, SONNET)
        .map(|r| (r.tries, r.successes));
    assert_eq!(row, Some((1, 0)), "the failure counts once");
}

/// Gate has seen two planning subtasks fail on Sonnet, which rules it
/// out of planning subtasks.
fn sonnet_failed_at_planning(fixture: &Fixture) {
    fixture.hub.gate.learn(
        RouteLevel::Subtask,
        (PromptCategory::Planning, None),
        SONNET,
        None,
        (2, false),
    );
}

#[tokio::test]
async fn a_frontier_leads_subtasks_start_below_its_tier_and_may_escalate_to_it() {
    let (fixture, _commands) = fixture();
    let mut b = borrowed(&fixture);
    let first = fixture.first;
    let mut state = loop_state(&fixture, &mut b);
    select_gate(&fixture, &mut state);
    fixture.hub.set_permissions(first, yolo());
    // Sonnet has failed at planning, so on its own this subtask would run
    // on the strongest model, the lead's.
    sonnet_failed_at_planning(&fixture);
    fixture.hub.gate.note_model(first, GPT);

    let check = format!("ls notes/{GPT}.txt");
    let result = dispatch(
        &fixture,
        task(first, "implement", "plan: write a note", Some(&check)),
    )
    .await;

    let attempts = children(&fixture, first, "implement");
    assert_eq!(attempts.len(), 2, "{}", result.final_message);
    let (prompts, outcomes) = record(&fixture, attempts[0]);
    assert_eq!(prompts[0].category, Some(PromptCategory::Planning));
    assert_eq!(
        prompts[0].route,
        Route::subtask(SONNET, RouteReason::Default).at_effort("medium"),
        "the strongest model below the lead's tier"
    );
    assert_eq!(outcomes[0].signal, OutcomeSignal::ChecksFailed);
    let (prompts, _) = record(&fixture, attempts[1]);
    assert_eq!(
        prompts[0].route,
        Route::subtask(GPT, RouteReason::Escalate).at_effort("medium"),
        "the retry may go up to the lead's tier"
    );
    assert!(result
        .final_message
        .contains(&format!("Check `{check}`: passed")));
}

#[tokio::test]
async fn once_the_leads_work_ends_its_subtasks_are_not_held_below_its_tier() {
    let (fixture, _commands) = fixture();
    let mut b = borrowed(&fixture);
    let first = fixture.first;
    let mut state = loop_state(&fixture, &mut b);
    select_gate(&fixture, &mut state);
    fixture.hub.set_permissions(first, yolo());
    sonnet_failed_at_planning(&fixture);
    fixture.hub.gate.note_model(first, GPT);

    fixture.hub.stop_running(first);
    assert_eq!(
        fixture.hub.gate.model_of(first),
        None,
        "forgotten once the lead's work ends"
    );
    dispatch(
        &fixture,
        task(first, "implement", "plan: write a note", None),
    )
    .await;
    let attempts = children(&fixture, first, "implement");
    let (prompts, _) = record(&fixture, attempts[0]);
    assert_eq!(
        prompts[0].route,
        Route::subtask(GPT, RouteReason::Default).at_effort("medium")
    );
}

/// Its own failing check says too little to pay for the work twice: the
/// check may fail without the change too. Only work a judge found
/// unfinished is done again on the strongest model.
#[tokio::test]
async fn on_the_strongest_model_a_failed_check_is_not_retried() {
    let (fixture, _commands) = fixture();
    let mut b = borrowed(&fixture);
    let first = fixture.first;
    let mut state = loop_state(&fixture, &mut b);
    select_gate(&fixture, &mut state);
    fixture.hub.set_permissions(first, yolo());
    sonnet_failed_at_planning(&fixture);

    let result = dispatch(
        &fixture,
        task(
            first,
            "implement",
            "plan: write a note",
            Some("ls no-such-dir"),
        ),
    )
    .await;

    let attempts = children(&fixture, first, "implement");
    assert_eq!(attempts.len(), 1, "{}", result.final_message);
    let (prompts, outcomes) = record(&fixture, attempts[0]);
    assert_eq!(
        prompts[0].route,
        Route::subtask(GPT, RouteReason::Default).at_effort("medium"),
        "with Sonnet sent up, planning goes to the strongest model"
    );
    assert_eq!(outcomes[0].signal, OutcomeSignal::ChecksFailed);
    assert!(
        fixture.workspace.join(format!("notes/{GPT}.txt")).exists(),
        "the failure is left for the lead to see"
    );
}
