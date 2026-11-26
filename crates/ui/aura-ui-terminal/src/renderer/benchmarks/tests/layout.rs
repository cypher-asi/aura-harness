use super::*;

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
fn example_tasks_show_their_source_and_a_tasks_section() {
    let shown = example_tasks_shown();
    assert!(
        shown.contains("coding · 2 runs per task and arm · own checks"),
        "{shown}"
    );
    assert!(!shown.contains("setting up"), "{shown}");
    assert!(shown.contains("── tasks ─"), "{shown}");
}

#[test]
fn example_tasks_show_a_row_each() {
    let shown = example_tasks_shown();
    let write = line_of(&shown, "write-file ");
    assert!(write.contains("2/2  $0.02  -93%"), "{write}");
    assert!(write.contains("1/2  $0.30"), "{write}");
    assert!(write.trim_end().ends_with('B'), "{write}");
    assert!(
        shown.lines().any(|l| l.starts_with("fix-shell-bug ")),
        "{shown}"
    );
}

#[test]
fn example_tasks_show_no_category_prefix_apply_or_prompts() {
    let shown = example_tasks_shown();
    assert!(!shown.contains("coding/"), "{shown}");
    assert!(!shown.contains("y apply"), "{shown}");
    assert!(!shown.contains("prompts"), "{shown}");
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
