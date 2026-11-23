//! The view of one comparison: its arms side by side, its runs and how
//! they were routed, plan usage, a replayed session's prompts and example
//! tasks, each also pinned byte for byte.

use super::super::render;
use super::{examples, line, payload, replay, routed, share};
use aura_core_types::{
    AgentId, BenchSource, ComparePayload, Difficulty, OutcomeSignal, Plan, PromptCategory,
    RefVerdict, RunTurn, SessionBaseline,
};

#[test]
fn the_view_shows_both_arms_side_by_side() {
    let text = render("Compare #1", &payload());
    assert!(text.starts_with("Compare #1 · \"fix the flaky parser test\" · done"));
    assert!(
        text.contains("A gate vs B aura-claude-opus-4-7 (cloud), 3 runs each"),
        "{text}"
    );
    assert_eq!(line(&text, "median time"), ["median time", "42s", "42s"]);
    assert_eq!(
        line(&text, "total cost"),
        ["total cost", "$0.18 · 87% less", "$1.35 · 100%"]
    );
    assert_eq!(
        line(&text, "median cost"),
        ["median cost", "$0.06 · 87% less", "$0.45 · 100%"]
    );
    assert_eq!(line(&text, "check"), ["check", "2/3 passed", "3/3 passed"]);
    assert_eq!(
        line(&text, "cost per fix"),
        ["cost per fix", "$0.09 · 80% less", "$0.45 · 100%"]
    );
    assert_eq!(
        line(&text, "judge "),
        ["judge", "0 won, 1 lost, 0 tied", "1 won, 0 lost, 0 tied"]
    );
    assert!(
        text.contains("Too close to call with 3 runs: check passes"),
        "{text}"
    );
    assert!(
        text.contains("A: claude-sonnet-4-6 9 calls, 126k in / 5k out, $0.18"),
        "{text}"
    );
    assert!(text.contains("Plan usage: no call reported a plan's usage"));
    assert!(!text.contains("share of each window"));
    assert!(
        text.contains("Judge: 1 blind call on aura-claude-opus-4-7 (cloud), $0.03"),
        "{text}"
    );
    assert!(text.contains("Note: the budget stopped 2 runs"));
    assert!(text.contains("/compare diff [run] · "));
    assert!(!text.contains("Per prompt"), "{text}");
    assert!(!text.contains("vs yours"), "{text}");
    let a_3 = line(&text, "a-3");
    assert!(a_3.contains(&"check failed"), "{a_3:?}");
    assert!(
        !text.contains(" · direct"),
        "older runs say nothing of routing"
    );
}

#[test]
fn the_heading_names_the_classifier_its_record_names() {
    let text = render("Compare #1", &payload());
    assert!(
        text.contains(", check `cargo test`, classifier aura-claude-haiku-4-5 (cloud)\n"),
        "{text}"
    );
    let older = ComparePayload {
        classifier: String::new(),
        ..payload()
    };
    assert!(
        !render("Compare #1", &older).contains("classifier"),
        "older records name no classifier"
    );
}

#[test]
fn each_run_says_how_its_turns_were_routed() {
    let mut payload = payload();
    payload.runs[0].turns = vec![RunTurn {
        category: Some(PromptCategory::Debugging),
        difficulty: Some(Difficulty::Hard),
        led: Some(false),
        escalations: 1,
        nudges: 1,
        ..RunTurn::new("m", 0.05, 1, OutcomeSignal::ChecksPassed)
    }];
    payload.runs[2].turns = vec![
        RunTurn {
            category: Some(PromptCategory::MultiFileChange),
            led: Some(true),
            subtask_cost_usd: 0.17,
            ..RunTurn::new("m", 0.77, 1, OutcomeSignal::ChecksPassed)
        },
        RunTurn {
            category: Some(PromptCategory::Question),
            led: Some(false),
            ..RunTurn::new("m", 0.03, 1, OutcomeSignal::JudgedPass)
        },
    ];
    let text = render("Compare #1", &payload);
    assert!(
        line(&text, "a-1").contains(&"· debugging/hard, direct, asked to check 1×, moved up 1×"),
        "{text}"
    );
    assert!(
        line(&text, "a-2").contains(
            &"· multi_file_change, question, 1 of 2 turns led: lead $0.63 + subtasks $0.17"
        ),
        "{text}"
    );
}

#[test]
fn plan_shares_per_arm_replace_the_note_when_a_plan_reported_usage() {
    let mut with_plan = payload();
    with_plan.runs[0].plan_usage = vec![
        share(Plan::Claude, "5h", 0.021),
        share(Plan::Claude, "7d_opus", 0.002),
    ];
    with_plan.runs[2].plan_usage = vec![share(Plan::Claude, "5h", 0.010)];
    with_plan.runs[1].plan_usage = vec![
        share(Plan::Claude, "5h", 0.090),
        share(Plan::Chatgpt, "5h", 0.012),
    ];
    let text = render("Compare #1", &with_plan);
    assert!(
        text.contains("A: Claude plan 5h window +3.1%, 7d opus window +0.2%"),
        "{text}"
    );
    assert!(
        text.contains("B: Claude plan 5h window +9.0%; ChatGPT plan 5h window +1.2%"),
        "{text}"
    );
    assert!(text.contains("don't count it twice"), "{text}");
    assert!(!text.contains("no call reported a plan's usage"), "{text}");

    let mut one_arm = payload();
    one_arm.runs[0].plan_usage = vec![share(Plan::Claude, "5h", 0.05)];
    let text = render("Compare #1", &one_arm);
    assert!(text.contains("B: no plan usage reported"), "{text}");
}

#[test]
fn a_replayed_session_shows_each_prompt_its_cost_and_the_reference() {
    let mut replay = payload();
    replay.prompts = vec!["fix the parser".to_string(), "now the docs".to_string()];
    replay.source = BenchSource::Session {
        agent_id: AgentId::generate(),
        started_ms: 1,
        title: "fix the parser".to_string(),
        baseline: SessionBaseline {
            calls: 12,
            cost_usd: 0.9,
            priced: true,
            models: vec!["claude-opus-4-7".to_string()],
        },
        reference: None,
    };
    let turn = |cost: f64, signal: OutcomeSignal| RunTurn::new("", cost, 1, signal);
    for run in &mut replay.runs {
        run.turns = vec![
            turn(0.02, OutcomeSignal::ChecksPassed),
            turn(0.04, OutcomeSignal::ChecksFailed),
        ];
    }
    replay.judgments[0].a_vs_reference = Some(RefVerdict::Worse);
    replay.judgments[0].b_vs_reference = Some(RefVerdict::Better);
    let text = render("Compare #1", &replay);
    assert!(
        text.contains("Replays the session \"fix the parser\": 2 prompts in order"),
        "{text}"
    );
    assert!(text.contains("The session itself: 12 calls, $0.90 on claude-opus-4-7"));
    assert_eq!(
        line(&text, "vs yours"),
        [
            "vs yours",
            "0 better, 0 as good, 1 worse",
            "1 better, 0 as good, 0 worse"
        ]
    );
    assert!(text.contains("Per prompt"), "{text}");
    let first = line(&text, "1.");
    assert!(first.contains(&"$0.02, 3 pass"), "{first:?}");
    let second = line(&text, "2.");
    assert!(second.contains(&"$0.04, 3 fail"), "{second:?}");
}

#[test]
fn example_tasks_show_per_task_passes_and_offer_no_apply() {
    let text = render("Compare #1", &examples());
    assert!(
        text.contains("3 runs of each of 2 example tasks per arm, each task's own check"),
        "{text}"
    );
    assert!(text.contains("Per task"), "{text}");
    let write_file = line(&text, "coding/write-file");
    assert_eq!(write_file.len(), 3, "{write_file:?}");
    assert!(
        write_file[1].starts_with("1/2 passed, $0.0"),
        "{write_file:?}"
    );
    assert!(
        write_file[2].starts_with("2/2 passed, $0.4"),
        "{write_file:?}"
    );
    assert!(line(&text, "a-2").contains(&"coding/fix-shell-bug"));
    assert!(!text.contains("/compare apply"), "{text}");
}

#[test]
fn the_view_is_pinned_byte_for_byte() {
    let expected = [
        "Compare #1 · \"fix the flaky parser test\" · done",
        "  A gate vs B aura-claude-opus-4-7 (cloud), 3 runs each, check `cargo test`, classifier aura-claude-haiku-4-5 (cloud)",
        "",
        "                A gate                            B aura-claude-opus-4-7",
        "  finished      3/3                               3/3",
        "  median time   42s                               42s",
        "  total cost    $0.18 · 87% less                  $1.35 · 100%",
        "  median cost   $0.06 · 87% less                  $0.45 · 100%",
        "  check         2/3 passed                        3/3 passed",
        "  cost per fix  $0.09 · 80% less                  $0.45 · 100%",
        "  judge         0 won, 1 lost, 0 tied             1 won, 0 lost, 0 tied",
        "  Too close to call with 3 runs: check passes are within one of each other.",
        "  Too close to call with 3 runs: judge wins are within one of each other.",
        "",
        "  Tokens per model",
        "    A: claude-sonnet-4-6 9 calls, 126k in / 5k out, $0.18",
        "    B: claude-opus-4-7 9 calls, 126k in / 5k out, $1.35",
        "  Plan usage: no call reported a plan's usage (only Claude and ChatGPT plans do), so tokens per model (what draws a plan down) are shown instead.",
        "  Judge: 1 blind call on aura-claude-opus-4-7 (cloud), $0.03 (not in the arm totals)",
        "",
        "  Runs",
        "    a-1      aura-claude-sonnet-4-6 (cloud)          41s     $0.05  check passed  2 files",
        "    b-1      aura-claude-opus-4-7 (cloud)            41s     $0.40  check passed  2 files",
        "    a-2      aura-claude-sonnet-4-6 (cloud)          42s     $0.07  check passed  2 files",
        "    b-2      aura-claude-opus-4-7 (cloud)            42s     $0.50  check passed  2 files",
        "    a-3      aura-claude-sonnet-4-6 (cloud)          43s     $0.06  check failed  2 files",
        "    b-3      aura-claude-opus-4-7 (cloud)            43s     $0.45  check passed  2 files",
        "  Note: the budget stopped 2 runs",
        "",
        "  /compare diff [run] · /compare apply <run> · /compare dismiss",
    ];
    assert_eq!(render("Compare #1", &payload()), expected.join("\n"));
}

#[test]
fn a_stopped_comparison_with_routed_runs_is_pinned_byte_for_byte() {
    let expected = [
        "Compare #2 · \"fix the flaky parser test\" · stopped",
        "  A gate vs B aura-claude-opus-4-7 (cloud), 3 runs each, check `cargo test`, classifier aura-claude-haiku-4-5 (cloud)",
        "",
        "                A gate                            B aura-claude-opus-4-7",
        "  finished      3/3                               2/3",
        "  median time   42s                               42s",
        "  total cost    $0.18 · 87% less                  $1.35 · 100%",
        "  median cost   $0.06 · 86% less                  $0.43 · 100%",
        "  check         2/3 passed                        2/2 passed",
        "  cost per fix  $0.09 · 87% less                  $0.68 · 100%",
        "  judge         0 won, 1 lost, 0 tied             1 won, 0 lost, 0 tied",
        "  Too close to call with 3 runs: check passes are within one of each other.",
        "  Too close to call with 3 runs: judge wins are within one of each other.",
        "",
        "  Tokens per model",
        "    A: claude-sonnet-4-6 9 calls, 126k in / 5k out, $0.18",
        "    B: claude-opus-4-7 9 calls, 126k in / 5k out, $1.35",
        "  Plan usage: no call reported a plan's usage (only Claude and ChatGPT plans do), so tokens per model (what draws a plan down) are shown instead.",
        "  Judge: 1 blind call on aura-claude-opus-4-7 (cloud), $0.03 (not in the arm totals)",
        "",
        "  Runs",
        "    a-1      aura-claude-sonnet-4-6 (cloud)          41s     $0.05  check passed  2 files  · debugging/hard, direct, asked to check 1×, moved up 1×",
        "    b-1      aura-claude-opus-4-7 (cloud)            41s     $0.40  check passed  2 files",
        "    a-2      aura-claude-sonnet-4-6 (cloud)          42s     $0.07  check passed  2 files  · multi_file_change, question, 1 of 2 turns led: lead $0.63 + subtasks $0.17, sent back 2×",
        "    b-2      aura-claude-opus-4-7 (cloud)            42s     $0.50                2 files  (the model fell over)",
        "    a-3      aura-claude-sonnet-4-6 (cloud)          43s     $0.06  check failed  2 files",
        "    b-3      aura-claude-opus-4-7 (cloud)            43s     $0.45  check passed  2 files",
        "  Note: the budget stopped 2 runs",
        "",
        "  /compare diff [run] · /compare apply <run> · /compare dismiss",
    ];
    assert_eq!(render("Compare #2", &routed()), expected.join("\n"));
}

#[test]
fn a_replayed_session_is_pinned_byte_for_byte() {
    let expected = [
        "Compare #3 · \"fix the flaky parser test\" · done",
        "  Replays the session \"fix the parser\": 2 prompts in order",
        "  A gate vs B aura-claude-opus-4-7 (cloud), 3 runs each, check `cargo test`, classifier aura-claude-haiku-4-5 (cloud)",
        "",
        "                A gate                            B aura-claude-opus-4-7",
        "  finished      3/3                               3/3",
        "  median time   42s                               42s",
        "  total cost    $0.18 · 87% less                  $1.35 · 100%",
        "  median cost   $0.06 · 87% less                  $0.45 · 100%",
        "  check         2/3 passed                        3/3 passed",
        "  cost per fix  $0.09 · 80% less                  $0.45 · 100%",
        "  judge         0 won, 1 lost, 0 tied             1 won, 0 lost, 0 tied",
        "  vs yours      0 better, 0 as good, 1 worse      1 better, 0 as good, 0 worse",
        "  Too close to call with 3 runs: check passes are within one of each other.",
        "  Too close to call with 3 runs: judge wins are within one of each other.",
        "  The session itself: 12 calls, $0.90 on claude-opus-4-7",
        "",
        "  Per prompt (mean turn cost; test or build commands that passed or failed)",
        "    1.        $0.02, 3 pass                     $0.02, 3 pass  \"fix the parser\"",
        "    2.        $0.04, 3 error                    $0.04, 3 error  \"now the docs\"",
        "",
        "  Tokens per model",
        "    A: claude-sonnet-4-6 9 calls, 126k in / 5k out, $0.18",
        "    B: claude-opus-4-7 9 calls, 126k in / 5k out, $1.35",
        "",
        "  Plan usage, share of each window's limit",
        "    A: Claude plan 5h window +2.1%, 7d opus window +0.2%",
        "    B: ChatGPT plan 5h window +1.2%",
        "  Plan readings cover the whole account: each rise between readings goes to the run whose call saw it, so runs at the same time don't count it twice, and use outside the comparison while it ran lands on the next run to read.",
        "  Judge: 1 blind call on aura-claude-opus-4-7 (cloud), $0.03 (not in the arm totals)",
        "",
        "  Runs",
        "    a-1      aura-claude-sonnet-4-6 (cloud)          41s     $0.05  check passed  2 files",
        "    b-1      aura-claude-opus-4-7 (cloud)            41s     $0.40  check passed  2 files",
        "    a-2      aura-claude-sonnet-4-6 (cloud)          42s     $0.07  check passed  2 files",
        "    b-2      aura-claude-opus-4-7 (cloud)            42s     $0.50  check passed  2 files",
        "    a-3      aura-claude-sonnet-4-6 (cloud)          43s     $0.06  check failed  2 files",
        "    b-3      aura-claude-opus-4-7 (cloud)            43s     $0.45  check passed  2 files",
        "  Note: the budget stopped 2 runs",
        "",
        "  /compare diff [run] · /compare apply <run> · /compare dismiss",
    ];
    assert_eq!(render("Compare #3", &replay()), expected.join("\n"));
}

#[test]
fn example_tasks_are_pinned_byte_for_byte() {
    let expected = [
        "Compare #4 · \"2 example coding tasks\" · done",
        "  A gate vs B aura-claude-opus-4-7 (cloud), 3 runs of each of 2 example tasks per arm, each task's own check, classifier aura-claude-haiku-4-5 (cloud)",
        "",
        "                A gate                            B aura-claude-opus-4-7",
        "  finished      3/3                               3/3",
        "  median time   42s                               42s",
        "  total cost    $0.18 · 87% less                  $1.35 · 100%",
        "  median cost   $0.06 · 87% less                  $0.45 · 100%",
        "  check         2/3 passed                        3/3 passed",
        "  cost per fix  $0.09 · 80% less                  $0.45 · 100%",
        "  judge         0 won, 1 lost, 0 tied             1 won, 0 lost, 0 tied",
        "  Too close to call with 3 runs: check passes are within one of each other.",
        "  Too close to call with 3 runs: judge wins are within one of each other.",
        "",
        "  Per task (checks passed; mean run cost)",
        "    coding/write-file                 1/2 passed, $0.06 · 87% less      2/2 passed, $0.43 · 100%",
        "    coding/fix-shell-bug              1/1 passed, $0.07 · 86% less      1/1 passed, $0.50 · 100%",
        "",
        "  Tokens per model",
        "    A: claude-sonnet-4-6 9 calls, 126k in / 5k out, $0.18",
        "    B: claude-opus-4-7 9 calls, 126k in / 5k out, $1.35",
        "  Plan usage: no call reported a plan's usage (only Claude and ChatGPT plans do), so tokens per model (what draws a plan down) are shown instead.",
        "  Judge: 1 blind call on aura-claude-opus-4-7 (cloud), $0.03 (not in the arm totals)",
        "",
        "  Runs",
        "    a-1      aura-claude-sonnet-4-6 (cloud)          41s     $0.05  check passed  2 files  coding/write-file",
        "    b-1      aura-claude-opus-4-7 (cloud)            41s     $0.40  check passed  2 files  coding/write-file",
        "    a-2      aura-claude-sonnet-4-6 (cloud)          42s     $0.07  check passed  2 files  coding/fix-shell-bug",
        "    b-2      aura-claude-opus-4-7 (cloud)            42s     $0.50  check passed  2 files  coding/fix-shell-bug",
        "    a-3      aura-claude-sonnet-4-6 (cloud)          43s     $0.06  check failed  2 files  coding/write-file",
        "    b-3      aura-claude-opus-4-7 (cloud)            43s     $0.45  check passed  2 files  coding/write-file",
        "  Note: the budget stopped 2 runs",
        "",
        "  /compare diff [run] · /compare dismiss",
    ];
    assert_eq!(render("Compare #4", &examples()), expected.join("\n"));
}
