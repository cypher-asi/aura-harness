use super::*;

#[test]
fn test_long_command_wraps_at_the_last_column() {
    let mut app = app_with_model(None);
    app.set_shell_prompt("$ ");
    submit_message(&mut app, "/swarm");
    let command = format!("echo {}", "x".repeat(200));
    submit_message(&mut app, &format!("${command}"));
    let rows = render_rows(&mut app, 120);

    let start = rows.iter().position(|r| r.contains("│ $ echo x")).unwrap();
    let chars: Vec<char> = rows[start].chars().collect();
    let left = rows[start]
        .find("│ $ echo")
        .map(|i| rows[start][..i].chars().count());
    let left = left.unwrap();
    let right = left + 1 + chars[left + 1..].iter().position(|c| *c == '│').unwrap();
    let (text_x, text_width) = (left + 2, right - left - 3);
    let shown: String = rows[start..]
        .iter()
        .take_while(|r| r.contains('x'))
        .map(|r| r.chars().skip(text_x).take(text_width).collect::<String>())
        .collect();
    assert_eq!(shown.trim_end(), format!("$ {command}"), "wrapped mid-word");
}

#[test]
fn test_pane_shows_only_the_running_command_and_input_keeps_the_prompt() {
    let mut app = app_with_model(None);
    app.set_shell_prompt("[u@h d]$ ");
    submit_message(&mut app, "$sleep 5");
    let rows = render_rows(&mut app, 120);
    let (_, chat_left, _) = chat_panel(&rows);
    let input_y = rows.len() - INPUT_FROM_BOTTOM;
    let pane = rows[..input_y].join("\n");
    assert_eq!(pane.matches("[u@h d]$ ").count(), 1, "{pane}");
    assert!(pane.contains("[u@h d]$ sleep 5"), "{pane}");
    assert!(!pane.contains("running sleep"), "{pane}");
    let typed: String = rows[input_y].chars().skip(chat_left + 2).collect();
    assert!(typed.starts_with("[u@h d]$ "), "{:?}", rows[input_y]);
}

#[test]
fn test_user_prompt_renders_markdown_under_its_prompt() {
    use crate::components::{Message, MessageRole};

    let mut app = app_with_model(None);
    app.add_message(Message::new(
        MessageRole::User,
        "Read-only. Use `git show` here.\n\n- **first** item\n- second item",
    ));
    let screen = render_rows(&mut app, 100).join("\n");
    assert!(
        screen.contains("→ Read-only. Use git show here."),
        "{screen}"
    );
    assert!(screen.contains("  • first item"), "{screen}");
    assert!(screen.contains("  • second item"), "{screen}");
    assert!(!screen.contains("**"), "{screen}");
    assert!(!screen.contains('`'), "{screen}");
}

#[test]
fn test_terminal_pane_shows_only_commands_and_chat_hides_them() {
    use crate::components::{Message, MessageRole};
    use crate::events::UiCommand;
    use crossterm::event::{KeyCode, KeyEvent};

    let mut app = app_with_model(None);
    app.add_message(Message::new(MessageRole::User, "hello agent"));
    submit_message(&mut app, "$echo hi");
    app.process_command(UiCommand::CommandFinished {
        output: "hi\n".to_string(),
        exit_code: None,
    });
    app.process_command(UiCommand::Complete);

    let screen = render_rows(&mut app, 120).join("\n");
    assert_eq!(app.chat_tab(), crate::app::ChatTab::Terminal);
    assert!(screen.contains("echo hi"), "{screen}");
    assert!(!screen.contains("hello agent"), "{screen}");

    app.handle_key(KeyEvent::from(KeyCode::Esc));
    let screen = render_rows(&mut app, 120).join("\n");
    assert_eq!(app.chat_tab(), crate::app::ChatTab::Chat);
    assert!(screen.contains("hello agent"), "{screen}");
    assert!(!screen.contains("echo hi"), "{screen}");

    type_text(&mut app, "$");
    let screen = render_rows(&mut app, 120).join("\n");
    assert!(screen.contains("echo hi"), "history kept: {screen}");
}

#[test]
fn test_empty_terminal_pane_says_how_to_leave() {
    let mut app = app_with_model(None);
    type_text(&mut app, "$");
    let screen = render_rows(&mut app, 120).join("\n");
    assert!(screen.contains("Esc returns to chat"), "{screen}");
}

#[test]
fn test_outer_text_is_inset_to_panel_borders() {
    let width = 100;
    let rows = render_with_user(None, width);
    let last_col = |row: &str| row.trim_end().chars().count() - 1;
    // The outer borders run through columns 1 and width - 2; the header
    // keeps a column inside them, as the footer does inside the chat's.
    let inner_right = usize::from(width) - 3;

    assert!(rows[0].starts_with("  AURA"), "{:?}", rows[0]);
    assert_eq!(last_col(&rows[0]), inner_right, "{:?}", rows[0]);
    let (_, _, chat_right) = chat_panel(&rows);
    let status: Vec<char> = rows[rows.len() - 1].chars().collect();
    // The placement switch (`⌂ local`) ends the footer.
    assert_eq!(status[chat_right - 1], 'l', "{status:?}");
    assert_eq!(status[chat_right], ' ', "{status:?}");
}
