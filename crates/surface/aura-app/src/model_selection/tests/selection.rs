use super::*;

#[test]
fn openrouter_options_are_grouped_by_vendor() {
    assert_eq!(
        group_heading(
            ModelBackend::OpenRouter,
            "openrouter/anthropic/claude-sonnet-4.6"
        ),
        "OpenRouter · anthropic"
    );
    let (selection, _) = selection_on(signed_in(None, &[ProviderKind::OpenRouter]), LOCAL);
    let options = selection.options();
    let entry = options
        .iter()
        .find(|o| o.id.starts_with("openrouter/anthropic/"))
        .expect("curated OpenRouter anthropic model listed");
    assert_eq!(entry.group, "OpenRouter · anthropic");
}

#[test]
fn openrouter_models_need_the_openrouter_sign_in() {
    let id = "openrouter/anthropic/claude-sonnet-4.6";
    assert!(!usable(
        &signed_in(Some("alice"), &[ProviderKind::Claude]),
        id
    ));
    assert!(usable(&signed_in(None, &[ProviderKind::OpenRouter]), id));

    let (mut selection, mut config) = selection(Some("alice"));
    let err = selection.select(&mut config, id).unwrap_err();
    assert!(err.contains("/login openrouter"), "{err}");
    assert!(err.contains("OpenRouter account"), "{err}");
}

#[test]
fn a_thinking_level_pins_every_turn_and_survives_a_model_switch() {
    let (mut selection, mut config) = selection(Some("alice"));
    assert_eq!(config.user_thinking_effort, None);
    assert_eq!(selection.thinking_display(), "auto");

    selection
        .set_thinking(&mut config, Some("high"))
        .expect("a level");
    assert_eq!(config.user_thinking_effort, Some(ThinkingEffort::High));
    selection
        .select(&mut config, "aura-claude-sonnet-4-6")
        .expect("signed-in cloud model");
    assert_eq!(config.user_thinking_effort, Some(ThinkingEffort::High));
    let aura_ui::UiCommand::SetModels { thinking, .. } = selection.ui_command() else {
        panic!("expected SetModels");
    };
    assert_eq!(thinking.as_deref(), Some("high"));

    selection.set_thinking(&mut config, Some("off")).unwrap();
    assert_eq!(selection.thinking_display(), "off");
    assert!(selection.set_thinking(&mut config, Some("lots")).is_err());
    assert_eq!(config.user_thinking_effort, Some(ThinkingEffort::Off));

    selection.set_thinking(&mut config, None).unwrap();
    assert_eq!(config.user_thinking_effort, None);
}

#[test]
fn the_thinking_level_is_saved_with_the_model() {
    let home = tempfile::tempdir().unwrap();
    let (mut selection, mut config) = selection(None);
    selection.prefs_path = Some(home.path().join("tui.json"));
    selection.set_thinking(&mut config, Some("max")).unwrap();
    let prefs = load_prefs(&home.path().join("tui.json")).expect("saved");
    assert_eq!(prefs.thinking.as_deref(), Some("max"));
    assert_eq!(prefs.model.as_deref(), Some(LOCAL));
}

#[test]
fn guest_cannot_select_cloud_model() {
    let (mut selection, mut config) = selection(None);
    assert!(selection
        .select(&mut config, "aura-claude-sonnet-4-6")
        .is_err());
    assert_eq!(config.model, LOCAL);
}

#[test]
fn switching_backends_swaps_limits() {
    let (mut selection, mut config) = selection(Some("alice"));
    assert_eq!(
        config.max_context_tokens,
        Some(model_catalog::local_context_tokens())
    );

    selection
        .select(&mut config, "aura-claude-sonnet-4-6")
        .expect("logged-in user may pick cloud models");
    assert_eq!(config.model, "aura-claude-sonnet-4-6");
    assert_eq!(config.max_context_tokens, Some(1_000_000));
}

#[test]
fn gate_runs_turns_on_its_pick_with_that_models_limits() {
    let (selection, config) = selection_on(signed_in(Some("alice"), &[]), GATE_MODEL_ID);
    assert!(selection.is_gate());
    assert_eq!(selection.current(), GATE_MODEL_ID);
    assert_eq!(config.model, "aura-claude-sonnet-4-6");
    assert_eq!(config.max_context_tokens, Some(1_000_000));
    assert_eq!(selection.display(), "Gate → aura-claude-sonnet-4-6 (cloud)");
}

#[test]
fn gate_follows_sign_ins() {
    let (mut selection, mut config) = selection_on(AuthSummary::default(), GATE_MODEL_ID);
    assert_eq!(config.model, LOCAL, "nothing signed in: local models only");

    selection.set_auth(signed_in(Some("alice"), &[]));
    assert!(selection.refresh_route(&mut config));
    assert_eq!(config.model, "aura-claude-sonnet-4-6");

    selection.set_auth(signed_in(Some("alice"), &[ProviderKind::Claude]));
    assert!(selection.refresh_route(&mut config));
    assert_eq!(
        config.model, "claude/claude-sonnet-4-6",
        "the plan route wins over the cloud router"
    );
    assert!(!selection.refresh_route(&mut config));

    selection.set_auth(AuthSummary::default());
    assert!(!selection.ensure_usable(&mut config));
    assert_eq!(config.model, LOCAL);
}

#[test]
fn gate_with_nothing_runnable_points_at_the_local_default() {
    let (mut selection, mut config) = selection_on(AuthSummary::default(), GATE_MODEL_ID);
    selection.local_status = LocalServerStatus::Unreachable;
    selection.refresh_route(&mut config);
    assert_eq!(selection.route(), None);
    assert_eq!(config.model, LOCAL);
    assert!(selection
        .local_hint()
        .is_some_and(|hint| hint.contains("No local model server")));
}

#[test]
fn a_refused_model_is_dropped_until_sign_ins_change() {
    let claude = signed_in(None, &[ProviderKind::Claude]);
    let (mut selection, mut config) = selection_on(claude.clone(), GATE_MODEL_ID);
    assert_eq!(config.model, "claude/claude-sonnet-4-6");

    selection.drop_model("claude/claude-sonnet-4-6");
    assert!(!selection
        .gate_candidates()
        .iter()
        .any(|c| c.id == "claude/claude-sonnet-4-6"));
    assert!(selection.refresh_route(&mut config));
    assert_eq!(config.model, "claude/claude-opus-4-7");

    selection.set_auth(claude);
    selection.refresh_route(&mut config);
    assert_eq!(config.model, "claude/claude-sonnet-4-6");
}

#[test]
fn losing_a_sign_in_switches_to_gate() {
    let (mut selection, mut config) = selection(Some("alice"));
    selection
        .select(&mut config, "aura-claude-sonnet-4-6")
        .expect("select cloud");
    selection.set_auth(AuthSummary::default());
    assert!(selection.ensure_usable(&mut config));
    assert_eq!(selection.current(), GATE_MODEL_ID);
    assert_eq!(config.model, LOCAL);
}

#[test]
fn plan_models_need_their_own_link() {
    let (mut selection, mut config) = selection(Some("alice"));
    let err = selection
        .select(&mut config, "claude/claude-sonnet-4-6")
        .unwrap_err();
    assert!(err.contains("/login claude"), "{err}");

    selection.set_auth(signed_in(Some("alice"), &[ProviderKind::Claude]));
    selection
        .select(&mut config, "claude/claude-sonnet-4-6")
        .expect("linked plan");
    assert_eq!(config.max_context_tokens, Some(1_000_000));

    let options = selection.options();
    let plan = options
        .iter()
        .find(|o| o.id == "chatgpt/gpt-5.4")
        .expect("chatgpt models listed");
    assert!(!plan.available);
    assert_eq!(plan.provider.as_deref(), Some("chatgpt"));

    selection.set_auth(signed_in(Some("alice"), &[]));
    assert!(selection.ensure_usable(&mut config));
    assert_eq!(selection.current(), GATE_MODEL_ID);
    assert_eq!(
        config.model, "aura-claude-sonnet-4-6",
        "Gate routes across the sign-ins left"
    );
}

#[test]
fn the_cheapest_fast_model_classifies_else_the_local_default() {
    let (claude, _) = selection_on(signed_in(None, &[ProviderKind::Claude]), LOCAL);
    assert_eq!(claude.classifier_model(), "claude/claude-haiku-4-5");
    let (guest, _) = selection_on(AuthSummary::default(), GATE_MODEL_ID);
    assert_eq!(guest.classifier_model(), LOCAL);
}
