use super::*;

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
    app.link.event_tx = Some(tx);
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
    assert_eq!(app.state(), AppState::SelectingModel);
    assert_eq!(
        app.account.model_picker.target(),
        PickerTarget::BenchClassifier
    );

    app.pick_bench_model(FormField::Classifier, DEEPSEEK);
    assert_eq!(app.state(), AppState::Idle);
    assert_eq!(app.bench.form.classifier, DEEPSEEK);
    assert_eq!(app.bench.form.field, FormField::Repeats, "on to the next");
    assert_eq!(
        app.account.current_model,
        App::new().account.current_model,
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
