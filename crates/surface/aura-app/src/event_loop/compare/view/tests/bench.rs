//! The BENCH tab's view of a comparison: its runs, its arms' sums, its
//! judge, a replayed session, example tasks and each status.

use super::{examples, payload, replay, routed, unpriced};
use aura_core_types::{AgentId, ComparePayload, CompareStatus};
use aura_ui_terminal::events::{BenchBaseline, BenchStatus, BenchView};

/// The BENCH tab's view of `payload`, held by an agent of its own in
/// another agent's tree.
fn bench_view(payload: &ComparePayload) -> BenchView {
    let (host, root) = (AgentId::new([0xc0; 32]), AgentId::new([0x0f; 32]));
    crate::event_loop::bench::view(host, root, "Compare #1", payload)
}

/// Each of `items` as `Debug` writes it on one line.
fn debug<T: std::fmt::Debug>(items: &[T]) -> Vec<String> {
    items.iter().map(|item| format!("{item:?}")).collect()
}

/// `view` without its arms, runs, tasks and judge.
fn rest(view: BenchView) -> BenchView {
    BenchView {
        arms: Default::default(),
        runs: Vec::new(),
        tasks: Vec::new(),
        judge: None,
        ..view
    }
}

#[test]
fn the_bench_tab_lists_each_run_as_it_ran() {
    let expected = [
        "BenchRun { name: \"a-1\", arm: 0, model: \"aura-claude-haiku-4-5\", \
         escalated_to: Some(\"aura-claude-sonnet-4-6\"), escalated_effort: Some(\"high\"), \
         category: Some(\"debugging\"), difficulty: Some(\"hard\"), \
         drivers: Some(\"a few files, loose spec, deep exploring, checkable, traps: concurrency\"), \
         led: false, \
         subtask_cost_usd: 0.0, wall_ms: 41000, cost_usd: 0.05, unpriced_calls: 0, check_passed: Some(true), \
         files_changed: 2, error: None, task: None, \
         agent_id: \"0101010101010101010101010101010101010101010101010101010101010101\" }",
        "BenchRun { name: \"b-1\", arm: 1, model: \"aura-claude-opus-4-7\", escalated_to: None, \
         escalated_effort: None, category: None, difficulty: None, drivers: None, led: false, \
         subtask_cost_usd: -0.0, wall_ms: 41000, cost_usd: 0.4, unpriced_calls: 0, check_passed: Some(true), \
         files_changed: 2, error: None, task: None, \
         agent_id: \"1111111111111111111111111111111111111111111111111111111111111111\" }",
        "BenchRun { name: \"a-2\", arm: 0, model: \"m\", escalated_to: None, \
         escalated_effort: None, category: Some(\"multi_file_change\"), difficulty: None, drivers: None, \
         led: true, subtask_cost_usd: 0.17, wall_ms: 42000, cost_usd: 0.07, unpriced_calls: 0, \
         check_passed: Some(true), files_changed: 2, error: None, task: None, \
         agent_id: \"0202020202020202020202020202020202020202020202020202020202020202\" }",
        "BenchRun { name: \"b-2\", arm: 1, model: \"aura-claude-opus-4-7\", escalated_to: None, \
         escalated_effort: None, category: None, difficulty: None, drivers: None, led: false, \
         subtask_cost_usd: -0.0, wall_ms: 42000, cost_usd: 0.5, unpriced_calls: 0, check_passed: None, \
         files_changed: 2, error: Some(\"the model fell over\\nand stayed down\"), task: None, \
         agent_id: \"1212121212121212121212121212121212121212121212121212121212121212\" }",
        "BenchRun { name: \"a-3\", arm: 0, model: \"aura-claude-sonnet-4-6\", \
         escalated_to: None, escalated_effort: None, category: None, difficulty: None, drivers: None, \
         led: false, subtask_cost_usd: -0.0, wall_ms: 43000, cost_usd: 0.06, unpriced_calls: 0, \
         check_passed: Some(false), files_changed: 2, error: None, task: None, \
         agent_id: \"0303030303030303030303030303030303030303030303030303030303030303\" }",
        "BenchRun { name: \"b-3\", arm: 1, model: \"aura-claude-opus-4-7\", escalated_to: None, \
         escalated_effort: None, category: None, difficulty: None, drivers: None, led: false, \
         subtask_cost_usd: -0.0, wall_ms: 43000, cost_usd: 0.45, unpriced_calls: 0, check_passed: Some(true), \
         files_changed: 2, error: None, task: None, \
         agent_id: \"1313131313131313131313131313131313131313131313131313131313131313\" }",
    ];
    assert_eq!(debug(&bench_view(&routed()).runs), expected);
}

#[test]
fn the_bench_tab_sums_up_each_arm() {
    let routed_arms = [
        "BenchArm { label: \"gate\", runs: 3, finished: 3, total_cost_usd: 0.18, \
         median_cost_usd: Some(0.06), unpriced_calls: 0, finished_unpriced_calls: 0, median_wall_ms: Some(42000), checks_passed: 2, \
         checks_run: 3, judge_wins: 0, judge_losses: 1, judge_ties: 0, ref_better: 0, \
         ref_as_good: 0, ref_worse: 0, tokens: [BenchTokens { model: \"claude-sonnet-4-6\", \
         purpose: None, calls: 9, input_tokens: 126000, output_tokens: 4500, cost_usd: 0.18, \
         unpriced_calls: 0 }], \
         turns: [BenchTurn { runs: 2, mean_cost_usd: Some(0.41000000000000003), unpriced_calls: 0, passed: 2, \
         failed: 0, errors: 0 }], escalated: 1, nudged: 1 }",
        "BenchArm { label: \"aura-claude-opus-4-7\", runs: 3, finished: 2, \
         total_cost_usd: 1.35, median_cost_usd: Some(0.42500000000000004), unpriced_calls: 0, finished_unpriced_calls: 0, \
         median_wall_ms: Some(42000), checks_passed: 2, checks_run: 2, judge_wins: 1, \
         judge_losses: 0, judge_ties: 0, ref_better: 0, ref_as_good: 0, ref_worse: 0, \
         tokens: [BenchTokens { model: \"claude-opus-4-7\", purpose: None, calls: 9, \
         input_tokens: 126000, output_tokens: 4500, cost_usd: 1.35, unpriced_calls: 0 }], \
         turns: [BenchTurn { runs: 0, \
         mean_cost_usd: None, unpriced_calls: 0, passed: 0, failed: 0, errors: 0 }], escalated: 0, nudged: 0 }",
    ];
    assert_eq!(debug(&bench_view(&routed()).arms), routed_arms);
    let replayed_arms = [
        "BenchArm { label: \"gate\", runs: 3, finished: 3, total_cost_usd: 0.18, \
         median_cost_usd: Some(0.06), unpriced_calls: 0, finished_unpriced_calls: 0, median_wall_ms: Some(42000), checks_passed: 2, \
         checks_run: 3, judge_wins: 0, judge_losses: 1, judge_ties: 0, ref_better: 0, \
         ref_as_good: 0, ref_worse: 1, tokens: [BenchTokens { model: \"claude-sonnet-4-6\", \
         purpose: None, calls: 9, input_tokens: 126000, output_tokens: 4500, cost_usd: 0.18, \
         unpriced_calls: 0 }], \
         turns: [BenchTurn { runs: 3, mean_cost_usd: Some(0.02), unpriced_calls: 0, passed: 3, failed: 0, \
         errors: 0 }, BenchTurn { runs: 3, mean_cost_usd: Some(0.04), unpriced_calls: 0, passed: 0, failed: 0, \
         errors: 3 }], escalated: 0, nudged: 0 }",
        "BenchArm { label: \"aura-claude-opus-4-7\", runs: 3, finished: 3, \
         total_cost_usd: 1.35, median_cost_usd: Some(0.45), unpriced_calls: 0, finished_unpriced_calls: 0, median_wall_ms: Some(42000), \
         checks_passed: 3, checks_run: 3, judge_wins: 1, judge_losses: 0, judge_ties: 0, \
         ref_better: 1, ref_as_good: 0, ref_worse: 0, \
         tokens: [BenchTokens { model: \"claude-opus-4-7\", purpose: None, calls: 9, \
         input_tokens: 126000, output_tokens: 4500, cost_usd: 1.35, unpriced_calls: 0 }], \
         turns: [BenchTurn { runs: 3, \
         mean_cost_usd: Some(0.02), unpriced_calls: 0, passed: 3, failed: 0, errors: 0 }, BenchTurn { runs: 3, \
         mean_cost_usd: Some(0.04), unpriced_calls: 0, passed: 0, failed: 0, errors: 3 }], escalated: 0, \
         nudged: 0 }",
    ];
    assert_eq!(debug(&bench_view(&replay()).arms), replayed_arms);
}

#[test]
fn the_bench_tab_counts_the_calls_with_no_known_price() {
    let view = bench_view(&unpriced());
    let b = &view.arms[1];
    assert_eq!((b.unpriced_calls, b.finished_unpriced_calls), (7, 7));
    assert_eq!(b.tokens[0].unpriced_calls, 7);
    assert_eq!(view.arms[0].unpriced_calls, 0);
    let unpriced: Vec<(&str, u32)> = (view.runs.iter())
        .map(|run| (run.name.as_str(), run.unpriced_calls))
        .collect();
    assert_eq!(
        unpriced,
        [
            ("a-1", 0),
            ("b-1", 3),
            ("a-2", 0),
            ("b-2", 3),
            ("a-3", 0),
            ("b-3", 1)
        ]
    );
}

#[test]
fn the_bench_tab_names_a_comparison_and_its_judge() {
    let view = bench_view(&routed());
    let judge = [
        "Some(BenchJudge { model: \"aura-claude-opus-4-7\", cost_usd: 0.03, unpriced_calls: 0, \
         verdicts: [BenchVerdict { a_run: \"a-1\", b_run: \"b-1\", parsed: true, \
         winner: Some(1), a_vs_reference: None, b_vs_reference: None, reasoning: \"\" }] })",
    ];
    assert_eq!(debug(&[&view.judge]), judge);
    let expected = BenchView {
        id: "c0".repeat(32),
        root: "0f".repeat(32),
        name: "Compare #1".to_string(),
        title: "fix the flaky parser test".to_string(),
        source: "prompt".to_string(),
        status: BenchStatus::Cancelled,
        repeats: 3,
        check: Some("cargo test".to_string()),
        prompts: vec!["fix the flaky parser test\nmore detail".to_string()],
        notes: vec!["the budget stopped 2 runs".to_string()],
        classifier: "aura-claude-haiku-4-5".to_string(),
        ..BenchView::default()
    };
    assert_eq!(rest(view), expected);
}

#[test]
fn the_bench_tab_shows_a_replayed_session_with_what_it_cost() {
    let view = bench_view(&replay());
    let judge = [
        "Some(BenchJudge { model: \"aura-claude-opus-4-7\", cost_usd: 0.03, unpriced_calls: 0, \
         verdicts: [BenchVerdict { a_run: \"a-1\", b_run: \"b-1\", parsed: true, \
         winner: Some(1), a_vs_reference: Some(\"worse\"), b_vs_reference: Some(\"better\"), \
         reasoning: \"\" }] })",
    ];
    assert_eq!(debug(&[&view.judge]), judge);
    let expected = BenchView {
        id: "c0".repeat(32),
        root: "0f".repeat(32),
        name: "Compare #1".to_string(),
        title: "fix the parser".to_string(),
        source: "session \"fix the parser\"".to_string(),
        status: BenchStatus::Done,
        repeats: 3,
        check: Some("cargo test".to_string()),
        prompts: vec!["fix the parser".to_string(), "now the docs".to_string()],
        baseline: Some(BenchBaseline {
            calls: 12,
            cost_usd: 0.9,
            priced: true,
            unpriced_calls: 0,
            models: vec!["claude-opus-4-7".to_string()],
        }),
        has_reference: true,
        notes: vec!["the budget stopped 2 runs".to_string()],
        classifier: "aura-claude-haiku-4-5".to_string(),
        ..BenchView::default()
    };
    assert_eq!(rest(view), expected);
}

#[test]
fn the_bench_tab_has_a_row_per_example_task() {
    let view = bench_view(&examples());
    let tasks = [
        "BenchTaskRow { id: \"coding/write-file\", title: \"coding/write-file\", \
         arms: [BenchTaskCell { runs: 2, passed: 1, checked: 2, \
         mean_cost_usd: Some(0.055), unpriced_calls: 0 }, BenchTaskCell { runs: 2, passed: 2, checked: 2, \
         mean_cost_usd: Some(0.42500000000000004), unpriced_calls: 0 }], setup: Ready }",
        "BenchTaskRow { id: \"coding/fix-shell-bug\", title: \"coding/fix-shell-bug\", \
         arms: [BenchTaskCell { runs: 1, passed: 1, checked: 1, mean_cost_usd: Some(0.07), unpriced_calls: 0 }, \
         BenchTaskCell { runs: 1, passed: 1, checked: 1, mean_cost_usd: Some(0.5), unpriced_calls: 0 }], \
         setup: Ready }",
    ];
    assert_eq!(debug(&view.tasks), tasks);
    let worked_on: Vec<(&str, Option<&str>)> = (view.runs.iter())
        .map(|run| (run.name.as_str(), run.task.as_deref()))
        .collect();
    let (write, fix) = (Some("coding/write-file"), Some("coding/fix-shell-bug"));
    let order = [
        ("a-1", write),
        ("b-1", write),
        ("a-2", fix),
        ("b-2", fix),
        ("a-3", write),
        ("b-3", write),
    ];
    assert_eq!(worked_on, order);
    let expected = BenchView {
        id: "c0".repeat(32),
        root: "0f".repeat(32),
        name: "Compare #1".to_string(),
        title: "2 example coding tasks".to_string(),
        source: "2 example tasks".to_string(),
        status: BenchStatus::Done,
        repeats: 3,
        notes: vec!["the budget stopped 2 runs".to_string()],
        classifier: "aura-claude-haiku-4-5".to_string(),
        ..BenchView::default()
    };
    assert_eq!(rest(view), expected);
}

#[test]
fn the_bench_tab_names_a_plans_work_and_each_status() {
    let mut planned = payload();
    planned.plan = Some("plans/parser.md".to_string());
    planned.task = "Build the plan\nin plans/parser.md".to_string();
    planned.budget_usd = Some(2.5);
    let view = rest(bench_view(&planned));
    assert_eq!(
        (view.title.as_str(), view.source.as_str(), view.budget_usd),
        ("Build the plan", "plan plans/parser.md", Some(2.5))
    );
    let applied = || CompareStatus::Applied("a-2".to_string());
    for (status, shown) in [
        (CompareStatus::Running, BenchStatus::Running),
        (CompareStatus::Done, BenchStatus::Done),
        (CompareStatus::Cancelled, BenchStatus::Cancelled),
        (applied(), BenchStatus::Applied("a-2".to_string())),
        (CompareStatus::Dismissed, BenchStatus::Dismissed),
    ] {
        let payload = ComparePayload {
            status,
            ..payload()
        };
        assert_eq!(bench_view(&payload).status, shown);
    }
}
