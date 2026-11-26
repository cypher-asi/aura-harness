use super::*;

#[test]
fn a_model_with_no_known_price_reads_unknown_not_free() {
    let theme = Theme::default();
    let shown = text(&detail_lines(
        &unpriced_view(6, 6, 0.0),
        None,
        &theme,
        140,
        None,
    ));
    let tokens = line_of(&shown, "B  glm-9 ");
    assert!(tokens.trim_end().ends_with("unknown"), "{shown}");
    assert!(!tokens.contains("free"), "{shown}");
    for label in ["cost ", "cost/run ", "$/fix "] {
        let row = line_of(&shown, label);
        assert!(row.trim_end().ends_with("unknown"), "{row}");
        assert!(
            !row.contains('%'),
            "no saving against a missing price: {row}"
        );
    }
    assert!(run_line(&shown, "b-1").contains("unknown"), "{shown}");
    assert!(
        shown.contains("+? some calls have no known price: glm-9"),
        "{shown}"
    );
    assert!(!shown.contains("free"), "{shown}");
}

#[test]
fn a_partly_priced_cost_shows_the_priced_part_marked() {
    let theme = Theme::default();
    let shown = text(&detail_lines(
        &unpriced_view(6, 2, 0.22),
        None,
        &theme,
        140,
        None,
    ));
    assert!(
        line_of(&shown, "B  glm-9 ").trim_end().ends_with("$0.22+?"),
        "{shown}"
    );
    let cost = line_of(&shown, "cost ");
    assert!(
        cost.contains("$0.12") && cost.trim_end().ends_with("$0.22+?"),
        "{cost}"
    );
    assert!(!cost.contains('%'), "{cost}");
    let per_run = line_of(&shown, "cost/run ");
    assert!(per_run.trim_end().ends_with("$0.11+?"), "{per_run}");
    assert!(!per_run.contains('%'), "{per_run}");
    assert!(
        line_of(&shown, "$/fix ").trim_end().ends_with("$0.22+?"),
        "{shown}"
    );
    assert!(run_line(&shown, "b-1").contains("$0.11+?"), "{shown}");
    assert!(shown.contains("$0.06+? 2✓"), "turn cells: {shown}");
    assert!(
        shown.contains("+? some calls have no known price"),
        "{shown}"
    );
}

#[test]
fn a_fully_priced_benchmark_has_no_note_and_keeps_free_for_local_models() {
    let theme = Theme::default();
    let mut view = view();
    view.arms[1].tokens = vec![BenchTokens {
        model: "local/qwen3:4b".to_string(),
        calls: 3,
        ..BenchTokens::default()
    }];
    let shown = text(&detail_lines(&view, None, &theme, 120, None));
    assert!(!shown.contains("no known price"), "{shown}");
    assert!(!shown.contains("+?"), "{shown}");
    assert!(
        line_of(&shown, "B  qwen3:4b ").trim_end().ends_with("free"),
        "{shown}"
    );
}

#[test]
fn savings_need_both_costs_complete() {
    let cost = |usd, unpriced| Some(BenchCost::new(usd, unpriced));
    assert_eq!(saving(cost(0.1, 0), cost(0.4, 0)), ["-75%", ""]);
    assert_eq!(saving(cost(0.1, 0), cost(0.0, 3)), ["", ""]);
    assert_eq!(saving(cost(0.1, 1), cost(0.4, 0)), ["", ""]);
    assert_eq!(saving(None, cost(0.4, 0)), ["", ""]);
}

#[test]
fn task_cells_mark_a_cost_that_leaves_out_unpriced_calls() {
    use crate::events::BenchTaskRow;
    let theme = Theme::default();
    let mut view = unpriced_view(6, 6, 0.0);
    view.prompts = Vec::new();
    let cell = |usd, unpriced| BenchTaskCell {
        runs: 1,
        passed: 1,
        checked: 1,
        mean_cost_usd: Some(usd),
        unpriced_calls: unpriced,
    };
    view.tasks = vec![
        BenchTaskRow {
            id: "coding/one".to_string(),
            arms: [cell(0.10, 0), cell(0.0, 4)],
            ..BenchTaskRow::default()
        },
        BenchTaskRow {
            id: "coding/two".to_string(),
            arms: [cell(0.10, 0), cell(0.05, 1)],
            ..BenchTaskRow::default()
        },
    ];
    let shown = text(&detail_lines(&view, None, &theme, 140, None));
    let one = line_of(&shown, "one ");
    assert!(one.contains("unknown") && !one.contains('%'), "{one}");
    let two = line_of(&shown, "two ");
    assert!(two.contains("$0.05+?") && !two.contains('%'), "{two}");
}

#[test]
fn the_classifiers_calls_are_shown_apart_from_the_arms() {
    let theme = Theme::default();
    let mut view = view();
    let shown = text(&detail_lines(&view, None, &theme, 160, None));
    assert!(!shown.contains("calls · "), "none until it ends:\n{shown}");
    view.classify = vec![BenchTokens {
        model: "claude-haiku-4-5".to_string(),
        purpose: Some("classify".to_string()),
        calls: 3,
        cost_usd: 0.0012,
        ..BenchTokens::default()
    }];
    let shown = text(&detail_lines(&view, None, &theme, 160, None));
    assert!(
        shown.contains("classifier haiku-4-5 · 3 calls · $0.0012, not in arm costs"),
        "{shown}"
    );
}

#[test]
fn a_judge_with_no_known_price_reads_unknown_not_free() {
    let theme = Theme::default();
    let judge_line = |cost_usd, unpriced_calls| {
        let mut view = view();
        if let Some(judge) = view.judge.as_mut() {
            judge.cost_usd = cost_usd;
            judge.unpriced_calls = unpriced_calls;
        }
        let shown = text(&detail_lines(&view, None, &theme, 160, None));
        shown
            .lines()
            .find(|l| l.contains("blind call"))
            .unwrap_or_else(|| panic!("no judge line in\n{shown}"))
            .to_string()
    };
    assert!(judge_line(0.0, 1).contains("· unknown, not in arm costs"));
    assert!(judge_line(0.03, 1).contains("· $0.03+?, not in arm costs"));
    assert!(judge_line(0.03, 0).contains("· $0.03, not in arm costs"));
}
