//! The estimate the user confirms before a comparison starts, for a
//! prompt, a replayed session and example tasks, each also pinned byte
//! for byte.

use super::super::estimate::{row, wrap, EstimateView, VALUE_WIDTH};
use super::example;
use aura_core_types::ExampleTaskInfo;
use aura_gate::compare::Estimate;

#[test]
fn the_estimate_names_the_arms_runs_and_budget() {
    let text = EstimateView {
        task: "fix it",
        prompts: 1,
        session: None,
        examples: &[],
        a: "gate (likely aura-claude-sonnet-4-6 (cloud))".to_string(),
        b: "aura-claude-opus-4-7 (cloud)".to_string(),
        classifier: "aura-claude-haiku-4-5",
        classifier_named: false,
        judge: "aura-claude-opus-4-7",
        repeats: 3,
        check: None,
        budget_usd: Some(2.0),
        concurrency: 4,
        estimate: Estimate {
            a_usd: Some(1.5),
            b_usd: Some(4.0),
            judge_usd: None,
        },
        notes: &[],
    }
    .render();
    assert!(text.starts_with("Compare: \"fix it\"\n"), "{text}");
    for row in [
        "\n  A             gate (likely aura-claude-sonnet-4-6 (cloud))\n",
        "\n  B             aura-claude-opus-4-7 (cloud)\n",
        "\n  Classifier    aura-claude-haiku-4-5 (cloud) · Gate's choice of fast model;\n                --classifier <model> picks another\n",
        "\n  Runs          3 per arm, up to 4 at a time,\n                each in its own copy of the workspace\n",
        "\n  Check         none · --check <cmd> or AURA_CHECK_COMMAND counts passes\n",
        "\n  Estimate      ~$5.50 at list prices\n                A ~$1.50 · B ~$4.00\n                judge ~unknown on ",
        "\n                some prices are unknown\n",
        "\n  Budget        $2.00: no new run starts",
    ] {
        assert!(text.contains(row), "{row:?} in\n{text}");
    }
    assert!(
        text.lines()
            .all(|line| line.chars().count() <= 16 + VALUE_WIDTH),
        "{text}"
    );
}

#[test]
fn long_values_wrap_at_spaces_under_the_value_column() {
    let mut out = String::from("Heading");
    let words = "word ".repeat(20);
    row(&mut out, "Label", words.trim_end());
    let lines: Vec<&str> = out.lines().collect();
    assert_eq!(lines.len(), 3, "{out}");
    assert!(lines[1].starts_with("  Label         word word"));
    assert!(lines[2].starts_with("                word"));
    assert!(lines[1].chars().count() <= 16 + VALUE_WIDTH);
    assert_eq!(wrap("one-long-word", 4), ["one-long-word"]);
}

#[test]
fn a_session_estimate_says_what_it_replays_with_its_notes() {
    let notes = vec!["shell changes are not undone".to_string()];
    let text = EstimateView {
        task: "fix it",
        prompts: 4,
        session: Some("fix the parser"),
        examples: &[],
        a: "claude-sonnet-4-6".to_string(),
        b: "claude-opus-4-7".to_string(),
        classifier: "claude/claude-haiku-4-5",
        classifier_named: true,
        judge: "claude-opus-4-7",
        repeats: 2,
        check: Some("cargo test"),
        budget_usd: None,
        concurrency: 4,
        estimate: Estimate {
            a_usd: Some(1.0),
            b_usd: Some(2.0),
            judge_usd: Some(0.5),
        },
        notes: &notes,
    }
    .render();
    assert!(
        text.starts_with("Benchmark: replay the session \"fix the parser\", 4 prompts in order"),
        "{text}"
    );
    assert!(
        text.contains("\n  A             claude-sonnet-4-6\n"),
        "{text}"
    );
    assert!(
        text.contains("\n  Classifier    claude-haiku-4-5 (claude plan)\n"),
        "a named classifier reads as itself: {text}"
    );
    assert!(text.contains("\n  Runs          2 per arm"), "{text}");
    assert!(
        text.contains("\n  Check         `cargo test` after each run\n"),
        "{text}"
    );
    assert!(
        text.contains("\n  Note          shell changes are not undone"),
        "{text}"
    );
}

/// Notes of a replayed session's estimate.
fn notes() -> Vec<String> {
    vec![
        "shell changes are not undone".to_string(),
        "Learn mode: Gate arms explore as routed turns do (5% of turns), so the runs teach \
         Gate about models it would not pick yet"
            .to_string(),
    ]
}

/// Two example tasks of different suites.
fn tasks() -> [ExampleTaskInfo; 2] {
    [
        example("coding/write-file"),
        example("gate_hard/retry-spec"),
    ]
}

/// The estimate of 2 runs per arm, up to 4 at a time, of Gate against
/// a model per difficulty, whose price is unknown, judged by Opus.
fn estimate_of(
    session: Option<&str>,
    examples: &[ExampleTaskInfo],
    check: Option<&str>,
    budget_usd: Option<f64>,
    notes: &[String],
) -> String {
    EstimateView {
        task: "fix the flaky parser test\nmore detail",
        prompts: 3,
        session,
        examples,
        a: "gate (likely aura-claude-sonnet-4-6 (cloud))".to_string(),
        b: "by difficulty: easy → aura-claude-haiku-4-5 (cloud) · medium → \
            aura-claude-sonnet-4-6 (cloud) · hard → aura-claude-opus-4-7 (cloud)"
            .to_string(),
        classifier: "haiku",
        classifier_named: true,
        judge: "aura-claude-opus-4-7",
        repeats: 2,
        check,
        budget_usd,
        concurrency: 4,
        estimate: Estimate {
            a_usd: Some(1.5),
            b_usd: None,
            judge_usd: Some(0.004),
        },
        notes,
    }
    .render()
}

#[test]
fn an_estimate_of_example_tasks_lists_them_with_their_own_checks() {
    let estimate = estimate_of(None, &tasks(), None, None, &[]);
    assert!(
        estimate.starts_with(
            "Benchmark: 2 example coding tasks\n  Tasks         coding/write-file, gate_hard/retry-spec\n"
        ),
        "{estimate}"
    );
    assert!(
        estimate
            .contains("\n  Runs          2 of each task per arm (4 per arm), up to 4 at a time,\n"),
        "{estimate}"
    );
    assert!(
        estimate.contains("\n  Check         each task's own command"),
        "{estimate}"
    );
    assert!(!estimate.contains("--check <cmd>"), "{estimate}");
}

#[test]
fn the_estimate_of_a_prompt_is_pinned_byte_for_byte() {
    let expected = [
        "Compare: \"fix the flaky parser test\"",
        "  A             gate (likely aura-claude-sonnet-4-6 (cloud))",
        "  B             by difficulty: easy → aura-claude-haiku-4-5 (cloud) · medium →",
        "                aura-claude-sonnet-4-6 (cloud) · hard → aura-claude-opus-4-7",
        "                (cloud)",
        "  Classifier    haiku (cloud)",
        "  Runs          2 per arm, up to 4 at a time,",
        "                each in its own copy of the workspace",
        "  Check         none · --check <cmd> or AURA_CHECK_COMMAND counts passes",
        "  Estimate      ~$1.50 at list prices",
        "                A ~$1.50 · B ~unknown",
        "                judge ~$0.0040 on aura-claude-opus-4-7 (cloud)",
        "                for turns of about 150k input and 6k output tokens",
        "                some prices are unknown",
        "  Budget        $2.00: no new run starts once the finished ones cost that much",
        "                it counts only priced calls: B has no known price, so its calls",
        "                are not counted",
    ];
    assert_eq!(
        estimate_of(None, &[], None, Some(2.0), &[]),
        expected.join("\n")
    );
}

#[test]
fn the_budget_row_names_every_arm_whose_price_is_unknown() {
    let budget_row = |a_usd, b_usd, judge_usd| {
        let text = EstimateView {
            estimate: Estimate {
                a_usd,
                b_usd,
                judge_usd,
            },
            ..view_with_budget(Some(2.0))
        }
        .render();
        let at = text.find("\n  Budget").map_or(text.len(), |at| at + 1);
        text[at..].to_string()
    };
    assert_eq!(
        budget_row(None, None, Some(0.1)),
        "  Budget        $2.00: no new run starts once the finished ones cost that much\n\
         \x20               it counts only priced calls: A and B have no known price, so\n\
         \x20               their calls are not counted"
    );
    assert_eq!(
        budget_row(Some(1.0), Some(2.0), None),
        "  Budget        $2.00: no new run starts once the finished ones cost that much",
        "the judge runs after the budget's runs, so its price does not matter here"
    );
    let unbudgeted = EstimateView {
        estimate: Estimate {
            a_usd: None,
            b_usd: None,
            judge_usd: None,
        },
        ..view_with_budget(None)
    }
    .render();
    assert!(!unbudgeted.contains("Budget"), "{unbudgeted}");
}

/// A prompt's estimate with `budget_usd`, every price known.
fn view_with_budget(budget_usd: Option<f64>) -> EstimateView<'static> {
    EstimateView {
        task: "fix it",
        prompts: 1,
        session: None,
        examples: &[],
        a: "aura-claude-sonnet-4-6 (cloud)".to_string(),
        b: "mystery".to_string(),
        classifier: "haiku",
        classifier_named: true,
        judge: "aura-claude-opus-4-7",
        repeats: 1,
        check: None,
        budget_usd,
        concurrency: 4,
        estimate: Estimate {
            a_usd: Some(1.0),
            b_usd: Some(1.0),
            judge_usd: Some(0.1),
        },
        notes: &[],
    }
}

#[test]
fn the_estimate_of_a_replayed_session_is_pinned_byte_for_byte() {
    let expected = [
        "Benchmark: replay the session \"fix the parser\", 3 prompts in order",
        "  A             gate (likely aura-claude-sonnet-4-6 (cloud))",
        "  B             by difficulty: easy → aura-claude-haiku-4-5 (cloud) · medium →",
        "                aura-claude-sonnet-4-6 (cloud) · hard → aura-claude-opus-4-7",
        "                (cloud)",
        "  Classifier    haiku (cloud)",
        "  Runs          2 per arm, up to 4 at a time,",
        "                each in its own copy of the workspace",
        "  Check         `cargo test` after each run",
        "  Estimate      ~$1.50 at list prices",
        "                A ~$1.50 · B ~unknown",
        "                judge ~$0.0040 on aura-claude-opus-4-7 (cloud)",
        "                for turns of about 150k input and 6k output tokens",
        "                some prices are unknown",
        "  Note          shell changes are not undone",
        "  Note          Learn mode: Gate arms explore as routed turns do (5% of turns),",
        "                so the runs teach Gate about models it would not pick yet",
    ];
    assert_eq!(
        estimate_of(
            Some("fix the parser"),
            &[],
            Some("cargo test"),
            None,
            &notes()
        ),
        expected.join("\n")
    );
}

#[test]
fn the_estimate_of_example_tasks_is_pinned_byte_for_byte() {
    let expected = [
        "Benchmark: 2 example coding tasks",
        "  Tasks         coding/write-file, gate_hard/retry-spec",
        "  A             gate (likely aura-claude-sonnet-4-6 (cloud))",
        "  B             by difficulty: easy → aura-claude-haiku-4-5 (cloud) · medium →",
        "                aura-claude-sonnet-4-6 (cloud) · hard → aura-claude-opus-4-7",
        "                (cloud)",
        "  Classifier    haiku (cloud)",
        "  Runs          2 of each task per arm (4 per arm), up to 4 at a time,",
        "                each in a fresh copy of its task's files",
        "  Check         each task's own command; hidden checks go in only after the run",
        "  Estimate      ~$1.50 at list prices",
        "                A ~$1.50 · B ~unknown",
        "                judge ~$0.0040 on aura-claude-opus-4-7 (cloud)",
        "                for turns of about 150k input and 6k output tokens",
        "                some prices are unknown",
        "  Note          Learn mode: Gate arms explore as routed turns do (5% of turns),",
        "                so the runs teach Gate about models it would not pick yet",
    ];
    assert_eq!(
        estimate_of(None, &tasks(), Some("cargo test"), None, &notes()[1..]),
        expected.join("\n")
    );
}
