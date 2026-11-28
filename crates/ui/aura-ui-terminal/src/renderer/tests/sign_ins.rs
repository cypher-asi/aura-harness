use super::*;

#[test]
fn test_header_shows_logged_in_user() {
    let rows = render_with_user(Some("alice"), 120);
    assert!(rows[0].trim_end().ends_with("● alice (zOS)  ☰"));
    assert!(!rows[0].contains("local"));
}

#[test]
fn test_header_shows_linked_plans_without_zos() {
    let rows = render_rows(&mut app_with_auth(None, &["Claude Max 20x"]), 120);
    assert!(
        rows[0].trim_end().ends_with("● CLAUDE MAX 20X  ☰"),
        "{:?}",
        rows[0]
    );
    assert!(!rows[0].contains("local"));
}

#[test]
fn test_header_shows_only_one_sign_in() {
    let rows = render_rows(
        &mut app_with_auth(Some("alice"), &["ChatGPT Pro", "Claude"]),
        120,
    );
    assert!(
        rows[0].trim_end().ends_with("● alice (zOS)  ☰"),
        "{:?}",
        rows[0]
    );
    assert!(!rows[0].contains("CHATGPT PRO"), "{:?}", rows[0]);
    assert!(!rows[0].contains("CLAUDE"), "{:?}", rows[0]);
}

fn select_model(app: &mut crate::App, id: &str, provider: Option<&str>, route: Option<&str>) {
    use crate::events::{ModelOptionView, UiCommand};

    app.process_command(UiCommand::SetModels {
        options: vec![ModelOptionView {
            id: id.to_string(),
            label: id.to_string(),
            local: false,
            installed: true,
            group: "Cloud".to_string(),
            provider: provider.map(str::to_string),
            available: true,
            context_window: None,
        }],
        current: id.to_string(),
        route: route.map(str::to_string),
        user: app.user().map(str::to_string),
        plans: app.plans().to_vec(),
        thinking: None,
    });
}

#[test]
fn test_header_shows_the_sign_in_serving_the_model() {
    let mut app = app_with_auth(Some("alice"), &["Claude Max 20x", "OpenRouter"]);
    let header = |app: &mut crate::App| render_rows(app, 120)[0].trim_end().to_string();

    select_model(&mut app, "openrouter/qwen", Some("openrouter"), None);
    let shown = header(&mut app);
    assert!(shown.ends_with("● OPENROUTER  ☰"), "{shown:?}");
    assert!(!shown.contains("CLAUDE MAX 20X"), "{shown:?}");
    assert!(!shown.contains("alice"), "{shown:?}");

    select_model(&mut app, "claude/claude-sonnet-4-6", Some("claude"), None);
    let shown = header(&mut app);
    assert!(shown.ends_with("● CLAUDE MAX 20X  ☰"), "{shown:?}");
    assert!(!shown.contains("OPENROUTER"), "{shown:?}");
    assert!(!shown.contains("alice"), "{shown:?}");

    select_model(&mut app, "aura-claude-haiku-4-5", Some("zos"), None);
    let shown = header(&mut app);
    assert!(shown.ends_with("● alice (zOS)  ☰"), "{shown:?}");
    assert!(!shown.contains("CLAUDE MAX 20X"), "{shown:?}");
    assert!(!shown.contains("OPENROUTER"), "{shown:?}");
}

#[test]
fn test_header_follows_gates_pick() {
    let mut app = app_with_auth(Some("alice"), &["Claude Max 20x", "OpenRouter"]);
    select_model(
        &mut app,
        "gate",
        None,
        Some("claude-sonnet-4-6 (claude plan)"),
    );
    let header = render_rows(&mut app, 120)[0].trim_end().to_string();
    assert!(header.ends_with("● CLAUDE MAX 20X  ☰"), "{header:?}");
    assert!(!header.contains("OPENROUTER"), "{header:?}");
    assert!(!header.contains("alice"), "{header:?}");
}

#[test]
fn test_header_shows_the_first_sign_in_for_a_local_model() {
    let rows = render_rows(&mut app_with_auth(Some("alice"), &["Claude Max 20x"]), 120);
    let header = rows[0].trim_end();
    assert!(header.ends_with("● alice (zOS)  ☰"), "{header:?}");
    assert!(!header.contains("CLAUDE MAX 20X"), "{header:?}");
}

#[test]
fn test_input_shows_gate_instead_of_its_pick() {
    use crate::events::{ModelOptionView, UiCommand};

    let mut app = app_with_model(None);
    app.process_command(UiCommand::SetModels {
        options: vec![ModelOptionView {
            id: "gate".to_string(),
            label: "Gate".to_string(),
            local: false,
            installed: true,
            group: "Gate".to_string(),
            provider: None,
            available: true,
            context_window: None,
        }],
        current: "gate".to_string(),
        route: Some("claude-sonnet-4-6 (claude plan)".to_string()),
        user: None,
        plans: Vec::new(),
        thinking: None,
    });
    let rows = render_rows(&mut app, 120);
    let status = &rows[rows.len() - 1];
    assert!(status.contains("GATE"), "{status:?}");
    assert!(status.contains("● Ready"), "{status:?}");
    assert!(!status.contains("claude-sonnet-4-6"), "{status:?}");
}

#[test]
fn test_status_line_under_gate_counts_models_instead_of_context() {
    use crate::events::{CallStats, ModelOptionView, UiCommand};

    let mut app = app_with_model(None);
    app.process_command(UiCommand::SetModels {
        options: vec![ModelOptionView {
            id: "gate".to_string(),
            label: "Gate".to_string(),
            local: false,
            installed: true,
            group: "Gate".to_string(),
            provider: None,
            available: true,
            context_window: None,
        }],
        current: "gate".to_string(),
        route: None,
        user: None,
        plans: Vec::new(),
        thinking: None,
    });
    for model in ["aura-claude-haiku-4-5", "aura-claude-sonnet-4-6"] {
        app.process_command(UiCommand::CallStats(CallStats {
            model: model.to_string(),
            input_tokens: 40_000,
            cost_usd: Some(0.1),
            context_limit: Some(200_000),
            ..CallStats::default()
        }));
    }
    let rows = render_rows(&mut app, 180);
    let status = &rows[rows.len() - 1];
    assert!(!status.contains("context"), "{status:?}");
    assert!(status.contains("$0.200 · 2 models · 2 calls"), "{status:?}");
}

#[test]
fn test_status_line_on_one_model_measures_context_against_its_window() {
    use crate::events::{CallStats, ModelOptionView, UiCommand};

    let mut app = app_with_model(None);
    app.process_command(UiCommand::CallStats(CallStats {
        model: "aura-claude-sonnet-4-6".to_string(),
        input_tokens: 40_000,
        context_limit: Some(1_000_000),
        ..CallStats::default()
    }));
    app.process_command(UiCommand::SetModels {
        options: vec![ModelOptionView {
            id: "aura-claude-haiku-4-5".to_string(),
            label: "Claude Haiku 4.5".to_string(),
            local: false,
            installed: true,
            group: "Cloud (aura-router)".to_string(),
            provider: Some("zos".to_string()),
            available: true,
            context_window: Some(200_000),
        }],
        current: "aura-claude-haiku-4-5".to_string(),
        route: None,
        user: None,
        plans: Vec::new(),
        thinking: None,
    });
    let rows = render_rows(&mut app, 160);
    let status = &rows[rows.len() - 1];
    assert!(
        status.contains("Claude Haiku 4.5 · auto thinking · ◔ 20% context used"),
        "{status:?}"
    );
    assert!(!status.contains("model"), "{status:?}");
}
