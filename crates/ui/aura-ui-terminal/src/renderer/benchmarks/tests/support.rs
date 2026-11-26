use super::*;

pub(super) fn arm(label: &str, cost: f64) -> BenchArm {
    BenchArm {
        label: label.to_string(),
        runs: 2,
        finished: 2,
        total_cost_usd: cost * 2.0,
        median_cost_usd: Some(cost),
        median_wall_ms: Some(42_000),
        checks_passed: 1,
        checks_run: 2,
        judge_wins: 1,
        ref_better: 1,
        ref_as_good: 1,
        turns: vec![
            BenchTurn {
                runs: 2,
                mean_cost_usd: Some(cost / 2.0),
                passed: 2,
                ..BenchTurn::default()
            },
            BenchTurn {
                runs: 2,
                mean_cost_usd: Some(cost / 2.0),
                failed: 1,
                ..BenchTurn::default()
            },
        ],
        ..BenchArm::default()
    }
}

pub(super) fn view() -> BenchView {
    BenchView {
        id: "ab".to_string(),
        name: "Compare #1".to_string(),
        title: "fix the parser".to_string(),
        source: "session \"fix the parser\"".to_string(),
        status: BenchStatus::Done,
        repeats: 2,
        prompts: vec!["fix the parser".to_string(), "now the docs".to_string()],
        arms: [arm("gate", 0.06), arm("claude-opus-4-7", 0.40)],
        runs: vec![BenchRun {
            name: "a-1".to_string(),
            model: "claude-sonnet-4-6".to_string(),
            wall_ms: 42_000,
            cost_usd: 0.06,
            check_passed: Some(true),
            files_changed: 2,
            ..BenchRun::default()
        }],
        judge: Some(BenchJudge {
            model: "opus".to_string(),
            cost_usd: 0.03,
            verdicts: vec![BenchVerdict {
                a_run: "a-1".to_string(),
                b_run: "b-1".to_string(),
                parsed: true,
                winner: Some(1),
                a_vs_reference: Some("as good".to_string()),
                b_vs_reference: Some("better".to_string()),
                reasoning: "B also fixed the docs".to_string(),
            }],
            ..BenchJudge::default()
        }),
        baseline: Some(BenchBaseline {
            calls: 12,
            cost_usd: 0.20,
            priced: true,
            unpriced_calls: 0,
            models: vec!["claude-opus-4-7".to_string()],
        }),
        has_reference: true,
        ..BenchView::default()
    }
}

pub(super) fn text(lines: &[Line<'_>]) -> String {
    lines
        .iter()
        .map(|l| {
            l.spans
                .iter()
                .map(|s| s.content.as_ref())
                .collect::<String>()
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// The detail of a benchmark whose arm B ran on `glm-9`, a model with
/// no known price: `unpriced` of the arm's `calls` calls had none and
/// the rest cost `priced_usd`.
pub(super) fn unpriced_view(calls: u32, unpriced: u32, priced_usd: f64) -> BenchView {
    let mut view = view();
    view.baseline = None;
    let b = &mut view.arms[1];
    b.label = "glm-9".to_string();
    b.total_cost_usd = priced_usd;
    b.median_cost_usd = Some(priced_usd / 2.0);
    b.unpriced_calls = unpriced;
    b.finished_unpriced_calls = unpriced;
    b.checks_passed = 1;
    for turn in &mut b.turns {
        turn.mean_cost_usd = Some(priced_usd / 4.0);
        turn.unpriced_calls = unpriced;
    }
    b.tokens = vec![BenchTokens {
        model: "openrouter/z-ai/glm-9".to_string(),
        purpose: None,
        calls,
        input_tokens: 90_000,
        output_tokens: 4_000,
        cost_usd: priced_usd,
        unpriced_calls: unpriced,
    }];
    view.arms[0].tokens = vec![BenchTokens {
        model: "claude-sonnet-4-6".to_string(),
        purpose: None,
        calls: 4,
        input_tokens: 40_000,
        output_tokens: 2_000,
        cost_usd: 0.12,
        unpriced_calls: 0,
    }];
    view.runs.push(BenchRun {
        name: "b-1".to_string(),
        arm: 1,
        model: "openrouter/z-ai/glm-9".to_string(),
        wall_ms: 30_000,
        cost_usd: priced_usd / 2.0,
        unpriced_calls: unpriced,
        ..BenchRun::default()
    });
    view
}

pub(super) fn line_of<'s>(shown: &'s str, start: &str) -> &'s str {
    shown
        .lines()
        .find(|l| l.starts_with(start))
        .unwrap_or_else(|| panic!("no {start:?} line in\n{shown}"))
}

pub(super) fn run_line<'s>(shown: &'s str, name: &str) -> &'s str {
    shown
        .lines()
        .skip_while(|l| !l.contains("── runs"))
        .find(|l| l.split_whitespace().next() == Some(name))
        .unwrap_or_else(|| panic!("no {name} run in\n{shown}"))
}

/// The detail of a benchmark with a-1 moved up and leading, b-1 staying.
pub(super) fn classified_shown() -> String {
    let theme = Theme::default();
    let mut view = view();
    view.runs[0].escalated_to = Some("claude-opus-4-7".to_string());
    view.runs[0].category = Some("multi_file_change".to_string());
    view.runs[0].difficulty = Some("hard".to_string());
    view.runs[0].led = true;
    view.runs[0].subtask_cost_usd = 0.26;
    view.runs.push(BenchRun {
        name: "b-1".to_string(),
        arm: 1,
        model: "claude-opus-4-7".to_string(),
        category: Some("small_change".to_string()),
        ..BenchRun::default()
    });
    view.arms[0].escalated = 1;
    view.arms[0].nudged = 1;
    text(&detail_lines(&view, None, &theme, 120, None))
}

/// A running benchmark with a-2 under way on a route it moved up to.
pub(super) fn under_way_view() -> BenchView {
    let mut view = view();
    view.status = BenchStatus::Running;
    view.runs[0].task = Some("ansible-b748edea".to_string());
    view.runs[0].category = Some("small_change".to_string());
    view.runs[0].difficulty = Some("medium".to_string());
    view.live = vec![BenchLiveRun {
        name: "a-2".to_string(),
        task: Some("ansible-40ade1f8".to_string()),
        model: Some("claude/claude-opus-5-5".to_string()),
        category: Some("multi_file_change".to_string()),
        difficulty: Some("hard".to_string()),
        wall_ms: Some(301_000),
        cost_usd: 0.80,
        doing: Some(BenchDoing {
            what: "writing 49 files".to_string(),
            for_ms: 3_000,
            ..BenchDoing::default()
        }),
        ..BenchLiveRun::default()
    }];
    view
}

/// A running benchmark whose four tasks are at each stage of setup, with
/// a-1 working and b-2 waiting for its task.
pub(super) fn running_setup_view() -> BenchView {
    use crate::events::BenchTaskRow;
    let mut view = view();
    view.status = BenchStatus::Running;
    view.judge = None;
    let task = |id: &str, setup| BenchTaskRow {
        id: format!("swe/{id}"),
        setup,
        ..BenchTaskRow::default()
    };
    view.tasks = vec![
        task("ansible-1", BenchTaskSetup::Ready),
        task("flipt-2", BenchTaskSetup::Fetching { pull: true }),
        task("flipt-3", BenchTaskSetup::Queued),
        task(
            "navidrome-4",
            BenchTaskSetup::Failed("no image".to_string()),
        ),
    ];
    view.live = vec![
        BenchLiveRun {
            name: "a-1".to_string(),
            task: Some("swe/ansible-1".to_string()),
            wall_ms: Some(72_000),
            cost_usd: 0.04,
            tokens: vec![BenchTokens {
                model: "claude-sonnet-4-6".to_string(),
                purpose: None,
                calls: 3,
                input_tokens: 42_000,
                output_tokens: 1_500,
                cost_usd: 0.04,
                unpriced_calls: 0,
            }],
            ..BenchLiveRun::default()
        },
        BenchLiveRun {
            name: "b-2".to_string(),
            arm: 1,
            task: Some("swe/flipt-2".to_string()),
            ..BenchLiveRun::default()
        },
    ];
    view
}

/// The detail of `view` at `width`, on shimmer frame 3.
pub(super) fn running_setup_shown(view: &BenchView, width: usize) -> String {
    text(&detail_lines(view, None, &Theme::default(), width, Some(3)))
}

/// The detail of a benchmark of two example tasks, one of them run.
pub(super) fn example_tasks_shown() -> String {
    use crate::events::{BenchTaskCell, BenchTaskRow};
    let theme = Theme::default();
    let mut view = view();
    view.source = "2 example tasks".to_string();
    view.prompts = Vec::new();
    view.baseline = None;
    view.tasks = vec![
        BenchTaskRow {
            id: "coding/write-file".to_string(),
            title: "Write a file".to_string(),
            arms: [
                BenchTaskCell {
                    runs: 2,
                    passed: 2,
                    checked: 2,
                    mean_cost_usd: Some(0.02),
                    unpriced_calls: 0,
                },
                BenchTaskCell {
                    runs: 2,
                    passed: 1,
                    checked: 2,
                    mean_cost_usd: Some(0.30),
                    unpriced_calls: 0,
                },
            ],
            ..BenchTaskRow::default()
        },
        BenchTaskRow {
            id: "coding/fix-shell-bug".to_string(),
            ..BenchTaskRow::default()
        },
    ];
    view.runs[0].task = Some("coding/write-file".to_string());
    text(&detail_lines(&view, None, &theme, 120, None))
}
