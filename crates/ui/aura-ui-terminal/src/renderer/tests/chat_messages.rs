use super::*;

#[test]
fn test_edited_file_block_shows_its_diff() {
    use crate::events::{ToolData, UiCommand};

    let mut app = crate::App::new();
    app.process_command(UiCommand::ShowTool(ToolData {
        id: "t1".to_string(),
        name: "write_file".to_string(),
        args: r#"{"path":"src/lib.rs","content":"fn a() {}\nfn c() {}\n"}"#.to_string(),
    }));
    app.process_command(UiCommand::ToolDiff {
        id: "t1".to_string(),
        diff: "@@ -1,2 +1,2 @@\n fn a() {}\n-fn b() {}\n+fn c() {}\n".to_string(),
    });
    app.process_command(UiCommand::CompleteTool {
        id: "t1".to_string(),
        result: "Wrote 20 bytes to src/lib.rs".to_string(),
        success: true,
    });

    let rows = render_rows(&mut app, 120);
    let text = rows.join("\n");
    assert!(text.contains("Wrote lib.rs · +1 -1"), "{text}");
    assert!(!text.contains("fn c()"), "collapsed blocks hide the diff");

    app.handle_key(crossterm::event::KeyEvent::new(
        crossterm::event::KeyCode::Char('o'),
        crossterm::event::KeyModifiers::CONTROL,
    ));
    let text = render_rows(&mut app, 120).join("\n");
    assert!(text.contains("│ src/lib.rs"), "{text}");
    assert!(text.contains("2 │ - fn b() {}"), "{text}");
    assert!(text.contains("2 │ + fn c() {}"), "{text}");
    assert!(
        !text.contains("\"content\""),
        "the diff replaces the raw args"
    );
}

#[test]
fn test_edit_restored_from_history_diffs_its_arguments() {
    use crate::events::{ActivityData, MessageData, MessageRole, UiCommand};

    let mut app = crate::App::new();
    let mut data = MessageData::new(MessageRole::Assistant, "");
    data.activity = Some(ActivityData::Tool {
        id: "t1".to_string(),
        name: "edit_file".to_string(),
        args: r#"{"path":"a.rs","old_text":"let x = 1;","new_text":"let x = 2;"}"#.to_string(),
        result: Some("Replaced 1 occurrence(s) in a.rs".to_string()),
        is_error: false,
        duration_ms: None,
        policy: None,
    });
    app.process_command(UiCommand::LoadConversation(vec![data]));
    app.handle_key(crossterm::event::KeyEvent::new(
        crossterm::event::KeyCode::Char('o'),
        crossterm::event::KeyModifiers::CONTROL,
    ));

    let text = render_rows(&mut app, 120).join("\n");
    assert!(text.contains("Edited a.rs · +1 -1"), "{text}");
    assert!(text.contains("│ - let x = 1;"), "{text}");
    assert!(text.contains("│ + let x = 2;"), "{text}");
}

#[test]
fn test_diff_message_shows_each_file() {
    use crate::events::{FileDiffData, UiCommand};

    let mut app = crate::App::new();
    app.process_command(UiCommand::ShowDiff {
        title: "Changes in turn 2 · fix it".to_string(),
        files: vec![
            FileDiffData {
                path: "src/a.rs".to_string(),
                diff: "@@ -3,1 +3,1 @@\n-old\n+new\n".to_string(),
            },
            FileDiffData {
                path: "notes.md".to_string(),
                diff: "@@ -0,0 +1,1 @@\n+hello\n".to_string(),
            },
        ],
    });

    let text = render_rows(&mut app, 120).join("\n");
    assert!(text.contains("Changes in turn 2 · fix it"), "{text}");
    assert!(text.contains("src/a.rs  +1 -1"), "{text}");
    assert!(text.contains("3 │ - old"), "{text}");
    assert!(text.contains("3 │ + new"), "{text}");
    assert!(text.contains("notes.md  +1 -0"), "{text}");
    assert!(text.contains("1 │ + hello"), "{text}");
}

#[test]
fn test_chat_renders_assistant_markdown() {
    use crate::components::{Message, MessageRole};

    let mut app = crate::App::new();
    app.add_message(Message::new(
        MessageRole::Assistant,
        "## Summary\n\nThe build is **green** and `cargo test` passes.\n\n\
         - first point that is long enough to wrap onto a second line in the chat panel\n\
         - [x] done\n\n\
         | Crate | Tests |\n|---|---:|\n| aura-terminal | 231 |\n\n\
         > note: see [docs](https://example.com)",
    ));
    app.scroll_chat_to_top();
    let mut terminal = ratatui::Terminal::new(ratatui::backend::TestBackend::new(85, 30)).unwrap();
    terminal
        .draw(|frame| super::super::render(frame, &mut app, &test_theme()))
        .unwrap();

    let buffer = terminal.backend().buffer();
    let rows: Vec<String> = (0..buffer.area.height)
        .map(|y| {
            (0..buffer.area.width)
                .map(|x| buffer[(x, y)].symbol())
                .collect()
        })
        .collect();
    let screen = rows.join("\n");
    assert!(!screen.contains("AURA  "), "{screen}");
    assert!(screen.contains("Summary"), "{screen}");
    assert!(!screen.contains("##") && !screen.contains("**"), "{screen}");
    assert!(screen.contains("The build is green and cargo"), "{screen}");
    assert!(screen.contains("• [✓] done"), "{screen}");
    assert!(screen.contains("│ aura-terminal │   231 │"), "{screen}");
    assert!(screen.contains("│ note: see docs"), "{screen}");
    assert!(screen.contains("(https://example.com)"), "{screen}");

    let bullet_row = rows.iter().find(|r| r.contains("• first point")).unwrap();
    let bullet_col = bullet_row.chars().position(|c| c == '•').unwrap();
    let next_row = &rows[rows.iter().position(|r| r == bullet_row).unwrap() + 1];
    let hang_col = next_row.chars().position(|c| c.is_alphanumeric()).unwrap();
    assert_eq!(
        hang_col,
        bullet_col + 2,
        "wrapped item hangs under its text"
    );
}

#[test]
fn test_chat_prose_rows_leave_out_code_and_tables() {
    use crate::components::{Message, MessageRole};

    let mut app = crate::App::new();
    app.add_message(Message::new(MessageRole::User, "run the tests"));
    app.add_message(Message::new(
        MessageRole::Assistant,
        "Run `cargo test` now.\n\n```sh\ncargo test\n```\n\n\
         | Crate | Tests |\n|---|---:|\n| aura-terminal | 231 |\n\n---\n\nAll green.",
    ));
    app.scroll_chat_to_top();
    let mut terminal = ratatui::Terminal::new(ratatui::backend::TestBackend::new(90, 34)).unwrap();
    terminal
        .draw(|frame| super::super::render(frame, &mut app, &test_theme()))
        .unwrap();

    let buffer = terminal.backend().buffer();
    let text = |y: u16, columns: std::ops::Range<u16>| -> String {
        columns.map(|x| buffer[(x, y)].symbol()).collect()
    };
    let prose: Vec<String> = app
        .prose_rows()
        .iter()
        .map(|row| text(row.y, row.x..row.x + row.width))
        .collect();
    assert_eq!(
        prose,
        ["→ run the tests", "Run cargo test now.", "All green."],
        "only flowing text is prose"
    );
    let run = &app.prose_rows()[1];
    assert_eq!(run.code.len(), 1);
    assert_eq!(text(run.y, run.code[0].clone()), "cargo test");
}

#[test]
fn test_measured_prose_packs_into_the_cells_it_covers() {
    use crate::components::{Message, MessageRole};

    #[allow(clippy::cast_precision_loss)]
    fn half(text: &str, _: ratatui::style::Modifier) -> f32 {
        text.chars().count() as f32 / 2.0
    }

    let prompt = "word ".repeat(60);
    let prompt = prompt.trim_end();
    let mut app = crate::App::new();
    app.set_prose_measure(half);
    app.add_message(Message::new(MessageRole::User, prompt));
    let mut terminal = ratatui::Terminal::new(ratatui::backend::TestBackend::new(90, 30)).unwrap();
    let frame = terminal
        .draw(|frame| super::super::render(frame, &mut app, &test_theme()))
        .unwrap();

    let rows = app.prose_rows();
    let texts: Vec<String> = rows
        .iter()
        .map(|row| {
            (row.x + row.indent..row.x + row.width)
                .map(|x| frame.buffer[(x, row.y)].symbol())
                .collect()
        })
        .collect();
    assert!(rows.iter().all(|row| row.indent == 2), "after the prompt");
    assert_eq!(texts.join(" "), prompt, "every character is kept, in order");
    assert!(
        texts[0].chars().count() > usize::from(rows[0].width) * 3 / 2,
        "a row holds about twice as many characters as cells: {texts:?}"
    );
}

#[test]
fn test_overlay_covers_chat_prose_rows() {
    use crate::components::{Message, MessageRole};

    let mut app = crate::App::new();
    app.add_message(Message::new(MessageRole::User, "run the tests"));
    let mut terminal = ratatui::Terminal::new(ratatui::backend::TestBackend::new(90, 30)).unwrap();
    let draw = |terminal: &mut ratatui::Terminal<_>, app: &mut crate::App| {
        terminal
            .draw(|frame| super::super::render(frame, app, &test_theme()))
            .unwrap();
    };
    draw(&mut terminal, &mut app);
    assert_eq!(app.prose_rows().len(), 1);

    submit(&mut app, "/help");
    draw(&mut terminal, &mut app);
    assert!(app.prose_rows().is_empty(), "help covers the prompt");
}

#[test]
fn test_chat_reads_like_a_terminal_from_the_left_edge() {
    use crate::components::{Message, MessageRole};

    let mut app = crate::App::new();
    app.add_message(Message::new(MessageRole::User, "hi"));
    app.add_message(Message::new(
        MessageRole::Assistant,
        "The task of extracting tasks from a spec file is not currently defined \
         or available in the current context.",
    ));
    let mut terminal = ratatui::Terminal::new(ratatui::backend::TestBackend::new(100, 20)).unwrap();
    terminal
        .draw(|frame| super::super::render(frame, &mut app, &test_theme()))
        .unwrap();

    let rows = buffer_rows(terminal.backend().buffer());
    let col = |row: &str, needle: &str| {
        let i = row
            .find(needle)
            .unwrap_or_else(|| panic!("{needle:?} not in {row:?}"));
        row[..i].chars().count()
    };
    let screen = rows.join("\n");
    assert!(
        !screen.contains("YOU  ") && !screen.contains("AURA  "),
        "{screen}"
    );
    let user = rows.iter().position(|r| r.contains("→ hi")).unwrap();
    let reply = rows.iter().position(|r| r.contains("The task")).unwrap();
    // The prompt's frame takes a padding row and a border row below it,
    // then the gap between blocks.
    assert_eq!(reply, user + 4, "the reply follows the prompt: {screen}");

    let text_col = col(&rows[user], "→ hi");
    // Measured on the reply's row: the prompt's row has its frame's own
    // side borders.
    let (border, inner) = panel_around(&rows[reply], "The task");
    let base = (inner - 4).min(110);
    let width = (base * 83 + 50) / 100;
    assert_eq!(
        text_col,
        border + 1 + (inner - width) / 2,
        "centered with equal padding on both sides: {screen}"
    );
    assert_eq!(col(&rows[reply], "The task"), text_col);
}

fn buffer_rows(buffer: &ratatui::buffer::Buffer) -> Vec<String> {
    (0..buffer.area.height)
        .map(|y| {
            (0..buffer.area.width)
                .map(|x| buffer[(x, y)].symbol())
                .collect()
        })
        .collect()
}

/// The column of the panel border left of `needle` on `row`, and the
/// panel's inner width up to the next border.
fn panel_around(row: &str, needle: &str) -> (usize, usize) {
    let border = row[..row.find(needle).unwrap()]
        .chars()
        .collect::<Vec<_>>()
        .iter()
        .rposition(|&c| c == '│')
        .unwrap();
    let chars: Vec<char> = row.chars().collect();
    let right_border = border + 1 + chars[border + 1..].iter().position(|&c| c == '│').unwrap();
    let inner = right_border - border - 1;
    (border, inner)
}

#[test]
fn test_user_message_is_framed_with_padding() {
    use crate::components::{Message, MessageRole};

    let theme = test_theme();
    let mut app = crate::App::new();
    app.add_message(Message::new(MessageRole::User, "hi"));
    app.add_message(Message::new(MessageRole::Assistant, "hello"));
    let mut terminal = ratatui::Terminal::new(ratatui::backend::TestBackend::new(100, 20)).unwrap();
    terminal
        .draw(|frame| super::super::render(frame, &mut app, &theme))
        .unwrap();
    let buffer = terminal.backend().buffer();
    let rows: Vec<String> = (0..buffer.area.height)
        .map(|y| {
            (0..buffer.area.width)
                .map(|x| buffer[(x, y)].symbol())
                .collect()
        })
        .collect();
    let screen = rows.join("\n");
    let user = rows.iter().position(|r| r.contains("→ hi")).unwrap();
    let text = rows[user].find("→ hi").unwrap();
    let text_col = rows[user][..text].chars().count();
    let left = text_col - 2;

    let at = |row: usize, x: usize| rows[row].chars().nth(x).unwrap();
    assert_eq!(at(user - 2, left), '╭', "top border:\n{screen}");
    assert_eq!(at(user + 2, left), '╰', "bottom border:\n{screen}");
    for row in user - 1..=user + 1 {
        assert_eq!(at(row, left), '│', "side border on row {row}:\n{screen}");
    }
    let right = rows[user - 2].chars().position(|c| c == '╮').unwrap();
    assert_eq!(at(user + 2, right), '╯', "{screen}");
    assert_eq!(at(user, right), '│', "{screen}");

    // The frame is not shaded: no cell in it, border included, carries
    // the surface background.
    #[allow(clippy::cast_possible_truncation)]
    let cell = |row: usize, x: usize| &buffer[(x as u16, row as u16)];
    for row in user - 2..=user + 2 {
        for x in left..=right {
            assert_ne!(
                cell(row, x).bg,
                theme.colors.surface,
                "({x}, {row}):\n{screen}"
            );
        }
    }
}
