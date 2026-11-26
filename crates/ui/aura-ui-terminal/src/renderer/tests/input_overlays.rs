use super::*;

#[test]
fn test_slash_menu_spans_chat_width_above_input() {
    let mut app = app_with_model(None);
    type_text(&mut app, "/log");
    let rows = render_rows(&mut app, 120);
    let n = rows.len();

    let (_, chat_left, chat_right) = chat_panel(&rows);

    let menu_bottom = &rows[n - 4];
    let chars: Vec<char> = menu_bottom.chars().collect();
    assert_eq!(chars[chat_left], '└', "{menu_bottom:?}");
    assert_eq!(chars[chat_right], '┘', "{menu_bottom:?}");
    assert!(menu_bottom.contains("Esc close"), "{menu_bottom:?}");

    assert!(rows[n - 7].contains("› /log "), "{:?}", rows[n - 7]);
    assert!(rows[n - 6].contains("  /login"), "{:?}", rows[n - 6]);
    assert!(rows[n - 6].contains("Login for cloud models"));
    assert!(rows[n - 5].contains("  /logout"), "{:?}", rows[n - 5]);
    assert!(rows[n - 8].contains(" COMMANDS "), "{:?}", rows[n - 8]);
    let menu_top: Vec<char> = rows[n - 8].chars().collect();
    assert_eq!(menu_top[chat_left], '┌');
    assert_eq!(menu_top[chat_right], '┐');

    type_text(&mut app, " ");
    let rows = render_rows(&mut app, 120);
    assert!(!rows.join("\n").contains(" COMMANDS "));
}

#[test]
fn test_mention_picker_lists_files_above_the_input() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir(dir.path().join("src")).unwrap();
    std::fs::write(dir.path().join("src/main.rs"), "").unwrap();
    let mut app = app_with_model(None);
    app.process_command(crate::events::UiCommand::SetSandboxRoot(
        dir.path().to_path_buf(),
    ));
    type_text(&mut app, "look at @ma");
    let rows = render_rows(&mut app, 120);
    let n = rows.len();
    assert!(rows[n - 6].contains(" FILES "), "{:?}", rows[n - 6]);
    assert!(rows[n - 5].contains("› @src/main.rs"), "{:?}", rows[n - 5]);
    assert!(rows[n - 4].contains("Enter insert"), "{:?}", rows[n - 4]);

    type_text(&mut app, " ");
    let screen = render_rows(&mut app, 120).join("\n");
    assert!(!screen.contains(" FILES "));
}

#[test]
fn test_input_shows_image_chips_and_pasted_newlines() {
    let theme = test_theme();
    let mut app = app_with_model(None);
    app.handle_paste("first\nsecond");
    app.attach_image(b"\x89PNG\r\n\x1a\nimage").unwrap();
    let rows = render_rows(&mut app, 120);
    let input_row = &rows[rows.len() - INPUT_FROM_BOTTOM];
    let first_row = &rows[rows.len() - INPUT_FROM_BOTTOM - 1];
    assert!(input_row.contains("│   second [image 1]"), "{input_row:?}");
    assert!(first_row.contains("│ → first "), "{first_row:?}");
    assert!(!first_row.contains('↵') && !input_row.contains('↵'));

    let spans = super::super::input::prose_spans("see @src/a.rs and [image 2] me@x", &theme);
    let picked: Vec<&str> = spans
        .iter()
        .filter(|s| s.style.add_modifier.contains(Modifier::UNDERLINED))
        .map(|s| s.content.as_ref())
        .collect();
    assert_eq!(picked, ["@src/a.rs", "[image 2]"]);

    submit(&mut app, "");
    let screen = render_rows(&mut app, 120).join("\n");
    assert!(screen.contains("[image 1]"), "the chat shows the chip");
}

#[test]
fn test_help_lists_the_input_keys_without_clipping() {
    let mut app = crate::App::new();
    submit(&mut app, "/help");
    let mut terminal = ratatui::Terminal::new(ratatui::backend::TestBackend::new(100, 50)).unwrap();
    terminal
        .draw(|frame| super::super::render(frame, &mut app, &test_theme()))
        .unwrap();
    let buffer = terminal.backend().buffer();
    let screen: String = (0..buffer.area.height)
        .map(|y| {
            (0..buffer.area.width)
                .map(|x| buffer[(x, y)].symbol())
                .collect::<String>()
                + "\n"
        })
        .collect();
    for line in [
        "@path      Attach a file or folder",
        "Ctrl+V     Paste an image (or text)",
        "Ctrl+G     Edit the input in $EDITOR",
        "Press any key to close",
    ] {
        assert!(screen.contains(line), "{line:?} missing:\n{screen}");
    }
}

#[test]
fn test_agent_space_shows_permanent_agents_to_add() {
    use crate::events::{AgentDefinitionSummary, UiCommand};

    let mut app = app_with_model(None);
    app.process_command(UiCommand::SetAgentDefinitions(vec![
        AgentDefinitionSummary {
            name: "explorer".to_string(),
            description: "Reads code\nand more".to_string(),
            ..Default::default()
        },
        AgentDefinitionSummary {
            name: "reviewer".to_string(),
            description: "Reviews diffs".to_string(),
            ..Default::default()
        },
    ]));
    type_text(&mut app, "/agent ");
    let rows = render_rows(&mut app, 120);
    let n = rows.len();

    assert!(
        rows[n - 7].contains(" ADD AGENT TO PROJECT "),
        "{:?}",
        rows[n - 7]
    );
    assert!(
        rows[n - 6].contains("› explorer   Reads code"),
        "{:?}",
        rows[n - 6]
    );
    assert!(!rows[n - 6].contains("and more"));
    assert!(
        rows[n - 5].contains("  reviewer   Reviews diffs"),
        "{:?}",
        rows[n - 5]
    );
    assert!(rows[n - 4].contains("Enter add"), "{:?}", rows[n - 4]);
}

#[test]
fn test_header_shows_local_and_model_moves_to_the_footer() {
    let rows = render_with_user(None, 120);
    assert!(rows[0].trim_end().ends_with("● local  ☰"));
    assert!(!rows[0].contains("qwen3:4b"));

    let n = rows.len();
    let (_, chat_left, chat_right) = chat_panel(&rows);
    let divider: Vec<char> = rows[n - 4].chars().collect();
    let box_bottom: Vec<char> = rows[n - 2].chars().collect();
    assert_eq!((divider[chat_left], divider[chat_right]), ('├', '┤'));
    assert_eq!((box_bottom[chat_left], box_bottom[chat_right]), ('└', '┘'));
    let above: Vec<char> = rows[n - 5].chars().collect();
    assert_eq!(
        (above[chat_left], above[chat_right]),
        ('│', '│'),
        "the chat panel runs down to the line above the input: {:?}",
        rows[n - 5]
    );

    let input_row = &rows[n - INPUT_FROM_BOTTOM];
    assert!(input_row.contains("│ →  Ask anything"), "{input_row:?}");
    assert!(!input_row.contains("]$ "), "{input_row:?}");
    assert!(!input_row.contains("qwen3:4b"), "{input_row:?}");

    let footer: String = rows[n - 1]
        .chars()
        .skip(chat_left)
        .take(chat_right - chat_left)
        .collect();
    assert!(footer.trim_start().starts_with("qwen3:4b"), "{footer:?}");
    assert!(footer.contains("● Ready"), "{footer:?}");
    assert!(
        footer.trim_end().ends_with("⌂ local"),
        "the placement switch ends the footer: {footer:?}"
    );
    assert!(!footer.contains("/model"));
}
