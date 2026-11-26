use super::*;

#[test]
fn test_folder_picker_overlay_lists_the_launch_directory() {
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

    let mut app = crate::App::new();
    app.handle_key(KeyEvent::new(KeyCode::Char('n'), KeyModifiers::CONTROL));
    let rows = render_rows(&mut app, 120);
    let screen = rows.join("\n");
    assert!(screen.contains("Select project folder"));
    assert!(screen.contains("› ✓ Use this folder"));
    assert!(screen.contains("▰ src"));
    assert!(screen.contains("n new folder"));

    app.handle_key(KeyEvent::new(KeyCode::Char('n'), KeyModifiers::NONE));
    for c in "demo".chars() {
        app.handle_key(KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE));
    }
    let screen = render_rows(&mut app, 120).join("\n");
    assert!(screen.contains("New folder: demo"));
    assert!(screen.contains("Enter create  Esc stop"));
}

#[test]
fn test_thinking_level_sits_after_the_model() {
    use crate::events::UiCommand;

    let mut app = app_with_model(None);
    let footer = |app: &mut crate::App| {
        render_rows(app, 120)
            .into_iter()
            .find(|row| row.contains("qwen3:4b"))
            .unwrap()
    };
    assert!(footer(&mut app).contains("qwen3:4b · auto thinking"));

    app.process_command(UiCommand::SetModels {
        options: app.models().to_vec(),
        current: "local/qwen3:4b".to_string(),
        route: None,
        user: None,
        plans: Vec::new(),
        thinking: Some("high".to_string()),
    });
    assert!(footer(&mut app).contains("qwen3:4b · high thinking"));
}

#[test]
fn test_thinking_level_outlasts_the_stats_after_a_turn() {
    use crate::events::{CallStats, UiCommand};

    let mut app = app_with_model(None);
    app.process_command(UiCommand::SetApproval(crate::ApprovalLevel::Ask));
    app.process_command(UiCommand::CallStats(CallStats {
        model: "local/qwen3:4b".to_string(),
        input_tokens: 4_000,
        cache_read_tokens: 120_000,
        cost_usd: Some(3.42),
        context_limit: Some(200_000),
        ..CallStats::default()
    }));
    let footer = |app: &mut crate::App, width| {
        render_rows(app, width)
            .into_iter()
            .find(|row| row.contains("qwen3:4b"))
            .expect("the footer shows the model")
    };
    let narrow = footer(&mut app, 120);
    assert!(
        narrow.contains("ASK · qwen3:4b · auto thinking"),
        "{narrow:?}"
    );
    assert!(
        !narrow.contains("context"),
        "the stats give way: {narrow:?}"
    );

    let wider = footer(&mut app, 160);
    assert!(
        wider.contains("ASK · qwen3:4b · auto thinking · 62% context used"),
        "{wider:?}"
    );
    assert!(
        !wider.contains("1 call"),
        "a later stat gives way: {wider:?}"
    );
}

#[test]
fn test_thinking_level_gives_way_before_the_model() {
    use crate::events::UiCommand;

    let mut app = app_with_model(None);
    app.process_command(UiCommand::SetApproval(crate::ApprovalLevel::Strict));
    app.process_command(UiCommand::SetMode(crate::PermissionMode::Plan));
    let footer = render_rows(&mut app, 136)
        .into_iter()
        .find(|row| row.contains("qwen3:4b"))
        .expect("the model stays when the footer is too narrow for both");
    assert!(!footer.contains("thinking"), "{footer:?}");
}

#[test]
fn test_approval_level_sits_before_the_model() {
    use crate::events::UiCommand;

    let mut app = app_with_model(None);
    let input_row = |app: &mut crate::App| {
        render_rows(app, 136)
            .into_iter()
            .find(|row| row.contains("qwen3:4b"))
            .unwrap()
    };
    assert!(!input_row(&mut app).contains("ASK"), "unknown until told");

    app.process_command(UiCommand::SetApproval(crate::ApprovalLevel::Yolo));
    assert!(input_row(&mut app).contains("YOLO · qwen3:4b"));

    app.process_command(UiCommand::SetApproval(crate::ApprovalLevel::Strict));
    app.process_command(UiCommand::SetMode(crate::PermissionMode::Plan));
    assert!(input_row(&mut app).contains("plan mode (/plan to leave) · STRICT · qwen3:4b"));
}

#[test]
fn test_hosted_panels_leave_their_title_bars_to_the_front_end() {
    use crate::app::{ChatTab, Pane, PaneSplits, PaneTab};

    let mut app = crate::App::new();
    app.set_host_chrome(true);
    let rows = render_rows(&mut app, 150);
    let screen = rows.join("\n");
    assert!(
        !screen.contains("CHAT") && !screen.contains("KRN"),
        "{screen}"
    );
    let panes: Vec<Pane> = app.pane_chrome().iter().map(|c| c.pane).collect();
    assert_eq!(panes, [Pane::Agents, Pane::Chat, Pane::Records, Pane::Side]);

    let chat = app.pane_chrome()[1].clone();
    assert_eq!(chat.active, Some(PaneTab::Chat(ChatTab::Chat)));
    let row = |y: u16| -> String {
        rows[usize::from(y)]
            .chars()
            .skip(usize::from(chat.bar.x))
            .take(usize::from(chat.bar.width))
            .collect()
    };
    assert!(row(chat.bar.y).starts_with('┌'), "{screen}");
    assert_eq!(row(chat.bar.y + 1).trim_matches('│').trim(), "", "{screen}");
    let joint = row(chat.bar.y + 2);
    assert!(joint.starts_with('├') && joint.ends_with('┤'), "{screen}");

    app.set_pane_splits(PaneSplits {
        agents: Some(0.1),
        ..PaneSplits::default()
    });
    render_rows(&mut app, 150);
    let [agents, chat] = [0, 1].map(|i| app.pane_chrome()[i].area);
    assert_eq!((agents.x, agents.width), (1, 15), "inside the edge column");
    assert_eq!(chat.x, agents.right() + 1, "a column between panels");

    app.set_pane_splits(PaneSplits {
        agents: Some(0.9),
        ..PaneSplits::default()
    });
    render_rows(&mut app, 150);
    assert!(app.pane_chrome()[1].area.width >= 30, "the chat keeps room");
}

#[test]
fn test_showing_the_right_column_keeps_the_agents_column_in_place() {
    use crate::app::{Pane, PaneSplits};

    let agents_area = |app: &crate::App| {
        app.pane_chrome()
            .iter()
            .find(|chrome| chrome.pane == Pane::Agents)
            .map(|chrome| chrome.area)
    };
    for splits in [
        PaneSplits::default(),
        PaneSplits {
            agents: Some(0.4),
            ..PaneSplits::default()
        },
        PaneSplits {
            agents: Some(0.9),
            right: Some(0.5),
            ..PaneSplits::default()
        },
    ] {
        let mut app = crate::App::new();
        app.set_host_chrome(true);
        app.set_pane_splits(splits);
        render_rows(&mut app, 150);
        let shown = agents_area(&app);
        assert!(shown.is_some());
        assert_eq!(app.pane_chrome().len(), 4, "the right column is shown");

        app.panels.record_visible = false;
        app.panels.sandbox_visible = false;
        render_rows(&mut app, 150);
        assert_eq!(app.pane_chrome().len(), 2, "the right column is hidden");
        assert_eq!(agents_area(&app), shown, "{splits:?}");
        assert!(app.pane_chrome()[1].area.width >= 30, "the chat keeps room");
    }
}

#[test]
fn test_side_panels_end_level_with_the_input_box() {
    let mut app = crate::App::new();
    let rows = render_rows(&mut app, 120);
    let input_bottom = rows.len() - INPUT_FROM_BOTTOM + 1;
    let bottom = &rows[input_bottom];
    assert!(bottom.starts_with(" └"), "{bottom:?}");
    assert!(bottom.trim_end().ends_with('┘'), "{bottom:?}");
    assert_eq!(bottom.matches('└').count(), 3, "{bottom:?}");
    for footer in &rows[input_bottom + 1..] {
        assert!(!footer.contains('│'), "{footer:?}");
    }
}
