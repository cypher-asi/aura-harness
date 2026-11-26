use super::*;

fn available(models: &[ModelOptionView]) -> Vec<ModelOptionView> {
    models
        .iter()
        .cloned()
        .map(|m| ModelOptionView {
            available: true,
            ..m
        })
        .collect()
}

#[test]
fn test_unlinked_plan_pick_starts_link_then_selects() {
    let (mut app, mut rx) = app_with_models(Some("alice"));
    app.handle_command("/model claude/claude-sonnet-4-6");
    assert!(matches!(
        rx.try_recv(),
        Ok(UiEvent::LinkSubscription { provider, paste: false }) if provider == "claude"
    ));

    app.process_command(UiCommand::SetModels {
        options: available(app.models()),
        current: "local/qwen3:4b".to_string(),
        route: None,
        user: Some("alice".to_string()),
        plans: Vec::new(),
        thinking: None,
    });
    assert!(matches!(
        rx.try_recv(),
        Ok(UiEvent::SelectModel(id)) if id == "claude/claude-sonnet-4-6"
    ));
}

#[test]
fn test_failed_link_drops_pending_model() {
    let (mut app, mut rx) = app_with_models(Some("alice"));
    app.handle_command("/model claude/claude-sonnet-4-6");
    let _ = rx.try_recv();
    app.process_command(UiCommand::PromptLinkCode {
        provider: "Claude".to_string(),
    });
    assert_eq!(app.state(), AppState::LinkCode);
    app.process_command(UiCommand::EndLink);
    assert_eq!(app.state(), AppState::Idle);

    app.process_command(UiCommand::SetModels {
        options: available(app.models()),
        current: "local/qwen3:4b".to_string(),
        route: None,
        user: Some("alice".to_string()),
        plans: Vec::new(),
        thinking: None,
    });
    assert!(rx.try_recv().is_err());
}

#[test]
fn test_login_and_logout_take_a_provider() {
    let (mut app, mut rx) = app_with_models(None);
    app.handle_command("/login chatgpt paste");
    assert!(matches!(
        rx.try_recv(),
        Ok(UiEvent::LinkSubscription { provider, paste: true }) if provider == "chatgpt"
    ));
    app.handle_command("/logout anthropic");
    assert!(matches!(rx.try_recv(), Ok(UiEvent::Unlink(p)) if p == "claude"));
    app.handle_command("/logout");
    assert!(matches!(rx.try_recv(), Ok(UiEvent::Logout)));
    app.handle_command("/login");
    assert_eq!(app.state(), AppState::LoginEmail);
}

#[test]
fn test_model_command_opens_selector_on_current() {
    let (mut app, _rx) = app_with_models(None);
    app.handle_command("/model");
    assert_eq!(app.state(), AppState::SelectingModel);
    assert_eq!(app.model_picker().cursor(), 0);

    app.handle_key(key(KeyCode::Esc));
    assert_eq!(app.state(), AppState::Idle);
}

#[test]
fn test_selector_enter_emits_select_model() {
    let (mut app, mut rx) = app_with_models(Some("alice"));
    app.handle_command("/model");
    app.handle_key(key(KeyCode::Down));
    app.handle_key(key(KeyCode::Enter));

    assert_eq!(app.state(), AppState::Idle);
    assert!(matches!(
        rx.try_recv(),
        Ok(UiEvent::SelectModel(id)) if id == "aura-claude-sonnet-4-6"
    ));
}

#[test]
fn test_selector_typing_filters_and_esc_clears_before_closing() {
    let (mut app, mut rx) = app_with_models(Some("alice"));
    app.handle_command("/model");
    for c in "sonnet".chars() {
        app.handle_key(key(KeyCode::Char(c)));
    }
    assert_eq!(app.model_picker().filter(), "sonnet");
    app.handle_key(key(KeyCode::Esc));
    assert_eq!(app.state(), AppState::SelectingModel);
    assert_eq!(app.model_picker().filter(), "");

    for c in "zzz".chars() {
        app.handle_key(key(KeyCode::Char(c)));
    }
    app.handle_key(key(KeyCode::Enter));
    assert_eq!(
        app.state(),
        AppState::SelectingModel,
        "no match keeps it open"
    );
    for _ in 0..3 {
        app.handle_key(key(KeyCode::Backspace));
    }
    for c in "sonnet".chars() {
        app.handle_key(key(KeyCode::Char(c)));
    }
    app.handle_key(key(KeyCode::Enter));
    assert_eq!(app.state(), AppState::Idle);
    assert!(matches!(
        rx.try_recv(),
        Ok(UiEvent::SelectModel(id)) if id == "aura-claude-sonnet-4-6"
    ));
}

#[test]
fn test_guest_cloud_pick_starts_login_then_selects() {
    let (mut app, mut rx) = app_with_models(None);
    app.handle_command("/model");
    // Signed-out cloud models live on the Sign-in tab.
    app.handle_key(key(KeyCode::Tab));
    app.handle_key(key(KeyCode::Enter));

    assert_eq!(app.state(), AppState::LoginEmail);
    assert!(rx.try_recv().is_err());

    app.process_command(UiCommand::SetModels {
        options: available(app.models()),
        current: "local/qwen3:4b".to_string(),
        route: None,
        user: Some("alice".to_string()),
        plans: Vec::new(),
        thinking: None,
    });
    assert!(matches!(
        rx.try_recv(),
        Ok(UiEvent::SelectModel(id)) if id == "aura-claude-sonnet-4-6"
    ));
}

#[test]
fn test_cancelled_login_drops_pending_model() {
    let (mut app, mut rx) = app_with_models(None);
    app.handle_command("/model Claude Sonnet 4.6");
    assert_eq!(app.state(), AppState::LoginEmail);
    app.handle_key(key(KeyCode::Esc));

    app.process_command(UiCommand::SetModels {
        options: available(app.models()),
        current: "local/qwen3:4b".to_string(),
        route: None,
        user: Some("alice".to_string()),
        plans: Vec::new(),
        thinking: None,
    });
    assert!(rx.try_recv().is_err());
}

#[test]
fn test_model_command_passes_unlisted_id_through() {
    let (mut app, mut rx) = app_with_models(None);
    app.handle_command("/model local/qwen3:8b");
    assert!(matches!(
        rx.try_recv(),
        Ok(UiEvent::SelectModel(id)) if id == "local/qwen3:8b"
    ));
    assert_eq!(app.state(), AppState::Idle);
}

#[test]
fn test_panel_commands_toggle_visibility() {
    let mut app = App::new();
    assert!(app.record_panel_visible());
    assert!(app.sandbox_panel_visible());
    assert!(app.swarm_panel_visible());

    app.handle_command("/kernel");
    app.handle_command("/files");
    app.handle_command("/agents");
    assert!(!app.record_panel_visible());
    assert!(!app.sandbox_panel_visible());
    assert!(!app.swarm_panel_visible());

    app.handle_command("/record");
    app.handle_command("/files");
    app.handle_command("/agents");
    assert!(app.record_panel_visible());
    assert!(app.sandbox_panel_visible());
    assert!(app.swarm_panel_visible());
}

#[test]
fn test_agents_command_refreshes_when_opened() {
    let (tx, mut rx) = mpsc::channel(10);
    let mut app = App::new().with_event_sender(tx);
    app.handle_command("/agents");
    assert!(rx.try_recv().is_err());
    app.handle_command("/agents");
    assert!(matches!(rx.try_recv(), Ok(UiEvent::RefreshAgents)));
}

#[test]
fn test_closing_focused_sandbox_returns_focus_to_chat() {
    let mut app = App::new();
    app.handle_key(key(KeyCode::Tab));
    app.handle_key(key(KeyCode::Tab));
    assert_eq!(app.focus(), PanelFocus::Sandbox);
    app.handle_command("/files");
    assert_eq!(app.focus(), PanelFocus::Chat);
}

#[test]
fn test_tab_cycles_visible_panels() {
    let mut app = App::new();
    let mut order = Vec::new();
    for _ in 0..4 {
        app.handle_key(key(KeyCode::Tab));
        order.push(app.focus());
    }
    assert_eq!(
        order,
        [
            PanelFocus::Records,
            PanelFocus::Sandbox,
            PanelFocus::Swarm,
            PanelFocus::Chat
        ]
    );

    app.handle_command("/record");
    app.handle_command("/agents");
    app.handle_key(key(KeyCode::Tab));
    assert_eq!(app.focus(), PanelFocus::Sandbox);
    app.handle_key(key(KeyCode::Tab));
    assert_eq!(app.focus(), PanelFocus::Chat);
}

#[test]
fn test_sandbox_keys_navigate_tree() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir(dir.path().join("src")).unwrap();
    std::fs::write(dir.path().join("src/lib.rs"), "").unwrap();
    std::fs::write(dir.path().join("notes.md"), "").unwrap();

    let mut app = App::new();
    app.process_command(UiCommand::SetSandboxRoot(dir.path().to_path_buf()));
    app.handle_key(key(KeyCode::Tab));
    app.handle_key(key(KeyCode::Tab));
    assert_eq!(app.focus(), PanelFocus::Sandbox);

    app.handle_key(key(KeyCode::Enter));
    let names: Vec<&str> = app
        .sandbox()
        .rows()
        .iter()
        .map(|r| r.name.as_str())
        .collect();
    assert_eq!(names, ["src", "lib.rs", "notes.md"]);

    app.handle_key(key(KeyCode::Down));
    app.handle_key(key(KeyCode::Left));
    assert_eq!(app.sandbox().selected(), 0);
    app.handle_key(key(KeyCode::Left));
    assert_eq!(app.sandbox().rows().len(), 2);
}

#[test]
fn test_arrows_move_input_cursor_when_panel_focused() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir(dir.path().join("src")).unwrap();

    let mut app = App::new();
    app.process_command(UiCommand::SetSandboxRoot(dir.path().to_path_buf()));
    app.handle_key(key(KeyCode::Tab));
    app.handle_key(key(KeyCode::Tab));
    assert_eq!(app.focus(), PanelFocus::Sandbox);

    app.handle_key(key(KeyCode::Char('a')));
    app.handle_key(key(KeyCode::Char('b')));
    app.handle_key(key(KeyCode::Left));
    assert_eq!(app.cursor_pos(), 1);
    app.handle_key(key(KeyCode::Right));
    assert_eq!(app.cursor_pos(), 2);
    assert_eq!(app.sandbox().rows().len(), 1);

    for focus in [PanelFocus::Records, PanelFocus::Swarm] {
        app.focus = focus;
        app.handle_key(key(KeyCode::Left));
        assert_eq!(app.cursor_pos(), 1);
        app.handle_key(key(KeyCode::Right));
        assert_eq!(app.cursor_pos(), 2);
    }
}

#[test]
fn test_sandbox_refreshes_after_tool_completes() {
    let dir = tempfile::tempdir().unwrap();
    let mut app = App::new();
    app.process_command(UiCommand::SetSandboxRoot(dir.path().to_path_buf()));
    assert!(app.sandbox().rows().is_empty());

    std::fs::write(dir.path().join("out.txt"), "").unwrap();
    app.process_command(UiCommand::CompleteTool {
        id: "t1".to_string(),
        result: String::new(),
        success: true,
    });
    assert!(app.sandbox().rows().is_empty(), "read on the next tick");
    app.tick();
    assert_eq!(app.sandbox().rows().len(), 1);
}

#[test]
fn test_notification_types() {
    assert_eq!(NotificationType::Success, NotificationType::Success);
    assert_ne!(NotificationType::Success, NotificationType::Warning);
    assert_ne!(NotificationType::Warning, NotificationType::Error);
}
