use super::*;

#[test]
fn test_scrolled_chat_stays_put_while_reply_streams() {
    use crate::components::{Message, MessageRole};
    use crate::events::UiCommand;
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

    let mut app = crate::App::new();
    for i in 0..40 {
        app.add_message(Message::new(
            MessageRole::Assistant,
            &format!("note {i:02}"),
        ));
    }
    render_rows(&mut app, 100);
    app.scroll_up(12);
    let notes = |rows: Vec<String>| -> Vec<(usize, String)> {
        rows.into_iter()
            .enumerate()
            .filter_map(|(y, row)| row.find("note ").map(|i| (y, row[i..i + 7].to_string())))
            .collect()
    };
    let before = render_rows(&mut app, 100);
    assert!(before.join("\n").contains("lines below"));

    app.process_command(UiCommand::StartStreaming);
    app.process_command(UiCommand::AppendText("first\nsecond\nthird".to_string()));
    let after = render_rows(&mut app, 100);
    let screen = after.join("\n");
    assert!(!screen.contains("third"), "{screen}");
    assert!(screen.contains("lines below"), "{screen}");
    assert_eq!(notes(after), notes(before));

    app.handle_key(KeyEvent::new(KeyCode::End, KeyModifiers::CONTROL));
    let screen = render_rows(&mut app, 100).join("\n");
    assert!(screen.contains("third"), "{screen}");
    assert!(!screen.contains("lines below"), "{screen}");
}

#[test]
fn test_cached_chat_lines_follow_message_edits() {
    use crate::components::{Message, MessageRole};
    use crate::events::UiCommand;

    let mut app = crate::App::new();
    app.process_command(UiCommand::StartStreaming);
    app.process_command(UiCommand::AppendText("draft".to_string()));
    render_rows(&mut app, 100);
    app.process_command(UiCommand::AppendText(" reply".to_string()));
    app.add_message(Message::new(MessageRole::User, "next"));
    app.scroll_chat_to_top();
    let screen = render_rows(&mut app, 100).join("\n");
    assert!(screen.contains("draft reply"), "{screen}");
    assert!(screen.contains("next"), "{screen}");

    let narrow = render_rows(&mut app, 60).join("\n");
    assert!(narrow.contains("draft reply"), "{narrow}");
}

#[test]
fn test_chat_bottom_leaves_room_below_latest_prompt() {
    use crate::components::{Message, MessageRole};

    let mut app = crate::App::new();
    for i in 0..40 {
        app.add_message(Message::new(
            MessageRole::Assistant,
            &format!("note {i:02}"),
        ));
    }
    app.add_message(Message::new(MessageRole::User, "latest prompt"));
    app.add_message(Message::new(MessageRole::Assistant, "short reply"));
    let rows = render_rows(&mut app, 100);
    let screen = rows.join("\n");
    let row = |needle: &str| {
        rows.iter()
            .position(|row| row.contains(needle))
            .unwrap_or_else(|| panic!("{needle:?} not in:\n{screen}"))
    };
    // The prompt sits under its frame's border and padding rows.
    assert!(row("latest prompt") <= row(" CHAT ") + 6, "{screen}");
    assert!(screen.contains("short reply"), "{screen}");
    assert!(!screen.contains("note 39"), "{screen}");
    assert!(!screen.contains("lines below"), "{screen}");

    app.scroll_up(3);
    let screen = render_rows(&mut app, 100).join("\n");
    assert!(!screen.contains("lines below"), "{screen}");
}

/// The row of `needle` in `rows`, and how many blank rows lie between it
/// and the chat's bottom border.
fn rows_under(rows: &[String], needle: &str) -> (usize, usize) {
    let screen = rows.join("\n");
    let row = rows
        .iter()
        .position(|row| row.contains(needle))
        .unwrap_or_else(|| panic!("{needle:?} not in:\n{screen}"));
    let column = rows[row][..rows[row].find(needle).unwrap()].chars().count();
    let border = (row + 1..rows.len())
        .find(|&y| rows[y].chars().nth(column) == Some('─'))
        .unwrap_or_else(|| panic!("no border below {needle:?}:\n{screen}"));
    let blank = |row: &String| {
        row.chars()
            .skip(column)
            .take(needle.chars().count())
            .all(|c| c == ' ')
    };
    assert!(
        rows[row + 1..border].iter().all(blank),
        "only blank rows below {needle:?}:\n{screen}"
    );
    (row, border - row - 1)
}

#[test]
fn test_chat_keeps_room_below_a_reply_that_reaches_the_bottom() {
    use crate::components::{Message, MessageRole};
    use crate::events::UiCommand;

    let mut app = crate::App::new();
    app.add_message(Message::new(MessageRole::User, "fix the login bug"));
    app.process_command(UiCommand::StartStreaming);
    app.process_command(UiCommand::AppendText("line 00".to_string()));
    let rows = render_rows(&mut app, 100);
    let height = app.areas.chat_text.expect("the chat is drawn").height;
    let room = usize::from(height) / 4;
    assert!(room > 1, "a quarter of {height} rows");
    let (_, under) = rows_under(&rows, "line 00");
    assert!(under > room, "a short reply leaves the rest blank");

    let mut scrolled = false;
    for i in 1..40 {
        app.process_command(UiCommand::AppendText(format!("\n\nline {i:02}")));
        let rows = render_rows(&mut app, 100);
        let screen = rows.join("\n");
        let (_, under) = rows_under(&rows, &format!("line {i:02}"));
        assert!(
            under >= room,
            "line {i:02} too close to the bottom:\n{screen}"
        );
        assert!(!screen.contains("lines below"), "{screen}");
        if !screen.contains("fix the login bug") {
            // The reply pushed the prompt away: it scrolls with the room.
            assert_eq!(under, room, "{screen}");
            scrolled = true;
        }
    }
    assert!(scrolled, "the reply outgrew the chat");

    // The rows under the last line are blank, not lines left to read.
    app.scroll_up(room);
    let rows = render_rows(&mut app, 100);
    let screen = rows.join("\n");
    assert_eq!(rows_under(&rows, "line 39").1, 0, "{screen}");
    assert!(!screen.contains("lines below"), "{screen}");
    app.scroll_up(1);
    let screen = render_rows(&mut app, 100).join("\n");
    assert!(screen.contains(" ↓ 1 line below "), "{screen}");
}

#[test]
fn test_terminal_output_runs_to_the_bottom_row() {
    use crate::events::UiCommand;

    let mut app = app_with_model(None);
    submit_message(&mut app, "$seq");
    let output: String = (0..60).map(|n| format!("line {n:02}\n")).collect();
    app.process_command(UiCommand::CommandFinished {
        output,
        exit_code: None,
    });
    app.process_command(UiCommand::Complete);
    assert_eq!(app.chat_tab(), crate::app::ChatTab::Terminal);
    let rows = render_rows(&mut app, 120);
    assert_eq!(rows_under(&rows, "line 59").1, 0, "{}", rows.join("\n"));
}

#[test]
fn test_chat_runs_from_its_top_padding_to_the_bottom_border() {
    use crate::components::{Message, MessageRole};

    let mut app = crate::App::new();
    app.add_message(Message::new(MessageRole::User, "prompt"));
    let reply: Vec<String> = (0..60).map(|i| format!("line {i:02}")).collect();
    app.add_message(Message::new(MessageRole::Assistant, &reply.join("\n\n")));
    render_rows(&mut app, 100);
    // Up past the blank rows kept below the last line.
    app.scroll_up(chat_room(&app));
    let rows = render_rows(&mut app, 100);
    let screen = rows.join("\n");
    let last = rows
        .iter()
        .position(|row| row.contains("line 59"))
        .unwrap_or_else(|| panic!("last line not in:\n{screen}"));
    let column = rows[last][..rows[last].find("line 59").unwrap()]
        .chars()
        .count();
    let border = |row: &String| row.chars().nth(column) == Some('─');
    assert!(
        border(&rows[last + 1]),
        "last line on the bottom row:\n{screen}"
    );
    assert!(!screen.contains("lines below"), "{screen}");

    app.scroll_chat_to_top();
    let rows = render_rows(&mut app, 100);
    let first = rows
        .iter()
        .position(|row| row.contains("prompt"))
        .expect("the prompt at the top");
    // Above the prompt: its frame's padding row, its top border, the
    // panel's blank row, then the panel's top border.
    assert!(rows[first - 2].contains('╭'), "prompt framed:\n{screen}");
    assert!(
        !border(&rows[first - 3]) && border(&rows[first - 4]),
        "first line a row under the top border:\n{screen}"
    );
}

#[test]
fn test_command_input_is_highlighted() {
    let theme = test_theme();
    let mut app = app_with_model(None);
    type_text(&mut app, "$echo -n hi");
    let buffer = render_buffer(&mut app, 120);
    let input_y = row_of(&buffer, "]$ echo -n hi");

    let (_, program) = find_cell(&buffer, input_y, "echo").unwrap();
    assert_eq!(program.fg, Some(theme.colors.success));
    assert!(program.add_modifier.contains(Modifier::BOLD));
    let (_, flag) = find_cell(&buffer, input_y, "-n").unwrap();
    assert_eq!(flag.fg, Some(theme.colors.warning));
}

#[test]
fn test_prose_input_is_in_the_reply_color() {
    let theme = test_theme();
    let mut app = app_with_model(None);
    type_text(&mut app, "please explain");
    let buffer = render_buffer(&mut app, 120);

    let (_, style) = find_cell(&buffer, buffer.area.height - 3, "please").unwrap();
    assert_eq!(style.fg, Some(theme.colors.text));
}

#[test]
fn test_command_output_renders_like_a_terminal() {
    use crate::events::UiCommand;

    let theme = test_theme();
    let mut app = app_with_model(None);
    app.set_shell_prompt("[u@h d]$ ");
    submit_message(&mut app, "$ls -la");
    app.process_command(UiCommand::CommandFinished {
        output: "total 0\ndrwxr-xr-x  2 u u  40 \x1b[01;34msrc\x1b[0m\n".to_string(),
        exit_code: None,
    });
    app.process_command(UiCommand::Complete);

    let buffer = render_buffer(&mut app, 120);
    let rows = render_rows(&mut app, 120);
    let screen = rows.join("\n");
    let echo = rows
        .iter()
        .position(|r| r.contains("[u@h d]$ ls -la"))
        .unwrap();
    assert!(!screen.contains("YOU  "), "{screen}");
    let total = &rows[echo + 1];
    let listing = &rows[echo + 2];
    assert!(total.contains("total 0"), "{total:?}");
    assert!(listing.contains("drwxr-xr-x  2 u u  40 src"), "{listing:?}");
    assert!(
        !rows[echo + 3].contains("[u@h d]$ "),
        "{:?}",
        rows[echo + 3]
    );
    let input_row = &rows[rows.len() - INPUT_FROM_BOTTOM];
    assert!(input_row.contains("[u@h d]$ "), "{input_row:?}");
    assert!(!screen.contains('\x1b'));
    let column = |row: &str, needle: &str| row.find(needle).map(|i| row[..i].chars().count());
    assert_eq!(
        column(total, "total"),
        column(&rows[echo], "[u@h d]"),
        "output starts at the prompt's column"
    );

    let y = u16::try_from(echo + 2).unwrap();
    let (_, dir) = find_cell(&buffer, y, "src").unwrap();
    assert_eq!(dir.fg, Some(Color::Indexed(4)));
    assert!(dir.add_modifier.contains(Modifier::BOLD));
    let (_, plain) = find_cell(&buffer, y, "drwx").unwrap();
    assert_eq!(plain.fg, Some(theme.colors.foreground));
}

#[test]
fn test_failed_command_shows_exit_code() {
    use crate::events::UiCommand;

    let mut app = app_with_model(None);
    submit_message(&mut app, "$false");
    app.process_command(UiCommand::CommandFinished {
        output: String::new(),
        exit_code: Some(1),
    });

    let rows = render_rows(&mut app, 120);
    let echo = rows.iter().position(|r| r.contains("]$ false")).unwrap();
    assert!(rows[echo + 1].contains("exit 1"), "{:?}", rows[echo + 1]);
}

#[test]
fn test_chat_opens_at_last_message_on_startup() {
    use crate::components::MessageRole;
    use crate::events::{MessageData, UiCommand};

    let history = |tag: &str| {
        (0..40)
            .map(|i| MessageData::new(MessageRole::User, format!("{tag} {i}")))
            .collect()
    };
    let shows = |rows: &[String], text: &str| rows.iter().any(|r| r.contains(&format!("{text} ")));

    let (tx, rx) = tokio::sync::mpsc::channel(200);
    let mut app = crate::App::new().with_command_receiver(rx);
    tx.try_send(UiCommand::LoadConversation(history("first")))
        .unwrap();
    app.tick();
    let rows = render_rows(&mut app, 120);
    assert!(shows(&rows, "first 39"), "{}", rows.join("\n"));
    assert!(!shows(&rows, "first 0"), "{}", rows.join("\n"));

    // Opening another chat after scrolling this one up starts at its end too.
    app.scroll_up(30);
    render_rows(&mut app, 120);
    tx.try_send(UiCommand::LoadConversation(history("second")))
        .unwrap();
    app.tick();
    let rows = render_rows(&mut app, 120);
    assert!(shows(&rows, "second 39"), "{}", rows.join("\n"));
}
