use super::*;
use crate::events::UiEvent;
use crossterm::event::KeyModifiers;

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

fn example(suite: &str, title: &str) -> BenchExample {
    BenchExample {
        suite: suite.to_string(),
        title: title.to_string(),
    }
}

fn session(started_ms: u64) -> BenchSession {
    BenchSession {
        started_ms,
        title: "fix it".to_string(),
        prompts: 1,
        cost_usd: None,
        unpriced_calls: 0,
        models: Vec::new(),
        current: false,
    }
}

/// The benchmark requests sent since the last call, as
/// `<agent> <request>`.
fn requests(rx: &mut tokio::sync::mpsc::Receiver<UiEvent>) -> Vec<String> {
    std::iter::from_fn(|| rx.try_recv().ok())
        .filter_map(|event| match event {
            UiEvent::Bench { agent_id, request } => Some(format!("{agent_id} {request:?}")),
            _ => None,
        })
        .collect()
}

/// Top-level agents `a`, in project `web`, and `b`, with `kid` under
/// `a` on screen, a benchmark's run and an archived agent between.
fn app_with_trees() -> App {
    let mut app = App::new();
    let agent = |id: &str, depth| AgentSummary {
        id: id.to_string(),
        name: id.to_string(),
        depth,
        ..AgentSummary::default()
    };
    app.agents = vec![
        AgentSummary {
            project: Some("web".to_string()),
            ..agent("a", 0)
        },
        agent("kid", 1),
        AgentSummary {
            bench_run: true,
            ..agent("run", 0)
        },
        AgentSummary {
            archived_at_ms: Some(1),
            ..agent("old", 0)
        },
        agent("b", 0),
    ];
    app.active_agent_id = "kid".to_string();
    app
}

#[test]
fn the_from_row_starts_on_the_tree_on_screen_and_cycles_top_level_agents() {
    let mut app = app_with_trees();
    let (tx, mut rx) = tokio::sync::mpsc::channel(8);
    app.event_tx = Some(tx);
    app.open_bench_form();
    assert_eq!(app.bench.form.from.as_deref(), Some("a"), "kid's tree");
    assert_eq!(app.bench_from_label().as_deref(), Some("a · web"));
    let offered: Vec<&str> = app
        .bench_from_choices()
        .iter()
        .map(|a| a.id.as_str())
        .collect();
    assert_eq!(offered, ["a", "b"], "no child, run or archived agent");

    app.bench.sessions = vec![session(5)];
    app.bench.form.work = 3;
    assert!(matches!(app.bench.work(), WorkChoice::Session(_)));
    app.bench.form.field = FormField::From;
    app.change_form_value(true);
    assert_eq!(app.bench.form.from.as_deref(), Some("b"));
    assert!(app.bench.sessions.is_empty(), "a's sessions go");
    assert_eq!(app.bench.form.work, DEFAULT_WORK, "and so does the pick");
    app.change_form_value(true);
    assert_eq!(app.bench.form.from.as_deref(), Some("a"), "and wraps");
    assert_eq!(
        requests(&mut rx),
        ["a Sessions", "b Sessions", "a Sessions"]
    );

    app.cmd_bench_sessions("b", vec![session(7)], Vec::new());
    assert!(app.bench.sessions.is_empty(), "another agent's sessions");
    app.cmd_bench_sessions("a", vec![session(7)], Vec::new());
    assert_eq!(app.bench.sessions.len(), 1);
}

#[test]
fn the_benchmark_is_prepared_and_started_from_the_agent_on_the_from_row() {
    let mut app = app_with_trees();
    app.open_bench_form();
    app.bench.form.field = FormField::From;
    app.change_form_value(true);
    let (tx, mut rx) = tokio::sync::mpsc::channel(8);
    app.event_tx = Some(tx);
    app.bench.form.field = FormField::Go;

    app.submit_bench_form();
    app.cmd_bench_estimate("a", Ok("about $1".to_string()));
    assert!(app.bench.form.waiting, "another agent's estimate");
    app.cmd_bench_estimate("b", Ok("about $2".to_string()));
    assert!(app.bench.form.ready());
    app.submit_bench_form();
    let sent = requests(&mut rx);
    assert_eq!(sent.len(), 2, "{sent:?}");
    assert!(sent[0].starts_with("b Prepare("), "{sent:?}");
    assert_eq!(sent[1], "b Start");
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

const OPUS_5_5: &str = "claude/claude-opus-5-5";

#[test]
fn opus_5_5_is_found_on_any_route_but_not_as_a_variant() {
    for id in [
        OPUS_5_5,
        "aura-claude-opus-5-5",
        "openrouter/anthropic/claude-opus-5.5",
    ] {
        assert!(is_opus_5_5(id), "{id}");
    }
    for id in [
        "claude/claude-opus-5-5-fast",
        "claude/claude-opus-4-7",
        "claude/claude-sonnet-5-5",
        "openrouter/anthropic/claude-opus-5.55",
    ] {
        assert!(!is_opus_5_5(id), "{id}");
    }

    let openrouter = "openrouter/anthropic/claude-opus-5.5";
    let app = app_with_models(&[("gate", "Gate"), (openrouter, "Claude Opus 5.5")]);
    assert_eq!(app.default_bench_against(), openrouter, "without a plan");
    let none = app_with_models(&[("claude/claude-opus-4-7", "Claude Opus 4.7")]);
    assert_eq!(
        none.default_bench_against(),
        "",
        "the server's strongest Opus stands in"
    );
}

#[test]
fn arm_b_starts_on_the_runnable_opus_5_5_and_cycles_from_it() {
    let mut app = app_with_models(&[
        ("gate", "Gate"),
        ("claude/claude-sonnet-5-5", "Claude Sonnet 5.5"),
        (OPUS_5_5, "Claude Opus 5.5"),
        ("chatgpt/gpt-5.4", "GPT-5.4"),
    ]);
    app.open_bench_form();
    assert_eq!(app.bench.form.b, OPUS_5_5);
    app.bench.form.field = FormField::B;
    app.change_form_value(true);
    assert_eq!(app.bench.form.b, "chatgpt/gpt-5.4");
    app.change_form_value(false);
    assert_eq!(app.bench.form.b, OPUS_5_5, "and back to the default");
    app.change_form_value(false);
    assert_eq!(app.bench.form.b, "claude/claude-sonnet-5-5");
}

#[test]
fn with_no_model_list_the_default_arm_b_is_sent_as_the_strongest_opus() {
    let mut app = App::new();
    app.models.clear();
    app.open_bench_form();
    assert_eq!(app.bench.form.b, DEFAULT_AGAINST, "kept until models load");
    let (tx, mut rx) = tokio::sync::mpsc::channel(4);
    app.event_tx = Some(tx);
    app.submit_bench_form();
    let sent = std::iter::from_fn(|| rx.try_recv().ok()).find_map(|event| match event {
        UiEvent::Bench {
            request: BenchRequest::Prepare(spec),
            ..
        } => Some(spec.b),
        _ => None,
    });
    assert_eq!(sent.as_deref(), Some(""));
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

/// An app offering `ids`, each available, from the provider its prefix
/// names.
fn app_with_models(ids: &[(&str, &str)]) -> App {
    let mut app = App::new();
    app.models = ids
        .iter()
        .map(|(id, label)| crate::events::ModelOptionView {
            id: (*id).to_string(),
            label: (*label).to_string(),
            local: false,
            installed: true,
            group: id.split('/').next().unwrap_or_default().to_string(),
            provider: None,
            available: true,
            context_window: None,
        })
        .collect();
    app
}

const DEEPSEEK: &str = "openrouter/deepseek/deepseek-v3.2";

fn three_models() -> App {
    app_with_models(&[
        ("gate", "Gate"),
        ("claude/claude-sonnet-4-6", "Claude Sonnet 4.6"),
        (DEEPSEEK, "DeepSeek V3.2"),
        ("chatgpt/gpt-5.4", "GPT-5.4"),
    ])
}

#[test]
fn difficulty_rows_show_only_for_a_by_difficulty_arm() {
    let mut app = three_models();
    app.bench.form.b.clear();
    assert!(app
        .bench_arm_choices(true)
        .contains(&BY_DIFFICULTY.to_string()));
    app.bench.form.field = FormField::B;
    app.move_form_field(true);
    assert_eq!(
        app.bench.form.field,
        FormField::Classifier,
        "skips hidden rows"
    );

    app.bench.form.b = BY_DIFFICULTY.to_string();
    app.bench.form.field = FormField::B;
    app.move_form_field(true);
    assert_eq!(app.bench.form.field, FormField::BEasy);
    app.bench.form.field = FormField::Classifier;
    app.move_form_field(false);
    assert_eq!(app.bench.form.field, FormField::BHard);
    assert!(!app.bench.form.shown(FormField::AEasy));
}

#[test]
fn a_difficulty_row_cycles_models_and_enter_opens_the_model_selector() {
    let mut app = three_models();
    let session = app.current_model.clone();
    let (tx, mut rx) = tokio::sync::mpsc::channel(8);
    app.event_tx = Some(tx);
    app.bench.mode = Some(BenchMode::Form);
    app.bench.form.a = BY_DIFFICULTY.to_string();
    app.bench.form.field = FormField::AEasy;
    app.change_form_value(true);
    assert_eq!(app.bench.form.a_models[0], "claude/claude-sonnet-4-6");
    app.change_form_value(false);
    assert_eq!(
        app.bench.form.a_models[0], "chatgpt/gpt-5.4",
        "only models, no Gate"
    );

    let key = |code| KeyEvent::new(code, KeyModifiers::NONE);
    assert!(app.bench_form_key(key(KeyCode::Enter)));
    assert_eq!(app.state, AppState::SelectingModel);
    assert_eq!(
        app.model_picker.target(),
        PickerTarget::BenchDifficulty {
            arm: 0,
            difficulty: 0
        }
    );
    assert_eq!(app.picker_marked(), "chatgpt/gpt-5.4", "the row's model");
    for c in "deep".chars() {
        app.handle_key(key(KeyCode::Char(c)));
    }
    app.handle_key(key(KeyCode::Enter));
    assert_eq!(app.state, AppState::Idle);
    assert_eq!(app.bench.form.a_models[0], DEEPSEEK);
    assert_eq!(app.bench.form.field, FormField::AMedium, "on to the next");
    assert_eq!(
        app.current_model, session,
        "the session's model is untouched"
    );
    assert!(
        !std::iter::from_fn(|| rx.try_recv().ok())
            .any(|event| matches!(event, UiEvent::SelectModel(_))),
        "nothing selects a model for the session"
    );
}

#[test]
fn typing_on_a_difficulty_row_opens_the_selector_filtered() {
    let mut app = three_models();
    app.bench.mode = Some(BenchMode::Form);
    app.bench.form.b = BY_DIFFICULTY.to_string();
    app.bench.form.field = FormField::BHard;
    app.input.text = "gpt".to_string();
    assert!(app.bench_fill_field());
    assert_eq!(app.state, AppState::SelectingModel);
    assert_eq!(app.model_picker.filter(), "gpt");
    assert!(app.input.text.is_empty());
    app.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
    assert_eq!(app.bench.form.b_models[2], "chatgpt/gpt-5.4");
}

#[test]
fn the_selector_refuses_a_model_that_needs_a_sign_in_for_a_row() {
    let mut app = three_models();
    app.models.push(crate::events::ModelOptionView {
        id: "openrouter/qwen/qwen3-coder".to_string(),
        label: "Qwen3 Coder".to_string(),
        local: false,
        installed: true,
        group: "openrouter".to_string(),
        provider: Some("openrouter".to_string()),
        available: false,
        context_window: None,
    });
    app.bench.mode = Some(BenchMode::Form);
    app.bench.form.a = BY_DIFFICULTY.to_string();
    app.open_bench_model_picker(FormField::AMedium, "");
    app.pick_bench_model(FormField::AMedium, "openrouter/qwen/qwen3-coder");
    assert_eq!(app.state, AppState::SelectingModel, "stays open");
    assert!(app.bench.form.a_models[1].is_empty());
    assert!(app.notifications.current.as_ref().is_some_and(|(text, _)| {
        text == "Qwen3 Coder needs a sign-in first: /login openrouter"
    }));

    app.handle_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
    assert_eq!(app.state, AppState::Idle, "Esc leaves the row as it was");
    assert!(app.bench.form.a_models[1].is_empty());
}

#[test]
fn a_by_difficulty_arm_needs_all_three_models_and_is_sent_as_one_string() {
    let mut app = three_models();
    app.bench.form.b = BY_DIFFICULTY.to_string();
    app.bench.form.b_models = [DEEPSEEK.to_string(), String::new(), String::new()];
    let (tx, mut rx) = tokio::sync::mpsc::channel(4);
    app.event_tx = Some(tx);
    app.bench.form.field = FormField::Go;
    app.submit_bench_form();
    assert_eq!(app.bench.form.field, FormField::BMedium);
    assert!(app
        .notifications
        .current
        .as_ref()
        .is_some_and(|(text, _)| text == "Pick a model for medium work of arm B"));

    app.bench.form.b_models[1] = "claude/claude-sonnet-4-6".to_string();
    app.bench.form.b_models[2] = "chatgpt/gpt-5.4".to_string();
    app.submit_bench_form();
    let sent = std::iter::from_fn(|| rx.try_recv().ok()).find_map(|event| match event {
        UiEvent::Bench {
            request: BenchRequest::Prepare(spec),
            ..
        } => Some(spec.b),
        _ => None,
    });
    assert_eq!(
        sent.as_deref(),
        Some(
            "difficulty:easy=openrouter/deepseek/deepseek-v3.2,\
                 medium=claude/claude-sonnet-4-6,hard=chatgpt/gpt-5.4"
        )
    );
}

#[test]
fn the_classifier_row_starts_on_haiku_cycles_models_and_is_sent() {
    let mut app = three_models();
    app.bench.form.b.clear();
    app.bench.mode = Some(BenchMode::Form);
    app.bench.form.field = FormField::B;
    app.move_form_field(true);
    assert_eq!(
        app.bench.form.field,
        FormField::Classifier,
        "after the arms"
    );
    assert_eq!(app.bench.form.classifier, DEFAULT_CLASSIFIER);
    assert_eq!(
        app.bench.form.classifier_model(),
        Some(DEFAULT_CLASSIFIER.to_string()),
        "haiku is sent for the server to find"
    );

    app.change_form_value(true);
    assert_eq!(
        app.bench.form.classifier, AUTO_CLASSIFIER,
        "auto comes first"
    );
    assert_eq!(app.bench.form.classifier_model(), None);
    app.change_form_value(true);
    assert_eq!(app.bench.form.classifier, "claude/claude-sonnet-4-6");
    app.change_form_value(false);
    assert_eq!(app.bench.form.classifier, AUTO_CLASSIFIER);
    app.change_form_value(false);
    assert_eq!(app.bench.form.classifier, "chatgpt/gpt-5.4", "and wraps");

    let (tx, mut rx) = tokio::sync::mpsc::channel(4);
    app.event_tx = Some(tx);
    app.bench.form.field = FormField::Go;
    app.submit_bench_form();
    let sent = std::iter::from_fn(|| rx.try_recv().ok()).find_map(|event| match event {
        UiEvent::Bench {
            request: BenchRequest::Prepare(spec),
            ..
        } => Some(spec.classifier),
        _ => None,
    });
    assert_eq!(sent, Some(Some("chatgpt/gpt-5.4".to_string())));
}

#[test]
fn enter_on_the_classifier_row_picks_it_in_the_selector_and_backspace_resets_it() {
    let mut app = three_models();
    app.bench.mode = Some(BenchMode::Form);
    app.bench.form.field = FormField::Classifier;
    assert!(app.bench_form_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)));
    assert_eq!(app.state, AppState::SelectingModel);
    assert_eq!(app.model_picker.target(), PickerTarget::BenchClassifier);

    app.pick_bench_model(FormField::Classifier, DEEPSEEK);
    assert_eq!(app.state, AppState::Idle);
    assert_eq!(app.bench.form.classifier, DEEPSEEK);
    assert_eq!(app.bench.form.field, FormField::Repeats, "on to the next");
    assert_eq!(
        app.current_model,
        App::new().current_model,
        "session unchanged"
    );

    app.bench.form.field = FormField::Classifier;
    assert!(app.bench_form_key(KeyEvent::new(KeyCode::Backspace, KeyModifiers::NONE)));
    assert_eq!(app.bench.form.classifier, DEFAULT_CLASSIFIER);
}

#[test]
fn opening_the_form_names_the_runnable_haiku_and_keeps_a_chosen_classifier() {
    const HAIKU: &str = "claude/claude-haiku-4-5";
    let mut app = app_with_models(&[
        ("gate", "Gate"),
        ("claude/claude-sonnet-4-6", "Claude Sonnet 4.6"),
        (HAIKU, "Claude Haiku 4.5"),
    ]);
    app.open_bench_form();
    assert_eq!(app.bench.form.classifier, HAIKU);
    assert_eq!(app.bench.form.classifier_model(), Some(HAIKU.to_string()));

    app.bench.form.classifier = AUTO_CLASSIFIER.to_string();
    app.open_bench_form();
    assert_eq!(
        app.bench.form.classifier, AUTO_CLASSIFIER,
        "a classifier the user chose stays"
    );

    app.bench.form.field = FormField::Classifier;
    assert!(app.bench_form_key(KeyEvent::new(KeyCode::Backspace, KeyModifiers::NONE)));
    assert_eq!(
        app.bench.form.classifier, HAIKU,
        "backspace goes back to Haiku"
    );
}
