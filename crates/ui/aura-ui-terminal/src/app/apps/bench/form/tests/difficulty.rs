use super::*;

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
