use super::*;
use crate::run_name;
use aura_core_types::{
    AgentId, ArmSpec, BenchSource, CompareArm, CompareCopy, CompareJudgment, CompareRun,
    CompareStatus, ExampleTaskInfo, ModelTokens, OutcomeSignal, RefVerdict, RunTurn, SystemKind,
};
use std::path::PathBuf;

fn run(arm: CompareArm, n: u32, wall_ms: u64, cost: f64, check: Option<bool>) -> CompareRun {
    CompareRun {
        name: run_name(arm, n),
        arm,
        agent_id: AgentId::generate(),
        prompt_tx: None,
        model: "m".to_string(),
        copy: CompareCopy {
            root: PathBuf::new(),
            workdir: PathBuf::new(),
            git_dir: None,
            baseline: String::new(),
        },
        wall_ms,
        cost_usd: cost,
        tokens: vec![ModelTokens {
            model: format!("model-{arm}"),
            calls: 2,
            input_tokens: 100,
            cost_usd: cost,
            ..ModelTokens::default()
        }],
        turns: Vec::new(),
        plan_usage: Vec::new(),
        check_passed: check,
        files_changed: 1,
        error: None,
        task: None,
    }
}

fn judged(a_run: &str, winner: Option<CompareArm>, parsed: bool) -> CompareJudgment {
    CompareJudgment {
        a_run: a_run.to_string(),
        b_run: a_run.replacen('a', "b", 1),
        a_first: true,
        winner,
        parsed,
        a_vs_reference: None,
        b_vs_reference: None,
        model: "judge".to_string(),
        cost_usd: 0.01,
        unpriced_calls: 0,
        reasoning: String::new(),
    }
}

fn turn(cost: f64, signal: OutcomeSignal) -> RunTurn {
    RunTurn::new("m", cost, 1, signal)
}

/// A finished comparison of one prompt with `runs` and `judgments`.
fn payload(runs: Vec<CompareRun>, judgments: Vec<CompareJudgment>) -> ComparePayload {
    ComparePayload {
        system_kind: SystemKind::Compare,
        task: "fix it".to_string(),
        plan: None,
        arm_a: ArmSpec::Gate { delegate: true },
        arm_b: ArmSpec::from_label("opus"),
        against: "opus".to_string(),
        prompts: vec!["fix it".to_string()],
        source: BenchSource::Prompt,
        repeats: 3,
        check: Some("true".to_string()),
        budget_usd: None,
        classifier: String::new(),
        requested_by: AgentId::generate(),
        apply_root: PathBuf::new(),
        started_ms: 0,
        runs,
        judgments,
        notes: Vec::new(),
        status: CompareStatus::Done,
    }
}

#[test]
fn medians_take_the_middle_or_the_mean_of_the_middle_two() {
    assert_eq!(median(&[]), None);
    assert_eq!(median(&[3.0]), Some(3.0));
    assert_eq!(median(&[9.0, 1.0, 5.0]), Some(5.0));
    assert_eq!(median(&[4.0, 1.0, 3.0, 10.0]), Some(3.5));
}

/// Three runs per arm, `b-3` failed, and four judgments, the last one
/// unparsed.
fn judged_payload() -> ComparePayload {
    let mut failed = run(CompareArm::B, 3, 1, 0.5, None);
    failed.error = Some("model fell over".to_string());
    let runs = vec![
        run(CompareArm::A, 1, 30_000, 0.10, Some(true)),
        run(CompareArm::B, 1, 60_000, 0.90, Some(true)),
        run(CompareArm::A, 2, 10_000, 0.20, Some(false)),
        run(CompareArm::B, 2, 50_000, 1.10, Some(true)),
        run(CompareArm::A, 3, 20_000, 0.30, Some(true)),
        failed,
    ];
    let judgments = vec![
        judged("a-1", Some(CompareArm::A), true),
        judged("a-2", Some(CompareArm::B), true),
        judged("a-3", None, true),
        judged("a-4", Some(CompareArm::A), false),
    ];
    payload(runs, judgments)
}

#[test]
fn summaries_give_medians_pass_counts_and_judge_tallies_per_arm() {
    let gate = summarize(&judged_payload(), CompareArm::A);
    assert_eq!((gate.runs, gate.finished), (3, 3));
    assert_eq!(gate.median_wall_ms, Some(20_000));
    assert!((gate.total_cost_usd - 0.60).abs() < 1e-9);
    assert!((gate.median_cost_usd.unwrap() - 0.20).abs() < 1e-9);
    assert_eq!((gate.checks_passed, gate.checks_run), (2, 3));
    assert_eq!(
        (gate.judge_wins, gate.judge_losses, gate.judge_ties),
        (1, 1, 1)
    );
    assert_eq!(gate.tokens.len(), 1);
    assert_eq!(gate.tokens[0].calls, 6);
    assert!(gate.plan_usage.is_empty());
    assert_eq!(
        (gate.ref_better, gate.ref_as_good, gate.ref_worse),
        (0, 0, 0)
    );
}

#[test]
fn a_failed_run_counts_in_the_cost_but_not_the_medians() {
    let payload = judged_payload();
    let other = summarize(&payload, CompareArm::B);
    assert_eq!((other.runs, other.finished), (3, 2));
    assert_eq!(
        other.median_wall_ms,
        Some(55_000),
        "the failed run is left out"
    );
    assert!(
        (other.total_cost_usd - 2.50).abs() < 1e-9,
        "but its cost counts"
    );
    assert_eq!((other.checks_passed, other.checks_run), (2, 2));
    assert_eq!((other.judge_wins, other.judge_losses), (1, 1));
    assert!((judge_cost_usd(&payload) - 0.04).abs() < 1e-9);
    let gate = summarize(&payload, CompareArm::A);
    assert!(too_close(gate.checks_passed, other.checks_passed));
    assert!(!too_close(3, 1));
}

/// A replay of two prompts: `a-1` and `a-2` got to both and `b-1`
/// stopped after the first; the judge rated the pairs against the
/// reference.
fn replay_payload() -> ComparePayload {
    let mut a1 = run(CompareArm::A, 1, 1, 0.3, None);
    a1.turns = vec![
        turn(0.1, OutcomeSignal::ChecksPassed),
        turn(0.2, OutcomeSignal::ChecksFailed),
    ];
    let mut a2 = run(CompareArm::A, 2, 1, 0.5, None);
    a2.turns = vec![
        turn(0.3, OutcomeSignal::ChecksPassed),
        turn(0.2, OutcomeSignal::Error),
    ];
    let mut b1 = run(CompareArm::B, 1, 1, 0.9, None);
    b1.turns = vec![turn(0.9, OutcomeSignal::Unavailable)];
    let mut first = judged("a-1", Some(CompareArm::A), true);
    first.a_vs_reference = Some(RefVerdict::AsGood);
    first.b_vs_reference = Some(RefVerdict::Worse);
    let mut second = judged("a-2", None, true);
    second.a_vs_reference = Some(RefVerdict::Better);
    ComparePayload {
        prompts: vec!["one".to_string(), "two".to_string()],
        ..payload(vec![a1, b1, a2], vec![first, second])
    }
}

#[test]
fn replays_show_each_prompts_cost_and_outcome_per_arm() {
    let payload = replay_payload();
    let a = turn_cells(&payload, CompareArm::A);
    assert_eq!(a.len(), 2);
    assert_eq!((a[0].runs, a[0].passed, a[0].failed), (2, 2, 0));
    assert!((a[0].mean_cost_usd.unwrap() - 0.2).abs() < 1e-9);
    assert_eq!((a[1].passed, a[1].failed, a[1].errors), (0, 1, 1));
    let b = turn_cells(&payload, CompareArm::B);
    assert_eq!(b[0].runs, 1);
    assert_eq!(b[1].runs, 0, "b-1 stopped after the first prompt");
    assert_eq!(b[1].mean_cost_usd, None);
}

#[test]
fn replays_tally_how_each_arm_did_against_the_reference() {
    let payload = replay_payload();
    let a = summarize(&payload, CompareArm::A);
    assert_eq!((a.ref_better, a.ref_as_good, a.ref_worse), (1, 1, 0));
    let b = summarize(&payload, CompareArm::B);
    assert_eq!((b.ref_better, b.ref_as_good, b.ref_worse), (0, 0, 1));
}

/// Two example tasks: arm A ran `coding/one` twice and `coding/two`
/// once, arm B ran `coding/one` once.
fn examples_payload() -> ComparePayload {
    let on = |mut run: CompareRun, task: &str| {
        run.task = Some(task.to_string());
        run
    };
    let task = |id: &str| ExampleTaskInfo {
        id: id.to_string(),
        ..ExampleTaskInfo::default()
    };
    let runs = vec![
        on(run(CompareArm::A, 1, 1, 0.2, Some(true)), "coding/one"),
        on(run(CompareArm::B, 1, 1, 0.8, Some(true)), "coding/one"),
        on(run(CompareArm::A, 2, 1, 0.4, Some(false)), "coding/two"),
        on(run(CompareArm::A, 3, 1, 0.4, Some(true)), "coding/one"),
    ];
    ComparePayload {
        prompts: Vec::new(),
        source: BenchSource::Examples {
            tasks: vec![task("coding/one"), task("coding/two")],
        },
        ..payload(runs, Vec::new())
    }
}

#[test]
fn example_tasks_count_passes_per_task_and_arm() {
    let payload = examples_payload();
    let a = task_cells(&payload, CompareArm::A);
    assert_eq!(a.len(), 2);
    assert_eq!((a[0].runs, a[0].passed, a[0].checked), (2, 2, 2));
    assert!((a[0].mean_cost_usd.unwrap() - 0.3).abs() < 1e-9);
    assert_eq!((a[1].runs, a[1].passed, a[1].checked), (1, 0, 1));
    let b = task_cells(&payload, CompareArm::B);
    assert_eq!((b[1].runs, b[1].mean_cost_usd), (0, None));
    let prompt = ComparePayload {
        source: BenchSource::Prompt,
        ..payload
    };
    assert!(task_cells(&prompt, CompareArm::A).is_empty());
}

/// `run` with `unpriced` more calls, to a model with no known price.
fn with_unpriced(mut run: CompareRun, unpriced: u32) -> CompareRun {
    run.tokens.push(ModelTokens {
        model: "mystery-model".to_string(),
        calls: unpriced,
        unpriced_calls: unpriced,
        ..ModelTokens::default()
    });
    run
}

#[test]
fn unpriced_calls_are_counted_per_arm_finished_runs_and_cells() {
    let mut payload = judged_payload();
    // b-3, the failed run, called a model with no known price.
    payload.runs[5] = with_unpriced(payload.runs[5].clone(), 2);
    payload.runs[0] = with_unpriced(payload.runs[0].clone(), 1);
    let a = summarize(&payload, CompareArm::A);
    assert_eq!((a.unpriced_calls, a.finished_unpriced_calls), (1, 1));
    let b = summarize(&payload, CompareArm::B);
    assert_eq!(
        (b.unpriced_calls, b.finished_unpriced_calls),
        (2, 0),
        "a failed run's calls leave the median complete"
    );
    let mystery = b
        .tokens
        .iter()
        .find(|t| t.model == "mystery-model")
        .unwrap();
    assert_eq!((mystery.calls, mystery.unpriced_calls), (2, 2));
    assert!((b.total_cost_usd - 2.50).abs() < 1e-9, "priced part only");

    let mut examples = examples_payload();
    examples.runs[2] = with_unpriced(examples.runs[2].clone(), 3);
    let cells = task_cells(&examples, CompareArm::A);
    assert_eq!((cells[0].unpriced_calls, cells[1].unpriced_calls), (0, 3));

    let mut replay = replay_payload();
    replay.runs[0].turns[1].unpriced_calls = 2;
    let turns = turn_cells(&replay, CompareArm::A);
    assert_eq!((turns[0].unpriced_calls, turns[1].unpriced_calls), (0, 2));
}
