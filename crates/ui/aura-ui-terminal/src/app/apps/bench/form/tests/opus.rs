use super::*;

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
    app.account.models.clear();
    app.open_bench_form();
    assert_eq!(app.bench.form.b, DEFAULT_AGAINST, "kept until models load");
    let (tx, mut rx) = tokio::sync::mpsc::channel(4);
    app.link.event_tx = Some(tx);
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
