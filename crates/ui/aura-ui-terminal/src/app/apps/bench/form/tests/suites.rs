use super::*;

#[test]
fn values_cycle_both_ways() {
    let values: Vec<String> = ["gate", "x", "y"].iter().map(ToString::to_string).collect();
    assert_eq!(cycle(&values, "gate", true), "x");
    assert_eq!(cycle(&values, "gate", false), "y");
    assert_eq!(cycle(&values, "unknown", true), "gate");
}

#[test]
fn work_choices_offer_the_sessions_after_the_prompts() {
    let mut state = BenchState {
        sessions: vec![BenchSession {
            started_ms: 5,
            title: "fix it".to_string(),
            prompts: 3,
            cost_usd: Some(0.4),
            unpriced_calls: 0,
            models: Vec::new(),
            current: true,
        }],
        ..BenchState::default()
    };
    let choices = state.work_choices();
    assert_eq!(choices.len(), 4);
    assert_eq!(choices[2], WorkChoice::Examples(None));
    assert!(matches!(choices[3], WorkChoice::Session(s) if s.started_ms == 5));
    state.form.work = 3;
    assert!(matches!(state.work(), WorkChoice::Session(_)));
}

#[test]
fn default_form_runs_each_task_once_against_opus_5_5() {
    let form = BenchForm::default();
    assert_eq!(form.repeats, DEFAULT_REPEATS);
    assert_eq!(form.a, "gate");
    assert_eq!(form.b, DEFAULT_AGAINST);
    assert!(form.b_models.iter().all(String::is_empty));
    assert_eq!(form.arm_value(1), Ok(DEFAULT_AGAINST.to_string()));
    assert_eq!(BenchState::default().work(), WorkChoice::LastPrompt);
}

#[test]
fn sessions_select_the_full_default_suite_regardless_of_its_position() {
    let mut app = App::new();
    let id = app.active_agent_id.clone();
    let examples = vec![
        example("coding", "easy 1"),
        example(DEFAULT_SUITE, "hard 1"),
        example("gate_hard", "other 1"),
        example(DEFAULT_SUITE, "hard 2"),
    ];
    app.cmd_bench_sessions(&id, Vec::new(), examples);
    assert_eq!(app.bench.work(), WorkChoice::Examples(Some(DEFAULT_SUITE)));
    assert_eq!(app.bench.form.tasks, 2);
}

#[test]
fn default_submission_prepares_the_full_suite_against_opus_5_5() {
    let mut app = app_with_models(&[
        ("gate", "Gate"),
        ("openrouter/anthropic/claude-opus-5.5", "Claude Opus 5.5"),
        (OPUS_5_5, "Claude Opus 5.5"),
        ("claude/claude-sonnet-5-5", "Claude Sonnet 5.5"),
    ]);
    let id = app.active_agent_id.clone();
    let examples = vec![
        example("coding", "easy 1"),
        example(DEFAULT_SUITE, "hard 1"),
        example(DEFAULT_SUITE, "hard 2"),
    ];
    app.cmd_bench_sessions(&id, Vec::new(), examples);
    let (tx, mut rx) = tokio::sync::mpsc::channel(4);
    app.event_tx = Some(tx);

    app.submit_bench_form();
    let event = rx.try_recv().expect("prepare request");
    let UiEvent::Bench {
        request: BenchRequest::Prepare(spec),
        ..
    } = event
    else {
        panic!("expected a benchmark prepare request");
    };
    assert_eq!(
        spec.work,
        BenchWork::Examples {
            suite: Some(DEFAULT_SUITE.to_string()),
            tasks: 2,
        }
    );
    assert_eq!(spec.a, "gate");
    assert_eq!(spec.b, OPUS_5_5, "the Claude plan's Opus 5.5");
    assert_eq!(spec.repeats, 1);
}

#[test]
fn the_default_suite_is_explicit_even_when_it_is_the_only_suite() {
    let mut app = App::new();
    let id = app.active_agent_id.clone();
    app.cmd_bench_sessions(
        &id,
        Vec::new(),
        vec![example(DEFAULT_SUITE, "one"), example(DEFAULT_SUITE, "two")],
    );
    assert_eq!(app.bench.work(), WorkChoice::Examples(Some(DEFAULT_SUITE)));
    assert_eq!(app.bench.form.tasks, 2);
    assert!(app
        .bench
        .work_choices()
        .contains(&WorkChoice::Examples(Some(DEFAULT_SUITE))));
}

#[test]
fn absent_default_suite_falls_back_to_all_examples() {
    let mut app = App::new();
    let id = app.active_agent_id.clone();
    app.cmd_bench_sessions(
        &id,
        Vec::new(),
        vec![example("coding", "one"), example("gate_hard", "two")],
    );
    assert_eq!(app.bench.work(), WorkChoice::Examples(None));
    assert_eq!(app.bench.form.tasks, 2);

    let mut empty = App::new();
    let id = empty.active_agent_id.clone();
    empty.cmd_bench_sessions(&id, Vec::new(), Vec::new());
    assert_eq!(empty.bench.work(), WorkChoice::LastPrompt);
}

#[test]
fn refreshing_examples_preserves_a_user_chosen_work_selection() {
    let mut app = App::new();
    let id = app.active_agent_id.clone();
    let examples = vec![
        example("coding", "easy 1"),
        example(DEFAULT_SUITE, "hard 1"),
        example(DEFAULT_SUITE, "hard 2"),
    ];
    app.cmd_bench_sessions(&id, Vec::new(), examples.clone());
    app.bench.form.field = FormField::Work;
    app.change_form_value(true);
    assert_eq!(app.bench.work(), WorkChoice::LastPrompt);
    app.bench.form.tasks = 1;

    app.cmd_bench_sessions(&id, Vec::new(), examples);
    assert_eq!(app.bench.work(), WorkChoice::LastPrompt);
    assert_eq!(app.bench.form.tasks, 1);
}

#[test]
fn successful_submission_resets_to_the_full_cached_suite() {
    let mut app = App::new();
    let id = app.active_agent_id.clone();
    app.cmd_bench_sessions(
        &id,
        Vec::new(),
        vec![
            example("coding", "easy 1"),
            example(DEFAULT_SUITE, "hard 1"),
            example(DEFAULT_SUITE, "hard 2"),
        ],
    );
    app.bench.mode = Some(BenchMode::Form);
    app.bench.form.work = 1;
    app.bench.form.repeats = 3;
    app.cmd_bench_notice(Ok("applied".to_string()));
    assert_eq!(app.bench.form.repeats, 3, "a notice answering no start");

    app.bench.form.estimate = Some(Ok("about $1".to_string()));
    app.submit_bench_form();
    app.cmd_bench_notice(Ok("started".to_string()));
    assert_eq!(app.bench.work(), WorkChoice::Examples(Some(DEFAULT_SUITE)));
    assert_eq!(app.bench.form.tasks, 2);
    assert_eq!(app.bench.form.repeats, DEFAULT_REPEATS);
    assert_eq!(app.bench.form.b, DEFAULT_AGAINST);
}

#[test]
fn the_tasks_row_counts_from_one_to_the_example_tasks() {
    let mut app = App::new();
    let id = app.active_agent_id.clone();
    let titles: Vec<BenchExample> = (1..=7)
        .map(|i| example("coding", &format!("task {i}")))
        .collect();
    app.cmd_bench_sessions(&id, Vec::new(), titles);
    assert_eq!(app.bench.form.tasks, 7);
    app.bench.form.work = 2;
    app.bench.form.field = FormField::Tasks;
    for _ in 0..5 {
        app.change_form_value(true);
    }
    assert_eq!(app.bench.form.tasks, 7, "stops at the last task");
    for _ in 0..10 {
        app.change_form_value(false);
    }
    assert_eq!(app.bench.form.tasks, 1, "and at one");
    app.cmd_bench_sessions(&id, Vec::new(), vec![example("coding", "only")]);
    app.bench.form.tasks = 4;
    app.cmd_bench_sessions(&id, Vec::new(), vec![example("coding", "only")]);
    assert_eq!(app.bench.form.tasks, 1, "clamped to what there is");
}

#[test]
fn each_suite_can_be_run_on_its_own() {
    let mut app = App::new();
    let id = app.active_agent_id.clone();
    let mut examples: Vec<BenchExample> = (1..=5)
        .map(|i| example("coding", &format!("easy {i}")))
        .collect();
    examples.extend((1..=3).map(|i| example("gate_hard", &format!("hard {i}"))));
    app.cmd_bench_sessions(&id, Vec::new(), examples);
    let choices = app.bench.work_choices();
    assert_eq!(
        choices[2..],
        [
            WorkChoice::Examples(None),
            WorkChoice::Examples(Some("coding")),
            WorkChoice::Examples(Some("gate_hard")),
        ]
    );

    app.bench.form.field = FormField::Work;
    app.bench.form.work = 3;
    app.change_form_value(true);
    assert_eq!(app.bench.work(), WorkChoice::Examples(Some("gate_hard")));
    assert_eq!(app.bench.form.tasks, 3, "clamped to the suite's tasks");
    app.bench.form.field = FormField::Tasks;
    app.change_form_value(true);
    assert_eq!(app.bench.form.tasks, 3, "stops at the suite's last task");

    let (tx, mut rx) = tokio::sync::mpsc::channel(4);
    app.event_tx = Some(tx);
    app.submit_bench_form();
    let sent = std::iter::from_fn(|| rx.try_recv().ok()).find_map(|event| match event {
        UiEvent::Bench {
            request: BenchRequest::Prepare(spec),
            ..
        } => Some(spec.work),
        _ => None,
    });
    assert_eq!(
        sent,
        Some(BenchWork::Examples {
            suite: Some("gate_hard".to_string()),
            tasks: 3,
        })
    );
}
