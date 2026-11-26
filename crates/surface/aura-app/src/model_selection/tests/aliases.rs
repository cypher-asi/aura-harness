use super::*;

#[test]
fn opus_alias_selects_the_claude_plan_opus_and_saves_that_id() {
    let home = tempfile::tempdir().unwrap();
    let (mut selection, mut config) =
        selection_on(signed_in(None, &[ProviderKind::Claude]), GATE_MODEL_ID);
    selection.prefs_path = Some(home.path().join("tui.json"));

    selection
        .select(&mut config, "opus")
        .expect("Claude plan Opus");

    assert_eq!(selection.current(), "claude/claude-opus-4-7");
    assert_eq!(config.model, "claude/claude-opus-4-7");
    let prefs = load_prefs(&home.path().join("tui.json")).expect("saved");
    assert_eq!(prefs.model.as_deref(), Some("claude/claude-opus-4-7"));
}

#[test]
fn sonnet_alias_selects_the_router_sonnet_when_signed_in_to_zos_only() {
    let (mut selection, mut config) = selection(Some("alice"));
    selection
        .select(&mut config, "sonnet")
        .expect("router Sonnet");
    assert_eq!(selection.current(), "aura-claude-sonnet-4-6");
}

#[test]
fn guest_alias_for_a_cloud_model_is_refused_with_its_sign_in() {
    let (mut selection, mut config) = selection(None);
    let err = selection.select(&mut config, "opus").unwrap_err();
    assert!(err.contains("/login"), "{err}");
    assert!(err.contains("\"opus\""), "{err}");
    // One `/login` unlocks the router's Opus, so the hint names it
    // rather than a plan's.
    assert!(
        err.starts_with("\"opus\" matches aura-claude-opus-4-7: "),
        "{err}"
    );
    assert_eq!(selection.current(), LOCAL);
    assert_eq!(config.model, LOCAL);
}

#[test]
fn a_full_model_name_is_selected_as_given() {
    for typed in [
        "claude-opus-4-6",
        "aura-claude-opus-4-6",
        "Claude-Sonnet-4-5-20250929",
    ] {
        let (mut selection, mut config) = selection(Some("alice"));
        assert_eq!(selection.resolve(typed), None, "{typed}");
        selection
            .select(&mut config, typed)
            .expect("router ids need only the zOS login");
        assert_eq!(selection.current(), typed);
        assert_eq!(config.model, typed);
    }
}

#[test]
fn a_listed_full_model_name_resolves_to_itself() {
    let (selection, _config) = selection(Some("alice"));
    assert_eq!(
        selection.resolve("AURA-CLAUDE-OPUS-4-7").as_deref(),
        Some("aura-claude-opus-4-7")
    );
}

#[test]
fn an_explicit_unlisted_id_is_selected_as_given() {
    let (mut selection, mut config) =
        selection_on(signed_in(None, &[ProviderKind::Claude]), GATE_MODEL_ID);
    selection
        .select(&mut config, "claude/claude-opus-4-1")
        .expect("any Claude plan model");
    assert_eq!(selection.current(), "claude/claude-opus-4-1");
}

#[test]
fn gate_is_selected_by_its_id_and_label() {
    for typed in ["gate", "Gate"] {
        let (mut selection, mut config) = selection(Some("alice"));
        selection.select(&mut config, typed).expect("gate");
        assert!(selection.is_gate(), "{typed}");
    }
}

#[test]
fn unknown_short_name_is_kept_as_a_router_id() {
    let (mut selection, mut config) = selection(Some("alice"));
    assert_eq!(selection.resolve("zzz-unknown"), None);
    selection
        .select(&mut config, "zzz-unknown")
        .expect("router ids need only the zOS login");
    assert_eq!(selection.current(), "zzz-unknown");
}

#[test]
fn only_short_names_other_than_gate_may_be_aliases() {
    for (typed, alias) in [
        ("opus", true),
        ("sonnet 4.5", true),
        ("claude opus", true),
        ("opus-4.6", true),
        ("gpt", true),
        ("claude-opus", true),
        ("aura-claude-opus-4-7", false),
        ("claude-opus-4-6", false),
        ("gpt-5.4", false),
        ("GPT-5.4", false),
        ("gate", false),
        (" Gate ", false),
        ("claude/claude-opus-4-7", false),
        ("local/qwen3:4b", false),
        ("  ", false),
    ] {
        assert_eq!(may_be_alias(typed), alias, "{typed:?}");
    }
}

#[test]
fn deepseek_alias_picks_the_best_live_openrouter_deepseek() {
    let (mut selection, mut config) =
        selection_on(signed_in(None, &[ProviderKind::OpenRouter]), GATE_MODEL_ID);
    let live = [
        "openrouter/deepseek/deepseek-chat-v3.1",
        "openrouter/deepseek/deepseek-v3.2",
        "openrouter/deepseek/deepseek-v3.2-exp",
        "openrouter/deepseek/deepseek-r1",
        "openrouter/anthropic/claude-opus-4.7",
    ]
    .map(|id| ModelOption {
        id: id.to_string(),
        label: id.to_string(),
        backend: ModelBackend::OpenRouter,
        installed: true,
        tier: infer_tier(id),
    });
    // Set directly rather than through `openrouter_listed`, which
    // publishes process-wide limits other tests read.
    selection.set_backend_models(ModelBackend::OpenRouter, live.to_vec());

    selection
        .select(&mut config, "deepseek")
        .expect("OpenRouter DeepSeek");
    assert_eq!(selection.current(), "openrouter/deepseek/deepseek-v3.2");
}
