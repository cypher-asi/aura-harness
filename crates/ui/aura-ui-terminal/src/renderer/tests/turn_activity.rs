use super::*;

#[test]
fn test_turn_activity_renders_inline_between_prompt_and_reply() {
    use crate::events::{ToolData, UiCommand};

    let mut app = crate::App::new();
    submit_message(&mut app, "fix it");
    app.process_command(UiCommand::StartThinking);
    app.process_command(UiCommand::AppendThinking("Look at main first.".to_string()));
    let live = render_rows(&mut app, 120).join("\n");
    assert!(live.contains("Thinking…"), "{live}");
    assert!(live.contains("│ Look at main first."), "{live}");
    assert!(!live.contains("Thinker"), "{live}");

    app.process_command(UiCommand::FinishThinking);
    for (id, path) in [("t1", "src/main.rs"), ("t2", "src/lib.rs")] {
        app.process_command(UiCommand::ShowTool(ToolData {
            id: id.to_string(),
            name: "read_file".to_string(),
            args: format!(r#"{{"path":"{path}"}}"#),
        }));
        app.process_command(UiCommand::CompleteTool {
            id: id.to_string(),
            result: "fn main() {}".to_string(),
            success: true,
        });
    }
    app.process_command(UiCommand::StartStreaming);
    app.process_command(UiCommand::AppendText("Fixed.".to_string()));
    app.process_command(UiCommand::Complete);

    let rows = render_rows(&mut app, 120);
    let find = |needle: &str| {
        rows.iter()
            .position(|row| row.contains(needle))
            .unwrap_or_else(|| panic!("{needle:?} not in:\n{}", rows.join("\n")))
    };
    let summary = find("▸ Explored 2 files");
    let reply = find("Fixed.");
    assert!(find("fix it") < summary);
    assert!(summary < reply);
    let text = rows.join("\n");
    assert!(!text.contains("Read main.rs"), "{text}");
    assert!(!text.contains("Look at main first."), "{text}");

    app.handle_key(crossterm::event::KeyEvent::new(
        crossterm::event::KeyCode::Char('o'),
        crossterm::event::KeyModifiers::CONTROL,
    ));
    app.scroll_chat_to_top();
    let rows = render_rows(&mut app, 120);
    let text = rows.join("\n");
    assert!(text.contains("▾ Explored 2 files"), "{text}");
    assert!(text.contains("Thought briefly"), "{text}");
    assert!(text.contains("│ Look at main first."), "{text}");
    assert!(text.contains("Read main.rs"), "{text}");
    assert!(text.contains("Read lib.rs"), "{text}");
}

#[test]
fn test_step_group_runs_open_then_collapses_and_clicks_open() {
    use crate::events::{ToolData, UiCommand};
    use crossterm::event::{KeyModifiers, MouseButton, MouseEvent, MouseEventKind};

    let mut app = crate::App::new();
    submit_message(&mut app, "look around");
    for (id, path) in [("t1", "src/main.rs"), ("t2", "src/lib.rs")] {
        app.process_command(UiCommand::ShowTool(ToolData {
            id: id.to_string(),
            name: "read_file".to_string(),
            args: format!(r#"{{"path":"{path}"}}"#),
        }));
        app.process_command(UiCommand::CompleteTool {
            id: id.to_string(),
            result: "fn main() {}".to_string(),
            success: true,
        });
    }
    let live = render_rows(&mut app, 120).join("\n");
    assert!(live.contains("Exploring 2 files"), "{live}");
    assert!(live.contains("Read main.rs"), "{live}");
    assert!(live.contains("Read lib.rs"), "{live}");
    let rows: Vec<&str> = live.lines().collect();
    let row = |needle: &str| rows.iter().position(|r| r.contains(needle));
    assert!(
        row("⬢ Thinking…") > row("Read lib.rs"),
        "the working line stays below the live steps: {live}"
    );

    app.process_command(UiCommand::Complete);
    let done = render_rows(&mut app, 120).join("\n");
    assert!(done.contains("▸ Explored 2 files"), "{done}");
    assert!(!done.contains("Read main.rs"), "{done}");

    let click = |app: &mut crate::App, needle: &str| {
        let buffer = render_buffer(app, 120);
        let row = row_of(&buffer, needle);
        let (column, _) = find_cell(&buffer, row, needle).unwrap();
        for kind in [
            MouseEventKind::Down(MouseButton::Left),
            MouseEventKind::Up(MouseButton::Left),
        ] {
            app.handle_mouse(MouseEvent {
                kind,
                column,
                row,
                modifiers: KeyModifiers::empty(),
            });
        }
    };
    click(&mut app, "Explored");
    let open = render_rows(&mut app, 120).join("\n");
    assert!(open.contains("▾ Explored 2 files"), "{open}");
    assert!(open.contains("Read lib.rs"), "{open}");
    assert!(!open.contains("│ fn main() {}"), "{open}");

    click(&mut app, "lib.rs");
    let view = render_rows(&mut app, 120).join("\n");
    assert!(
        matches!(app.detail_view(), Some(crate::app::DetailView::File(_))),
        "a read opens in the file view: {view}"
    );
    assert!(view.contains("src/lib.rs"), "{view}");
    assert!(view.contains("1 │ fn main() {}"), "{view}");
}

/// Click the first chat text on screen that reads `needle`.
fn click_text(app: &mut crate::App, needle: &str) {
    use crossterm::event::{KeyModifiers, MouseButton, MouseEvent, MouseEventKind};

    let buffer = render_buffer(app, 120);
    let row = row_of(&buffer, needle);
    let (column, _) = find_cell(&buffer, row, needle).unwrap();
    for kind in [
        MouseEventKind::Down(MouseButton::Left),
        MouseEventKind::Up(MouseButton::Left),
    ] {
        app.handle_mouse(MouseEvent {
            kind,
            column,
            row,
            modifiers: KeyModifiers::empty(),
        });
    }
}

fn read_tool(app: &mut crate::App, id: &str, path: &str) {
    use crate::events::{ToolData, UiCommand};

    app.process_command(UiCommand::ShowTool(ToolData {
        id: id.to_string(),
        name: "read_file".to_string(),
        args: format!(r#"{{"path":"{path}"}}"#),
    }));
    app.process_command(UiCommand::CompleteTool {
        id: id.to_string(),
        result: "fn main() {}".to_string(),
        success: true,
    });
}

#[test]
fn test_turn_output_starts_right_under_the_prompt_while_steps_arrive() {
    use crate::events::UiCommand;

    let mut app = crate::App::new();
    submit_message(&mut app, "fix it");
    let aura_row = |app: &mut crate::App| {
        let rows = render_rows(app, 120);
        let prompt = rows
            .iter()
            .position(|row| row.contains("→ fix it"))
            .unwrap();
        let below = &rows[prompt + 2];
        let text = below.trim_matches(|c: char| c.is_whitespace() || c == '│');
        assert!(!text.is_empty(), "{}", rows.join("\n"));
        assert!(!rows.join("\n").contains("AURA  "));
        prompt
    };
    let waiting = aura_row(&mut app);

    app.process_command(UiCommand::StartThinking);
    app.process_command(UiCommand::AppendThinking("Look at main.".to_string()));
    assert_eq!(aura_row(&mut app), waiting);
    app.process_command(UiCommand::FinishThinking);
    assert_eq!(aura_row(&mut app), waiting);
    let between = render_rows(&mut app, 120).join("\n");
    assert!(between.contains("⬢ Thinking…"), "{between}");

    read_tool(&mut app, "t1", "src/main.rs");
    assert_eq!(aura_row(&mut app), waiting);
    app.process_command(UiCommand::StartStreaming);
    assert_eq!(aura_row(&mut app), waiting);
    app.process_command(UiCommand::AppendText("Fixed.".to_string()));
    app.process_command(UiCommand::Complete);
    assert_eq!(aura_row(&mut app), waiting);
    let done = render_rows(&mut app, 120).join("\n");
    assert!(done.contains("Fixed."), "{done}");
}

#[test]
fn test_live_steps_are_numbered_in_order_then_condense() {
    use crate::events::UiCommand;

    let mut app = crate::App::new();
    submit_message(&mut app, "look around");
    for i in 1..=6 {
        read_tool(&mut app, &format!("t{i}"), &format!("src/f{i}.rs"));
    }
    let live = render_rows(&mut app, 120).join("\n");
    assert!(live.contains("Exploring 6 files · step 6"), "{live}");
    for i in 1..=6 {
        assert!(live.contains(&format!("{i} ▸ Read f{i}.rs")), "{live}");
    }

    app.process_command(UiCommand::Complete);
    let done = render_rows(&mut app, 120).join("\n");
    assert!(done.contains("▸ Explored 6 files"), "{done}");
    assert!(done.contains("6 steps (click to expand)"), "{done}");
    assert!(!done.contains("Read f1.rs"), "{done}");

    click_text(&mut app, "6 steps (click to expand)");
    let open = render_rows(&mut app, 120).join("\n");
    assert!(open.contains("▾ Explored 6 files"), "{open}");
    assert!(open.contains("1 ▸ Read f1.rs"), "{open}");
    assert!(!open.contains("click to expand"), "{open}");
}

fn record_at(seq: u64, ts_ms: u64) -> crate::events::RecordSummary {
    crate::events::RecordSummary {
        seq,
        timestamp: "12:00:00".to_string(),
        full_hash: String::new(),
        hash_suffix: String::new(),
        tx_kind: "UserPrompt".to_string(),
        sender: String::new(),
        message: String::new(),
        action_count: 0,
        effect_status: String::new(),
        status: crate::events::RecordStatus::Ok,
        info: String::new(),
        what: String::new(),
        group: String::new(),
        detail: String::new(),
        error_details: String::new(),
        tx_id: String::new(),
        agent_id: String::new(),
        ts_ms,
        policy: None,
    }
}

#[test]
fn test_open_thinking_shows_its_reasoning_and_jumps_to_turn_records() {
    use crate::app::PanelFocus;
    use crate::events::UiCommand;

    let mut app = crate::App::new();
    app.process_command(UiCommand::NewRecord(record_at(1, 1_000)));
    submit_message(&mut app, "fix it");
    let now = u64::try_from(chrono::Utc::now().timestamp_millis()).unwrap();
    app.process_command(UiCommand::NewRecord(record_at(2, now)));
    app.process_command(UiCommand::NewRecord(record_at(3, now + 5)));
    app.process_command(UiCommand::StartThinking);
    app.process_command(UiCommand::FinishThinking);
    read_tool(&mut app, "t1", "src/main.rs");
    app.process_command(UiCommand::Complete);

    click_text(&mut app, "click to expand");
    click_text(&mut app, "Thought");
    let open = render_rows(&mut app, 120).join("\n");
    assert!(
        open.contains("the model shared no reasoning text"),
        "{open}"
    );
    assert!(open.contains("→ records for this turn"), "{open}");

    click_text(&mut app, "records for this turn");
    assert_eq!(app.focus(), PanelFocus::Records);
    assert_eq!(app.selected_record(), 1);
}

#[test]
fn test_clicking_the_working_indicator_jumps_to_turn_records() {
    use crate::app::PanelFocus;
    use crate::events::UiCommand;

    let mut app = crate::App::new();
    app.process_command(UiCommand::NewRecord(record_at(1, 1_000)));
    submit_message(&mut app, "hi");
    let now = u64::try_from(chrono::Utc::now().timestamp_millis()).unwrap();
    app.process_command(UiCommand::NewRecord(record_at(2, now)));

    click_text(&mut app, "Thinking…");
    assert_eq!(app.focus(), PanelFocus::Records);
    assert_eq!(app.selected_record(), 1);
}

#[test]
fn test_policy_decisions_fold_into_their_tool_calls() {
    use crate::events::{ToolData, UiCommand};

    let mut app = crate::App::new();
    submit_message(&mut app, "kill it then respawn");
    for id in ["t1", "t2", "t3"] {
        app.process_command(UiCommand::ShowTool(ToolData {
            id: id.to_string(),
            name: "run_command".to_string(),
            args: r#"{"command":"node -e 1"}"#.to_string(),
        }));
        app.process_command(UiCommand::ToolPolicy {
            id: id.to_string(),
            tool: "run_command".to_string(),
            decision: "allowed by user_default.full_access".to_string(),
        });
        app.process_command(UiCommand::CompleteTool {
            id: id.to_string(),
            result: "ok".to_string(),
            success: id != "t1",
        });
    }
    app.process_command(UiCommand::Complete);

    let done = render_rows(&mut app, 120).join("\n");
    assert!(done.contains("▸ Ran 3 commands · 1 failed"), "{done}");
    assert!(!done.contains("policy:"), "{done}");

    app.handle_key(crossterm::event::KeyEvent::new(
        crossterm::event::KeyCode::Char('o'),
        crossterm::event::KeyModifiers::CONTROL,
    ));
    let open = render_rows(&mut app, 120).join("\n");
    assert!(open.contains("▾ Ran node -e 1"), "{open}");
    assert!(open.contains("│ policy: allowed by"), "{open}");
    assert!(!open.contains("policy: run_command"), "{open}");

    app.process_command(UiCommand::ToolPolicy {
        id: "gone".to_string(),
        tool: "write_file".to_string(),
        decision: "denied by user.off".to_string(),
    });
    let rows = render_rows(&mut app, 120).join("\n");
    assert!(
        rows.contains("policy: write_file denied by user.off"),
        "{rows}"
    );
}

#[test]
fn test_policy_lines_follow_a_star_and_stack() {
    use crate::events::{MessageData, MessageRole, UiCommand};

    let mut app = crate::App::new();
    submit_message(&mut app, "write them");
    for tool in ["write_file", "edit_file"] {
        app.process_command(UiCommand::ShowMessage(MessageData::new(
            MessageRole::System,
            format!("policy: {tool} allowed by user.on"),
        )));
    }
    let rows = render_rows(&mut app, 120);
    let row = |needle: &str| {
        rows.iter()
            .position(|row| row.contains(needle))
            .unwrap_or_else(|| panic!("{needle:?} not in:\n{}", rows.join("\n")))
    };
    let first = row("policy: write_file allowed by user.on");
    let star = rows[first].find("* policy: write_file").unwrap();
    let before = rows[first][..star].trim_end_matches(' ');
    assert!(
        before.ends_with('│') && before.len() + 2 <= star,
        "{}",
        rows[first]
    );
    assert_eq!(row("policy: edit_file allowed by user.on"), first + 1);
}
