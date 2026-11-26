use super::*;

#[test]
fn a_plan_list_that_fails_to_load_falls_back_to_the_one_saved_last() {
    let home = tempfile::tempdir().unwrap();
    let claude = signed_in(None, &[ProviderKind::Claude]);
    let (mut selection, _) = selection_on(claude, GATE_MODEL_ID);
    let ids = |selection: &ModelSelection| -> Vec<String> {
        selection
            .gate_candidates()
            .into_iter()
            .filter(|c| c.backend == ModelBackend::Claude)
            .map(|c| c.id)
            .collect()
    };

    let note = selection
        .plans_listed(Err("timed out".to_string()))
        .unwrap();
    assert!(note.contains("timed out") && note.contains("built-in list"));
    assert!(ids(&selection).contains(&"claude/claude-sonnet-4-6".to_string()));

    selection.prefs_path = Some(home.path().join("tui.json"));
    let live = ["claude/claude-haiku-5-5", "claude/claude-sonnet-5-5"].map(|id| ModelOption {
        id: id.to_string(),
        label: id.to_string(),
        backend: ModelBackend::Claude,
        installed: true,
        tier: infer_tier(id),
    });
    assert_eq!(selection.plans_listed(Ok(live.to_vec())), None);
    assert_eq!(ids(&selection), live.clone().map(|m| m.id));

    let (mut later, _) = selection_on(signed_in(None, &[ProviderKind::Claude]), GATE_MODEL_ID);
    later.prefs_path = Some(home.path().join("tui.json"));
    let note = later.plans_listed(Err("timed out".to_string())).unwrap();
    assert!(note.contains("saved when it last loaded"), "{note}");
    assert_eq!(ids(&later), live.map(|m| m.id), "not the built-in list");
    assert_eq!(later.classifier_model(), "claude/claude-haiku-5-5");
}

fn openrouter() -> AuthSummary {
    signed_in(None, &[ProviderKind::OpenRouter])
}

/// The selector's entries, not Gate's candidates: Gate leaves out
/// OpenRouter models without a curated price, as these are.
fn openrouter_ids(selection: &ModelSelection) -> Vec<String> {
    selection
        .options()
        .into_iter()
        .filter(|o| backend_for(&o.id) == ModelBackend::OpenRouter)
        .map(|o| o.id)
        .collect()
}

fn live_openrouter_entries() -> Vec<(ModelOption, LiveModelInfo)> {
    [
        "openrouter/acme/saved-test-fast",
        "openrouter/acme/saved-test-pro",
    ]
    .into_iter()
    .map(|id| {
        let option = ModelOption {
            id: id.to_string(),
            label: id.to_string(),
            backend: ModelBackend::OpenRouter,
            installed: true,
            tier: infer_tier(id),
        };
        let info = LiveModelInfo {
            context_window: Some(123_456),
            max_output: Some(8_192),
            price: Some(LivePrice {
                input: 1.0,
                output: 2.0,
                cache_write: 1.0,
                cache_read: 0.5,
            }),
        };
        (option, info)
    })
    .collect()
}

/// A later failed listing falls back to the list saved in `home`, with its limits.
fn assert_saved_openrouter_list_is_used(home: &std::path::Path, live_ids: &[String]) {
    let (mut later, _) = selection_on(openrouter(), GATE_MODEL_ID);
    later.prefs_path = Some(home.join("tui.json"));
    let note = later
        .openrouter_listed(Err("timed out".to_string()))
        .unwrap();
    assert!(note.contains("timed out"), "{note}");
    assert!(note.contains("OpenRouter"), "{note}");
    assert!(note.contains("saved when it last loaded"), "{note}");
    assert!(
        note.contains("/model openrouter/<vendor>/<model>"),
        "{note}"
    );
    assert_eq!(openrouter_ids(&later), live_ids, "not the built-in list");
    let info = model_catalog::live_model_info("openrouter/acme/saved-test-pro")
        .expect("saved limits published");
    assert_eq!(info.context_window, Some(123_456));
    assert_eq!(info.max_output, Some(8_192));
    assert_eq!(info.price.map(|p| p.output), Some(2.0));
}

/// At startup, the saved list's prices are published before any
/// listing is fetched.
fn assert_saved_openrouter_prices_are_published_at_startup(home: &std::path::Path) {
    model_catalog::replace_live_models(ModelBackend::OpenRouter, []);
    assert_eq!(
        model_catalog::live_model_info("openrouter/acme/saved-test-pro"),
        None
    );
    publish_saved_openrouter_info(&home.join("tui.json"));
    let usage = aura_model::Usage::new(1_000_000, 1_000_000);
    assert_eq!(
        model_catalog::estimate_cost_usd("openrouter/acme/saved-test-pro", &usage),
        Some(3.0),
        "the saved price is used before any listing is fetched"
    );
    // Nothing saved: what is known stays.
    let empty = tempfile::tempdir().unwrap();
    publish_saved_openrouter_info(&empty.path().join("tui.json"));
    assert!(model_catalog::live_model_info("openrouter/acme/saved-test-pro").is_some());
}

#[test]
fn an_openrouter_list_that_fails_to_load_falls_back_to_the_one_saved_last() {
    let home = tempfile::tempdir().unwrap();
    let live = live_openrouter_entries();
    let live_ids: Vec<String> = live.iter().map(|(o, _)| o.id.clone()).collect();

    let (mut selection, _) = selection_on(openrouter(), GATE_MODEL_ID);
    selection.prefs_path = Some(home.path().join("tui.json"));
    assert_eq!(selection.openrouter_listed(Ok(live)), None);
    assert_eq!(openrouter_ids(&selection), live_ids);
    assert!(home.path().join(OPENROUTER_MODELS_FILE).exists());

    assert_saved_openrouter_list_is_used(home.path(), &live_ids);
    assert_saved_openrouter_prices_are_published_at_startup(home.path());

    let (mut fresh, _) = selection_on(openrouter(), GATE_MODEL_ID);
    let note = fresh.openrouter_listed(Err("offline".to_string())).unwrap();
    assert!(note.contains("built-in list"), "{note}");
}
