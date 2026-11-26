use super::form::form_lines;
use super::format::*;
use super::*;
use crate::events::*;
use crate::events::{BenchBaseline, BenchJudge, BenchRun, UiCommand};
use crate::renderer::text::display_width;
use ratatui::text::Line;
use ratatui::{backend::TestBackend, Terminal};

fn arm(label: &str, cost: f64) -> BenchArm {
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

fn view() -> BenchView {
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

#[test]
fn a_partly_priced_session_shows_no_ratio_and_marks_its_cost() {
    let theme = Theme::default();
    let mut view = view();
    if let Some(baseline) = view.baseline.as_mut() {
        baseline.unpriced_calls = 3;
    }
    let shown = text(&detail_lines(&view, Some(0), &theme, 120, None));
    assert!(!shown.contains("vs session"), "{shown}");
    assert!(
        shown.contains("session: 12 calls, $0.20+? on opus-4-7"),
        "{shown}"
    );
    if let Some(baseline) = view.baseline.as_mut() {
        baseline.priced = false;
        baseline.cost_usd = 0.0;
        baseline.unpriced_calls = 12;
    }
    let shown = text(&detail_lines(&view, Some(0), &theme, 120, None));
    assert!(!shown.contains("vs session"), "{shown}");
    assert!(
        shown.contains("session: 12 calls, no price on opus-4-7"),
        "{shown}"
    );
}

fn text(lines: &[Line<'_>]) -> String {
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

#[test]
fn the_detail_shows_cost_quality_and_each_prompt() {
    let theme = Theme::default();
    let shown = text(&detail_lines(&view(), Some(0), &theme, 120, None));
    let row = |label: &str| {
        shown
            .lines()
            .find(|l| l.starts_with(&format!("{label} ")))
            .unwrap_or_else(|| panic!("no {label} row in\n{shown}"))
            .to_string()
    };
    assert!(
        row("").contains("A gate") && row("").contains("B opus-4-7"),
        "{shown}"
    );
    for (label, a, b) in [
        ("cost", "$0.12", "$0.80"),
        ("cost/run", "$0.06", "$0.40"),
        ("$/fix", "$0.12", "$0.80"),
    ] {
        let line = row(label);
        assert!(
            line.contains(a) && line.contains("-85%") && line.trim_end().ends_with(b),
            "{line}"
        );
    }
    assert!(row("vs session").contains("0.30×"), "{shown}");
    assert!(row("vs session").contains("2.00×"), "{shown}");
    assert!(row("vs yours +/=/-").contains("1/1/0"), "{shown}");
    assert!(shown.contains("$0.03 2✓"), "{shown}");
    assert!(shown.contains("$0.03 1✗"), "{shown}");
    assert!(
        shown.contains("a-1 b-1  B  A= B+  B also fixed the docs"),
        "{shown}"
    );
    assert!(
        shown.contains("session: 12 calls, $0.20 on opus-4-7"),
        "{shown}"
    );
    assert!(shown.contains("›  a-1  sonnet-4-6"), "{shown}");
    assert!(
        !shown.lines().any(|l| l.starts_with("moved up")),
        "no summary row when nothing moved up: {shown}"
    );
}

#[test]
fn a_run_shows_what_drove_its_difficulty() {
    let theme = Theme::default();
    let mut view = view();
    let header = |view: &BenchView| {
        let shown = text(&detail_lines(view, None, &theme, 160, None));
        let header = shown
            .lines()
            .skip_while(|l| !l.contains("── runs"))
            .nth(1)
            .unwrap_or_default()
            .to_string();
        (shown, header)
    };
    let (_, plain) = header(&view);
    assert!(
        !plain.split_whitespace().any(|w| w == "why"),
        "no column for runs whose classifier named the difficulty: {plain}"
    );
    view.runs[0].difficulty = Some("hard".to_string());
    view.runs[0].drivers = Some("many files, loose spec, deep exploring".to_string());
    let (shown, header) = header(&view);
    let heads: Vec<&str> = header.split_whitespace().collect();
    assert!(
        heads.windows(2).any(|w| w == ["difficulty", "why"]),
        "{shown}"
    );
    let run = shown
        .lines()
        .skip_while(|l| !l.contains("── runs"))
        .find(|l| l.split_whitespace().next() == Some("a-1"))
        .unwrap_or_default();
    assert!(
        run.contains("hard") && run.contains("many files, loose spec, deep exploring"),
        "{shown}"
    );
}

#[test]
fn a_run_shows_how_it_was_classified_and_where_it_moved_up() {
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
    let shown = text(&detail_lines(&view, None, &theme, 120, None));
    let run = |name: &str| {
        shown
            .lines()
            .skip_while(|l| !l.contains("── runs"))
            .find(|l| l.split_whitespace().next() == Some(name))
            .unwrap_or_else(|| panic!("no {name} run in\n{shown}"))
            .to_string()
    };
    let moved = run("a-1");
    assert!(moved.contains("sonnet-4-6"), "{shown}");
    let words: Vec<&str> = moved.split_whitespace().collect();
    assert!(
        words.windows(2).any(|w| w == ["multi-file", "hard"]),
        "{shown}"
    );
    let header = shown
        .lines()
        .skip_while(|l| !l.contains("── runs"))
        .nth(1)
        .unwrap_or_default();
    let heads: Vec<&str> = header.split_whitespace().collect();
    assert!(
        heads.windows(2).any(|w| w == ["classified", "difficulty"]),
        "{shown}"
    );
    assert!(moved.contains("to opus-4-7"), "{shown}");
    assert!(moved.contains("yes, $0.26 in subtasks"), "{shown}");
    let stayed = run("b-1");
    assert!(stayed.contains("small change"), "{shown}");
    assert_eq!(
        stayed.split_whitespace().filter(|w| *w == "no").count(),
        2,
        "neither moved up nor led: {shown}"
    );
    assert!(shown.contains("classified"), "{shown}");
    let row = |label: &str| {
        shown
            .lines()
            .find(|l| l.starts_with(label))
            .unwrap_or_else(|| panic!("no {label} row in\n{shown}"))
            .to_string()
    };
    assert!(row("moved up").contains("1/"), "{shown}");
    assert!(row("nudged").contains("1/"), "{shown}");
}

#[test]
fn a_move_that_only_raised_the_effort_shows_as_moved_up() {
    let theme = Theme::default();
    let mut view = view();
    view.runs[0].model = "claude/claude-opus-5-5".to_string();
    view.runs[0].escalated_to = Some("claude/claude-opus-5-5".to_string());
    view.runs[0].escalated_effort = Some("max".to_string());
    let shown = text(&detail_lines(&view, None, &theme, 160, None));
    let moved = shown
        .lines()
        .skip_while(|l| !l.contains("── runs"))
        .find(|l| l.split_whitespace().next() == Some("a-1"))
        .unwrap_or_else(|| panic!("no a-1 run in\n{shown}"));
    assert!(moved.contains("to opus-5-5 max"), "{shown}");
}

#[test]
fn moved_up_and_led_show_only_when_a_run_did_either() {
    let theme = Theme::default();
    let mut view = view();
    view.runs[0].category = Some("small_change".to_string());
    view.runs[0].difficulty = Some("medium".to_string());
    let shown = text(&detail_lines(&view, None, &theme, 120, None));
    let header = shown
        .lines()
        .skip_while(|l| !l.contains("── runs"))
        .nth(1)
        .unwrap_or_default();
    assert!(header.contains("difficulty"), "{shown}");
    assert!(
        !header.contains("moved up") && !header.contains("led"),
        "{header}"
    );
    let run = shown
        .lines()
        .skip_while(|l| !l.contains("── runs"))
        .find(|l| l.split_whitespace().next() == Some("a-1"))
        .unwrap_or_default();
    assert!(run.contains("small change  medium"), "{shown}");
    assert!(!run.split_whitespace().any(|w| w == "no"), "{run}");
}

#[test]
fn a_run_under_way_shows_its_route_and_what_it_does_without_moving_the_table() {
    let theme = Theme::default();
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
    let shown = text(&detail_lines(&view, None, &theme, 120, None));
    let row = |shown: &str, name: &str| {
        shown
            .lines()
            .skip_while(|l| !l.contains("── runs"))
            .find(|l| l.split_whitespace().next() == Some(name))
            .unwrap_or_else(|| panic!("no {name} run in\n{shown}"))
            .to_string()
    };
    let live = row(&shown, "a-2");
    let words: Vec<&str> = live.split_whitespace().collect();
    assert!(
        words
            .windows(3)
            .any(|w| w == ["opus-5-5", "multi-file", "hard"]),
        "the model it went to and how it was classified: {live}"
    );
    let status = live
        .find("writing 49 files")
        .unwrap_or_else(|| panic!("{live}"));
    let cost = live.find("$0.80").unwrap_or_else(|| panic!("{live}"));
    assert!(cost < status, "the status comes after the columns: {live}");
    assert!(shown.contains("ansible-b748edea"), "{shown}");

    view.live[0].doing = Some(BenchDoing {
        what: "running run_command, read_file, write_file, and a long list of tools".to_string(),
        for_ms: 3_000,
        ..BenchDoing::default()
    });
    let longer = text(&detail_lines(&view, None, &theme, 120, None));
    for name in ["a-1", "a-2"] {
        let (before, after) = (row(&shown, name), row(&longer, name));
        let at = |line: &str, cell: &str| line.find(cell).unwrap_or_else(|| panic!("{line}"));
        for cell in ["$", "ansible-"] {
            assert_eq!(
                at(&before, cell),
                at(&after, cell),
                "{name}:\n{shown}\n{longer}"
            );
        }
    }
    for line in longer.lines() {
        assert!(display_width(line) <= 120, "{line}");
    }
}

#[test]
fn every_line_fits_the_width_and_separators_line_up() {
    let theme = Theme::default();
    let mut view = view();
    view.arms[1].label = "anthropic/claude-opus-5-5-with-a-very-long-name".to_string();
    view.prompts[0] = "a long prompt ".repeat(20);
    view.runs[0].model = "claude/claude-sonnet-5-5".to_string();
    view.runs[0].task = Some("swe_bench_pro_hard/ansible-40ade1f8-and-more".to_string());
    view.runs[0].error = Some("the copy could not be made ".repeat(10));
    view.notes = vec!["a note ".repeat(30)];
    view.live = vec![BenchLiveRun {
        name: "a-2".to_string(),
        task: Some("swe_bench_pro_hard/ansible-b748edea".to_string()),
        wall_ms: Some(5_000),
        doing: Some(BenchDoing {
            what: "writing a very long list of files ".repeat(4),
            for_ms: 3_000,
            ..BenchDoing::default()
        }),
        ..BenchLiveRun::default()
    }];
    if let Some(judge) = view.judge.as_mut() {
        judge.verdicts[0].reasoning = "b-1 is the better attempt ".repeat(20);
    }
    for width in [60, 80, 120] {
        let lines = detail_lines(&view, Some(0), &theme, width, None);
        let shown = text(&lines);
        for line in shown.lines() {
            assert!(display_width(line) <= width, "{width}: {line}");
        }
        let bars = |line: &str| -> Vec<usize> {
            line.chars()
                .enumerate()
                .filter(|(_, c)| matches!(c, '│' | '┼'))
                .map(|(i, _)| i)
                .collect()
        };
        let summary: Vec<_> = shown
            .lines()
            .skip(2)
            .take_while(|l| !l.starts_with("session:"))
            .filter(|l| l.contains('│') || l.contains('┼'))
            .map(bars)
            .collect();
        assert!(summary.len() > 3, "{shown}");
        assert!(summary.windows(2).all(|w| w[0] == w[1]), "{shown}");
    }
}

#[test]
fn bench_draws_the_benchmark_it_shows() {
    let mut app = App::new();
    app.process_command(UiCommand::BenchList {
        benches: vec![view()],
    });
    app.bench_mut().mode = Some(BenchMode::Detail("ab".to_string()));
    let theme = Theme::default();
    let mut terminal = Terminal::new(TestBackend::new(120, 80)).unwrap();
    terminal
        .draw(|frame| render_benchmarks(frame, frame.area(), &mut app, &theme, true))
        .unwrap();
    let screen = format!("{:?}", terminal.backend().buffer());
    assert!(screen.contains("Compare #1"), "{screen}");
    assert!(screen.contains("cost/run"), "{screen}");
    assert!(screen.contains("Esc list"), "{screen}");
}

#[test]
fn the_from_row_names_the_agent_and_its_project() {
    use crate::events::AgentSummary;
    let theme = Theme::default();
    let mut app = App::new();
    let shown = text(&form_lines(&app, &theme, 120));
    assert!(
        shown.contains("From          none yet: make an agent in PROJECTS first"),
        "{shown}"
    );

    app.process_command(UiCommand::SetAgents(vec![AgentSummary {
        id: "a".to_string(),
        name: "fixer".to_string(),
        project: Some("web".to_string()),
        ..AgentSummary::default()
    }]));
    let shown = text(&form_lines(&app, &theme, 120));
    assert!(
        shown.contains("←/→ picks the agent it runs from"),
        "{shown}"
    );
    let form = &mut app.bench_mut().form;
    form.from = Some("a".to_string());
    form.field = FormField::From;
    let shown = text(&form_lines(&app, &theme, 120));
    assert!(shown.contains("‹ fixer · web ›"), "{shown}");
}

#[test]
fn example_tasks_show_a_row_each_and_no_apply() {
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
    let shown = text(&detail_lines(&view, None, &theme, 120, None));
    assert!(
        shown.contains("coding · 2 runs per task and arm · own checks"),
        "{shown}"
    );
    assert!(!shown.contains("setting up"), "{shown}");
    assert!(shown.contains("── tasks ─"), "{shown}");
    let write = shown
        .lines()
        .find(|l| l.starts_with("write-file "))
        .unwrap_or_else(|| panic!("{shown}"));
    assert!(write.contains("2/2  $0.02  -93%"), "{write}");
    assert!(write.contains("1/2  $0.30"), "{write}");
    assert!(write.trim_end().ends_with('B'), "{write}");
    assert!(
        shown.lines().any(|l| l.starts_with("fix-shell-bug ")),
        "{shown}"
    );
    assert!(!shown.contains("coding/"), "{shown}");
    assert!(!shown.contains("y apply"), "{shown}");
    assert!(!shown.contains("prompts"), "{shown}");
}

#[test]
fn a_running_benchmark_shows_its_setup_and_the_runs_under_way() {
    use crate::events::BenchTaskRow;
    let theme = Theme::default();
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
    let shown = text(&detail_lines(&view, None, &theme, 140, Some(3)));
    assert!(
            shown.contains(
                "setting up: 1/4 tasks ready · 1 fetching · 1 queued · 1 failed · pulling an image can take minutes"
            ),
            "{shown}"
        );
    let row = |start: &str| {
        shown
            .lines()
            .find(|l| l.starts_with(start))
            .unwrap_or_else(|| panic!("{start}: {shown}"))
            .trim_end()
            .to_string()
    };
    assert!(row("flipt-2 ").ends_with("pulling image"), "{shown}");
    assert!(row("flipt-3 ").ends_with("queued"), "{shown}");
    assert!(row("navidrome-4 ").ends_with("failed: no image"), "{shown}");
    let live = |status: &str| {
        shown
            .lines()
            .skip_while(|l| !l.contains("── runs"))
            .find(|l| l.contains(status))
            .unwrap_or_else(|| panic!("{status}: {shown}"))
            .to_string()
    };
    let working = live("working");
    assert!(working.contains(" a-1 "), "{working}");
    assert!(working.contains("ansible-1"), "{working}");
    assert!(working.contains("1m12s"), "{working}");
    assert!(working.contains("$0.04"), "{working}");
    assert!(
        live("waiting for its task's files").contains(" b-2 "),
        "{shown}"
    );
    let finished = row("finished ");
    assert!(finished.contains("2/8, 1 working"), "{finished}");
    assert!(finished.ends_with("2/8"), "{finished}");
    assert!(row("cost ").contains("$0.16"), "spent so far: {shown}");
    assert!(row("A  sonnet-4-6 ").contains("42k"), "{shown}");

    view.live[0].doing = Some(BenchDoing {
        what: "running task".to_string(),
        for_ms: 492_000,
        subtask: Some(Box::new(BenchDoing {
            what: "thinking".to_string(),
            for_ms: 100_000,
            call: 4,
            restarts: 2,
            subtask: None,
        })),
        ..BenchDoing::default()
    });
    let shown = text(&detail_lines(&view, None, &theme, 200, Some(3)));
    let a_1 = shown
        .lines()
        .find(|l| l.contains(" a-1 ") && l.contains("ansible-1"))
        .unwrap_or_else(|| panic!("{shown}"));
    assert!(
        a_1.contains("running task 8m12s › thinking 1m40s · call 4 · restarted 2×"),
        "{shown}"
    );
}

#[test]
fn runs_show_in_number_order_with_a_rule_between_numbers() {
    let theme = Theme::default();
    let mut view = view();
    view.status = BenchStatus::Running;
    view.runs[0].name = "b-1".to_string();
    view.runs[0].arm = 1;
    let live = |name: &str, arm| BenchLiveRun {
        name: name.to_string(),
        arm,
        wall_ms: Some(1_000),
        ..BenchLiveRun::default()
    };
    view.live = vec![live("b-2", 1), live("a-1", 0), live("a-2", 0)];
    let shown = text(&detail_lines(&view, None, &theme, 140, None));
    let runs: Vec<&str> = shown
        .lines()
        .skip_while(|l| !l.starts_with("── runs"))
        .skip(1)
        .take_while(|l| !l.starts_with("note:"))
        .collect();
    let named: Vec<&str> = runs
        .iter()
        .filter_map(|l| {
            ["a-1", "b-1", "a-2", "b-2"]
                .into_iter()
                .find(|name| l.contains(&format!(" {name} ")))
        })
        .collect();
    assert_eq!(named, ["a-1", "b-1", "a-2", "b-2"], "{shown}");
    let at = |name: &str| {
        runs.iter()
            .position(|l| l.contains(&format!(" {name} ")))
            .unwrap()
    };
    let between = |from: &str, to: &str| {
        runs[at(from) + 1..at(to)]
            .iter()
            .filter(|l| l.trim().chars().all(|c| c == '─') && !l.trim().is_empty())
            .count()
    };
    assert_eq!(between("a-1", "b-1"), 0, "{shown}");
    assert_eq!(between("b-1", "a-2"), 1, "{shown}");
    assert_eq!(between("a-2", "b-2"), 0, "{shown}");
}

#[test]
fn runs_under_way_add_to_tokens_used_so_far() {
    let token = |model: &str, cost: f64| BenchTokens {
        model: model.to_string(),
        purpose: None,
        calls: 1,
        input_tokens: 1_000,
        output_tokens: 100,
        cost_usd: cost,
        unpriced_calls: 0,
    };
    let live = [
        BenchLiveRun {
            tokens: vec![token("opus", 0.5), token("haiku", 0.01)],
            ..BenchLiveRun::default()
        },
        BenchLiveRun {
            tokens: vec![
                token("sonnet", 0.2),
                BenchTokens {
                    unpriced_calls: 1,
                    ..token("sonnet", 0.0)
                },
            ],
            ..BenchLiveRun::default()
        },
    ];
    let used = tokens_so_far(&[token("sonnet", 0.1)], live.iter());
    let models: Vec<&str> = used.iter().map(|t| t.model.as_str()).collect();
    assert_eq!(models, ["opus", "sonnet", "haiku"]);
    assert_eq!((used[1].calls, used[1].unpriced_calls), (3, 1));
    assert!((used[1].cost_usd - 0.3).abs() < 1e-9);
}

#[test]
fn calls_beside_the_work_are_named_for_what_they_were_for() {
    let theme = Theme::default();
    let token = |model: &str, purpose: Option<&str>, calls, cost_usd| BenchTokens {
        model: model.to_string(),
        purpose: purpose.map(str::to_string),
        calls,
        cost_usd,
        ..BenchTokens::default()
    };
    let sonnet = "claude/claude-sonnet-5-5";
    let terra = "openrouter/openai/gpt-6.1-terra";
    let mut view = view();
    view.arms[0].tokens = vec![
        token(sonnet, None, 20, 0.28),
        token(sonnet, Some("judge"), 1, 0.02),
        token(terra, Some("criteria"), 1, 0.01),
    ];
    view.live = vec![BenchLiveRun {
        name: "a-2".to_string(),
        tokens: vec![token(terra, Some("criteria"), 2, 0.02)],
        ..BenchLiveRun::default()
    }];
    let shown = text(&detail_lines(&view, None, &theme, 120, None));
    let calls = |label: &str| {
        let row = (shown.lines())
            .skip_while(|l| !l.starts_with("── tokens"))
            .find(|l| l.contains(label))
            .unwrap_or_else(|| panic!("no {label:?} row in\n{shown}"));
        let (_, after) = row.split_once('│').unwrap();
        after.split_whitespace().next().unwrap().to_string()
    };
    assert_eq!(calls("A  sonnet-5-5 "), "20", "the work's own row");
    assert_eq!(calls("sonnet-5-5 · judge"), "1");
    assert_eq!(
        calls("gpt-6.1-terra · criteria"),
        "3",
        "a run under way adds to its row"
    );
}

/// The detail of a benchmark whose arm B ran on `glm-9`, a model with
/// no known price: `unpriced` of the arm's `calls` calls had none and
/// the rest cost `priced_usd`.
fn unpriced_view(calls: u32, unpriced: u32, priced_usd: f64) -> BenchView {
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

fn line_of<'s>(shown: &'s str, start: &str) -> &'s str {
    shown
        .lines()
        .find(|l| l.starts_with(start))
        .unwrap_or_else(|| panic!("no {start:?} line in\n{shown}"))
}

fn run_line<'s>(shown: &'s str, name: &str) -> &'s str {
    shown
        .lines()
        .skip_while(|l| !l.contains("── runs"))
        .find(|l| l.split_whitespace().next() == Some(name))
        .unwrap_or_else(|| panic!("no {name} run in\n{shown}"))
}

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

#[test]
fn runs_under_way_add_to_each_arms_total_time() {
    let theme = Theme::default();
    let mut view = view();
    view.status = BenchStatus::Running;
    view.runs.push(BenchRun {
        name: "b-1".to_string(),
        arm: 1,
        wall_ms: 3_600_000,
        ..BenchRun::default()
    });
    let live = |name: &str, arm, wall_ms| BenchLiveRun {
        name: name.to_string(),
        arm,
        wall_ms,
        ..BenchLiveRun::default()
    };
    view.live = vec![
        live("a-2", 0, Some(30_000)),
        live("b-2", 1, Some(300_000)),
        live("a-3", 0, None),
    ];
    let shown = text(&detail_lines(&view, None, &theme, 120, None));
    let row = |label: &str| {
        shown
            .lines()
            .find(|l| l.starts_with(&format!("{label} ")))
            .unwrap_or_else(|| panic!("no {label} row in\n{shown}"))
            .trim_end()
            .to_string()
    };
    let total = row("total time");
    assert!(total.contains(" 1m12s "), "42s ended, 30s so far: {total}");
    assert!(total.ends_with(" 1h05m"), "1h ended, 5m so far: {total}");
    assert!(row("time").contains(" 42s "), "the median run: {shown}");

    view.runs.clear();
    view.live.clear();
    let shown = text(&detail_lines(&view, None, &theme, 120, None));
    let total = shown
        .lines()
        .find(|l| l.starts_with("total time "))
        .unwrap_or_else(|| panic!("{shown}"));
    assert!(total.trim_end().ends_with(" -"), "nothing ran yet: {total}");
}

#[test]
fn running_and_working_shimmer_while_it_runs() {
    let theme = Theme::default();
    let mut view = view();
    view.status = BenchStatus::Running;
    view.live = vec![
        BenchLiveRun {
            name: "a-2".to_string(),
            wall_ms: Some(5_000),
            ..BenchLiveRun::default()
        },
        BenchLiveRun {
            name: "b-2".to_string(),
            arm: 1,
            ..BenchLiveRun::default()
        },
    ];
    // Lit when the word's letters are in more than one color.
    let lit = |line: &Line<'_>, word: &str| {
        let mut colors: Vec<_> = line
            .spans
            .iter()
            .filter(|s| !s.content.trim().is_empty() && word.contains(s.content.trim()))
            .map(|s| format!("{:?}", s.style.fg))
            .collect();
        colors.dedup();
        colors.len() > 1
    };
    let find = |lines: &[Line<'static>], status: &str| {
        let runs = lines
            .iter()
            .position(|l| text(std::slice::from_ref(l)).contains("── runs"))
            .unwrap_or_default();
        lines[runs..]
            .iter()
            .find(|l| text(std::slice::from_ref(*l)).contains(status))
            .cloned()
            .unwrap_or_else(|| panic!("{status}: {}", text(lines)))
    };

    // On frame 3 the band's center is on the label's first letter.
    let lines = detail_lines(&view, None, &theme, 140, Some(3));
    assert!(lit(&lines[0], "running"), "{:?}", lines[0]);
    assert!(lit(&find(&lines, "working"), "working"));
    let waiting = find(&lines, "waiting for its task's files");
    assert!(
        !lit(&waiting, "waiting for its task's files"),
        "{waiting:?}"
    );

    let still = detail_lines(&view, None, &theme, 140, None);
    assert!(!lit(&still[0], "running"), "{:?}", still[0]);
    assert!(!lit(&find(&still, "working"), "working"));

    view.status = BenchStatus::Done;
    let done = detail_lines(&view, None, &theme, 140, Some(3));
    assert!(!lit(&done[0], "done"), "{:?}", done[0]);
}

#[test]
fn the_form_offers_the_example_tasks_and_how_many() {
    let theme = Theme::default();
    let mut app = App::new();
    let state = app.bench_mut();
    state.examples = ["Write a file", "Fix a bug"]
        .map(|title| aura_ui::benchmarks::BenchExample {
            suite: "coding".to_string(),
            title: title.to_string(),
        })
        .to_vec();
    state.form.tasks = 2;
    state.form.work = 2;
    state.form.field = FormField::Tasks;
    let shown = text(&form_lines(&app, &theme, 120));
    assert!(
        shown.contains("example coding tasks, every suite (2)"),
        "{shown}"
    );
    assert!(
        shown.contains("‹ 2 of 2 · Write a file, Fix a bug ›"),
        "{shown}"
    );
    assert!(shown.contains("1 per task"), "{shown}");
    assert!(shown.contains("each task's own"), "{shown}");

    app.bench_mut().form.work = 0;
    let shown = text(&form_lines(&app, &theme, 120));
    assert!(
        shown.contains("(only when the work is example tasks)"),
        "{shown}"
    );
}

#[test]
fn a_by_difficulty_arm_shows_a_row_per_difficulty() {
    use crate::app::BY_DIFFICULTY;
    let theme = Theme::default();
    let mut app = App::new();
    app.bench_mut().form.b.clear();
    let shown = text(&form_lines(&app, &theme, 120));
    assert!(!shown.contains("Medium"), "{shown}");

    let form = &mut app.bench_mut().form;
    form.b = BY_DIFFICULTY.to_string();
    form.b_models = Default::default();
    form.b_models[0] = "openrouter/deepseek/deepseek-v3.2".to_string();
    form.field = FormField::BEasy;
    let shown = text(&form_lines(&app, &theme, 120));
    assert!(
        shown.contains("by difficulty · a model for easy, medium and hard work"),
        "{shown}"
    );
    assert!(
        shown.contains("‹ openrouter/deepseek/deepseek-v3.2 ›"),
        "{shown}"
    );
    assert!(shown.contains("  Medium"), "{shown}");
    assert!(
        shown.contains("Enter opens the model selector · ←/→ cycles"),
        "{shown}"
    );
    assert_eq!(
        short_model("difficulty:easy=a,medium=b,hard=c"),
        BY_DIFFICULTY
    );
}
