use super::markdown::{parse_content_segments, ContentSegment};
use super::panels::truncate_record_info;
use super::text::{centered_rect, display_width, wrap_words};
use crate::Theme;
use ratatui::{
    layout::Rect,
    style::{Color, Modifier, Style},
};

fn test_theme() -> Theme {
    Theme::cyber()
}

// ========================================================================
// Word Wrapping Tests
// ========================================================================

#[test]
fn test_wrap_words_simple() {
    let result = wrap_words("hello world", 20);
    assert_eq!(result, vec!["hello world"]);
}

#[test]
fn test_wrap_words_wraps_at_boundary() {
    let result = wrap_words("hello world foo bar", 11);
    assert_eq!(result, vec!["hello world", "foo bar"]);
}

#[test]
fn test_wrap_words_empty_input() {
    let result = wrap_words("", 20);
    assert_eq!(result, vec![""]);
}

#[test]
fn test_wrap_words_whitespace_only() {
    let result = wrap_words("   ", 20);
    assert_eq!(result, vec![""]);
}

#[test]
fn test_wrap_words_long_word() {
    let result = wrap_words("supercalifragilistic", 10);
    assert_eq!(result.len(), 2);
    assert!(result[0].len() <= 10);
}

#[test]
fn test_wrap_words_zero_width() {
    let result = wrap_words("hello", 0);
    assert_eq!(result, vec!["hello"]);
}

#[test]
fn test_wrap_words_exact_fit() {
    let result = wrap_words("hello", 5);
    assert_eq!(result, vec!["hello"]);
}

// ========================================================================
// Display Width Tests
// ========================================================================

#[test]
fn test_display_width_ascii() {
    assert_eq!(display_width("hello"), 5);
    assert_eq!(display_width(""), 0);
}

#[test]
fn test_display_width_unicode() {
    let width = display_width("你好");
    assert!(width >= 2);
}

#[test]
fn test_display_width_emoji() {
    let width = display_width("🎉");
    assert!(width >= 1);
}

// ========================================================================
// Content Segment Parsing Tests
// ========================================================================

#[test]
fn test_parse_content_segments_text_only() {
    let segments = parse_content_segments("Hello\nWorld");
    assert_eq!(segments.len(), 1);
    assert!(matches!(&segments[0], ContentSegment::Text(_)));
}

#[test]
fn test_parse_content_segments_code_block() {
    let content = "Before\n```rust\nfn main() {}\n```\nAfter";
    let segments = parse_content_segments(content);

    assert_eq!(segments.len(), 3);
    assert!(matches!(&segments[0], ContentSegment::Text(_)));
    assert!(matches!(&segments[1], ContentSegment::CodeBlock { .. }));
    assert!(matches!(&segments[2], ContentSegment::Text(_)));

    if let ContentSegment::CodeBlock { language, code } = &segments[1] {
        assert_eq!(language, "rust");
        assert!(code.contains("fn main()"));
    }
}

#[test]
fn test_parse_content_segments_multiple_code_blocks() {
    let content = "```python\nprint('hello')\n```\ntext\n```js\nconsole.log('hi')\n```";
    let segments = parse_content_segments(content);

    assert_eq!(segments.len(), 3);
    assert!(matches!(&segments[0], ContentSegment::CodeBlock { .. }));
    assert!(matches!(&segments[1], ContentSegment::Text(_)));
    assert!(matches!(&segments[2], ContentSegment::CodeBlock { .. }));
}

#[test]
fn test_parse_content_segments_unclosed_code_block() {
    let content = "Before\n```rust\nfn main() {}";
    let segments = parse_content_segments(content);

    assert!(!segments.is_empty());
    for segment in &segments {
        assert!(matches!(segment, ContentSegment::Text(_)));
    }
}

// ========================================================================
// Centered Rect Tests
// ========================================================================

#[test]
fn test_centered_rect_normal() {
    let area = Rect {
        x: 0,
        y: 0,
        width: 100,
        height: 50,
    };
    let result = centered_rect(20, 10, area);

    assert_eq!(result.width, 20);
    assert_eq!(result.height, 10);
    assert_eq!(result.x, 40);
    assert_eq!(result.y, 20);
}

#[test]
fn test_centered_rect_larger_than_area() {
    let area = Rect {
        x: 0,
        y: 0,
        width: 50,
        height: 30,
    };
    let result = centered_rect(100, 50, area);

    assert_eq!(result.width, 50);
    assert_eq!(result.height, 30);
}

#[test]
fn test_centered_rect_with_offset() {
    let area = Rect {
        x: 10,
        y: 5,
        width: 100,
        height: 50,
    };
    let result = centered_rect(20, 10, area);

    assert_eq!(result.x, 50);
    assert_eq!(result.y, 25);
}

// ========================================================================
// Layout
// ========================================================================

#[test]
fn test_default_layout_shows_agents_column() {
    use crate::events::{AgentDefinitionSummary, AgentSummary, UiCommand};

    let mut app = crate::App::new();
    app.process_command(UiCommand::SetAgents(vec![
        AgentSummary {
            id: "aa".to_string(),
            name: "Terminal Agent".to_string(),
            zns_id: "0://terminal/aa".to_string(),
            is_active: true,
            record_count: 12,
            last_active: "-".to_string(),
            ..Default::default()
        },
        AgentSummary {
            id: "bb".to_string(),
            name: "Researcher".to_string(),
            zns_id: "0://terminal/bb".to_string(),
            is_active: false,
            record_count: 0,
            last_active: "-".to_string(),
            ..Default::default()
        },
        AgentSummary {
            id: "cc".to_string(),
            name: "reviewer #1".to_string(),
            zns_id: "0://terminal/cc".to_string(),
            definition: Some("reviewer".to_string()),
            depth: 1,
            ..Default::default()
        },
    ]));
    app.process_command(UiCommand::SetActiveAgent("aa".to_string()));
    app.process_command(UiCommand::SetAgentDefinitions(vec![
        AgentDefinitionSummary {
            name: "reviewer".to_string(),
            description: "Reviews diffs".to_string(),
        },
    ]));

    let mut terminal = ratatui::Terminal::new(ratatui::backend::TestBackend::new(120, 30)).unwrap();
    terminal
        .draw(|frame| super::render(frame, &mut app, &test_theme()))
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
    assert!(screen.contains("AGENTS"));
    assert!(screen.contains("CHAT"));
    assert!(screen.contains("RECORD"));
    assert!(rows
        .iter()
        .any(|r| r.contains("● 1. Terminal") && r.contains("12│")));
    assert!(screen.contains("2. Researcher"));
    let column_of = |needle: &str| {
        rows.iter()
            .find_map(|r| r.find(needle).map(|i| r[..i].chars().count()))
            .unwrap()
    };
    assert_eq!(column_of("New agent"), column_of("Researcher"));
    assert_eq!(column_of("+"), column_of("2."));
    assert!(screen.contains("Alt+↑↓ switch  ^N new"));
    // Spawned instances sit indented under their parent.
    assert_eq!(column_of("reviewer #1"), column_of("Researcher") + 2);

    assert!(screen.contains("PERMANENT AGENTS"));
    assert!(rows.iter().any(|r| r.contains("reviewer  Reviews")));
    assert!(screen.contains("+  New permanent"));
    assert!(screen.contains("⏎ spawn e edit d del"));
    assert!(row_index(&rows, "PERMANENT AGENTS") > row_index(&rows, "Researcher"));
}

fn row_index(rows: &[String], needle: &str) -> usize {
    rows.iter().position(|r| r.contains(needle)).unwrap()
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
    let mut terminal = ratatui::Terminal::new(ratatui::backend::TestBackend::new(90, 30)).unwrap();
    terminal
        .draw(|frame| super::render(frame, &mut app, &test_theme()))
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
    let header = rows.iter().position(|r| r.contains("AURA  ")).unwrap();
    assert!(rows[header + 1].contains("Summary"), "{screen}");
    assert!(!screen.contains("##") && !screen.contains("**"), "{screen}");
    assert!(screen.contains("The build is green and cargo"), "{screen}");
    assert!(screen.contains("• [✓] done"), "{screen}");
    assert!(screen.contains("│ aura-terminal │   231 │"), "{screen}");
    assert!(screen.contains("│ note: see docs"), "{screen}");
    assert!(screen.contains("│ (https://example.com)"), "{screen}");

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
fn test_chat_names_sit_above_centered_text() {
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
        .draw(|frame| super::render(frame, &mut app, &test_theme()))
        .unwrap();

    let buffer = terminal.backend().buffer();
    let rows: Vec<String> = (0..buffer.area.height)
        .map(|y| {
            (0..buffer.area.width)
                .map(|x| buffer[(x, y)].symbol())
                .collect()
        })
        .collect();
    let col = |row: &str, needle: &str| {
        let i = row
            .find(needle)
            .unwrap_or_else(|| panic!("{needle:?} not in {row:?}"));
        row[..i].chars().count()
    };
    let user = rows.iter().position(|r| r.contains("YOU  ")).unwrap();
    let aura = rows.iter().position(|r| r.contains("AURA  ")).unwrap();

    let text_col = col(&rows[user], "YOU");
    assert_eq!(col(&rows[user + 1], "hi"), text_col);
    assert_eq!(col(&rows[aura], "AURA"), text_col);
    assert_eq!(col(&rows[aura + 1], "The task"), text_col);
    assert_eq!(rows[aura + 2].chars().nth(text_col - 1), Some(' '));
    assert_ne!(rows[aura + 2].chars().nth(text_col), Some(' '));
}

#[test]
fn test_chat_text_is_centered_with_margins() {
    use super::panels::centered_text_area;

    let gaps = |area: Rect| {
        let text = centered_text_area(area);
        (
            text.x - area.x,
            (area.x + area.width) - (text.x + text.width),
        )
    };
    assert_eq!(gaps(Rect::new(10, 0, 60, 20)), (4, 4));
    let (left, right) = gaps(Rect::new(10, 0, 61, 20));
    assert!(left.abs_diff(right) <= 1);
    let wide = Rect::new(0, 0, 200, 20);
    assert_eq!(centered_text_area(wide).width, 100);
    assert_eq!(gaps(wide), (50, 50));
}

fn submit(app: &mut crate::App, text: &str) {
    use crossterm::event::{KeyCode, KeyEvent};
    for c in text.chars() {
        let _ = app.handle_key(KeyEvent::from(KeyCode::Char(c)));
    }
    let _ = app.handle_key(KeyEvent::from(KeyCode::Enter));
}

#[test]
fn test_sandbox_panel_sits_below_record() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir(dir.path().join("src")).unwrap();
    std::fs::write(dir.path().join("Cargo.toml"), "").unwrap();

    let mut app = crate::App::new();
    app.process_command(crate::events::UiCommand::SetSandboxRoot(
        dir.path().to_path_buf(),
    ));
    let rows = render_rows(&mut app, 120);

    let record = rows.iter().position(|r| r.contains(" RECORD ")).unwrap();
    let sandbox = rows.iter().position(|r| r.contains(" SANDBOX ")).unwrap();
    assert!(sandbox > record + 10, "record={record} sandbox={sandbox}");
    let column = |row: &str, title: &str| row[..row.find(title).unwrap()].chars().count();
    assert_eq!(
        column(&rows[record], " RECORD "),
        column(&rows[sandbox], " SANDBOX ")
    );
    assert!(rows[sandbox + 1].contains("▸ src/"));
    assert!(rows[sandbox + 2].contains("Cargo.toml"));
}

#[test]
fn test_files_command_toggles_sandbox_panel() {
    let mut app = crate::App::new();
    submit(&mut app, "/record");
    submit(&mut app, "/files");
    let screen = render_rows(&mut app, 120).join("\n");
    assert!(!screen.contains(" RECORD "));
    assert!(!screen.contains(" SANDBOX "));

    submit(&mut app, "/files");
    let screen = render_rows(&mut app, 120).join("\n");
    assert!(screen.contains(" SANDBOX "));
    assert!(!screen.contains(" RECORD "));
}

fn render_with_user(user: Option<&str>, width: u16) -> Vec<String> {
    render_rows(&mut app_with_model(user), width)
}

fn app_with_model(user: Option<&str>) -> crate::App {
    use crate::events::{ModelOptionView, UiCommand};

    let mut app = crate::App::new();
    app.set_shell_prompt("[n3o@arch-aura aura-harness]$ ");
    app.process_command(UiCommand::SetModels {
        options: vec![ModelOptionView {
            id: "local/qwen3:4b".to_string(),
            label: "qwen3:4b".to_string(),
            local: true,
            installed: true,
        }],
        current: "local/qwen3:4b".to_string(),
        user: user.map(str::to_string),
    });
    app.process_command(UiCommand::SetApiStatus {
        url: Some("http://127.0.0.1:8080".to_string()),
        active: true,
    });
    app
}

fn render_rows(app: &mut crate::App, width: u16) -> Vec<String> {
    let mut terminal =
        ratatui::Terminal::new(ratatui::backend::TestBackend::new(width, 30)).unwrap();
    terminal
        .draw(|frame| super::render(frame, app, &test_theme()))
        .unwrap();

    let buffer = terminal.backend().buffer();
    (0..buffer.area.height)
        .map(|y| {
            (0..buffer.area.width)
                .map(|x| buffer[(x, y)].symbol())
                .collect()
        })
        .collect()
}

#[test]
fn test_slash_menu_spans_chat_width_above_input() {
    let mut app = app_with_model(None);
    type_text(&mut app, "/lo");
    let rows = render_rows(&mut app, 120);
    let n = rows.len();

    let title = rows.iter().find(|r| r.contains("┌ CHAT ")).unwrap();
    let chat_left = title[..title.find("┌ CHAT ").unwrap()].chars().count();
    let chat_right = chat_left
        + 1
        + title
            .chars()
            .skip(chat_left + 1)
            .position(|c| c == '┐')
            .unwrap();

    let menu_bottom = &rows[n - 3];
    let chars: Vec<char> = menu_bottom.chars().collect();
    assert_eq!(chars[chat_left], '└', "{menu_bottom:?}");
    assert_eq!(chars[chat_right], '┘', "{menu_bottom:?}");
    assert!(menu_bottom.contains("Esc close"), "{menu_bottom:?}");

    assert!(rows[n - 5].contains("› /login"), "{:?}", rows[n - 5]);
    assert!(rows[n - 5].contains("Login for cloud models"));
    assert!(rows[n - 4].contains("  /logout"), "{:?}", rows[n - 4]);
    assert!(rows[n - 6].contains(" COMMANDS "), "{:?}", rows[n - 6]);
    let menu_top: Vec<char> = rows[n - 6].chars().collect();
    assert_eq!(menu_top[chat_left], '┌');
    assert_eq!(menu_top[chat_right], '┐');

    type_text(&mut app, " ");
    let rows = render_rows(&mut app, 120);
    assert!(!rows.join("\n").contains(" COMMANDS "));
}

#[test]
fn test_header_shows_local_url_and_model_moves_to_input() {
    let rows = render_with_user(None, 120);
    assert!(rows[1]
        .trim_end()
        .ends_with("● local  http://127.0.0.1:8080"));
    let header = rows[..3].join("\n");
    assert!(!header.contains("qwen3:4b"));

    let n = rows.len();
    assert!(rows[n - 3].contains('└'), "panel border above input");
    assert!(
        rows[n - 1].chars().all(|c| c == '─'),
        "rule below input: {:?}",
        rows[n - 1]
    );
    let input_row = rows[n - 2].trim_end();
    assert!(
        input_row.contains("[n3o@arch-aura aura-harness]$ "),
        "{input_row:?}"
    );
    assert!(!input_row.contains("> "), "{input_row:?}");
    assert!(input_row.ends_with("qwen3:4b  ● Ready"), "{input_row:?}");
    assert!(!input_row.contains("/model"));
}

fn type_text(app: &mut crate::App, text: &str) {
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

    for c in text.chars() {
        app.handle_key(KeyEvent::new(KeyCode::Char(c), KeyModifiers::empty()));
    }
}

#[test]
fn test_prompt_colors_user_green_and_host_gray() {
    let theme = test_theme();
    let cyan = theme.colors.primary;
    let spans = super::input::prompt_spans("[n3o@arch-aura aura-harness]$ ", cyan, &theme);
    let parts: Vec<(&str, Option<Color>)> = spans
        .iter()
        .map(|s| (s.content.as_ref(), s.style.fg))
        .collect();
    assert_eq!(
        parts,
        [
            ("[", Some(cyan)),
            ("n3o", Some(theme.colors.success)),
            ("@arch-aura", Some(theme.colors.muted)),
            (" aura-harness]$ ", Some(cyan)),
        ]
    );

    let email = super::input::prompt_spans("Email: ", cyan, &theme);
    assert_eq!(email.len(), 1);
    assert_eq!(email[0].style.fg, Some(cyan));
}

#[test]
fn test_typing_starts_at_chat_left_border_with_prompt_under_agents() {
    let mut app = app_with_model(None);
    app.set_shell_prompt("[u@h d]$ ");
    type_text(&mut app, "hello");
    let rows = render_rows(&mut app, 120);
    let input_row = &rows[rows.len() - 2];

    let chat_border = rows
        .iter()
        .find_map(|r| {
            r.find("┌ CHAT")
                .or_else(|| r.find("╭ CHAT"))
                .map(|i| r[..i].chars().count())
        })
        .expect("chat title row");
    let chars: Vec<char> = input_row.chars().collect();
    let typing = chat_border + 1;
    let typed: String = chars[typing..typing + 5].iter().collect();
    let prompt: String = chars[typing - 9..typing].iter().collect();
    assert_eq!(typed, "hello", "{input_row:?}");
    assert_eq!(prompt, "[u@h d]$ ", "{input_row:?}");
    assert!(
        chars[..typing - 9].iter().all(|c| *c == ' '),
        "{input_row:?}"
    );
}

#[test]
fn test_wide_prompt_pushes_typing_past_chat_border() {
    let mut app = app_with_model(None);
    app.set_shell_prompt("[someone@a-very-long-hostname some-project-dir]$ ");
    type_text(&mut app, "hello");
    let rows = render_rows(&mut app, 120);
    let input_row = &rows[rows.len() - 2];

    assert!(
        input_row.starts_with(" [someone@a-very-long-hostname some-project-dir]$ hello"),
        "{input_row:?}"
    );
}

#[test]
fn test_prompt_pushes_typing_right_without_gutter() {
    let mut app = app_with_model(None);
    submit_message(&mut app, "/swarm");
    type_text(&mut app, "hello");
    let rows = render_rows(&mut app, 120);
    let input_row = &rows[rows.len() - 2];

    assert!(
        input_row.starts_with(" [n3o@arch-aura aura-harness]$ hello"),
        "{input_row:?}"
    );
}

#[test]
fn test_outer_text_is_inset_to_panel_borders() {
    let width = 100;
    let rows = render_with_user(None, width);
    let last_col = |row: &str| row.trim_end().chars().count() - 1;
    let inner_right = usize::from(width) - 2;

    assert!(rows[1].starts_with(" AURA"), "{:?}", rows[1]);
    assert_eq!(last_col(&rows[1]), inner_right, "{:?}", rows[1]);
    let input_row = &rows[rows.len() - 2];
    assert_eq!(last_col(input_row), inner_right, "{input_row:?}");
}

#[test]
fn test_header_shows_logged_in_user() {
    let rows = render_with_user(Some("alice"), 120);
    assert!(rows[1]
        .trim_end()
        .ends_with("● alice  http://127.0.0.1:8080"));
    assert!(!rows[..3].join("\n").contains("local"));
}

#[test]
fn test_input_hides_model_when_narrow() {
    let rows = render_with_user(None, 46);
    let input_row = &rows[rows.len() - 2];
    assert!(!input_row.contains("qwen3:4b"));
    assert!(input_row.contains("Ready"));
}

// ========================================================================
// Record Info Truncation Tests
// ========================================================================

#[test]
fn test_truncate_record_info_fits() {
    assert_eq!(
        truncate_record_info("read_file src/main.rs", 40),
        "read_file src/main.rs"
    );
}

#[test]
fn test_truncate_record_info_adds_ellipsis() {
    let out = truncate_record_info("fix the login redirect bug", 10);
    assert_eq!(out, "fix the l…");
    assert!(display_width(&out) <= 10);
}

#[test]
fn test_truncate_record_info_folds_newlines() {
    let out = truncate_record_info("line one\nline two\r\n\tthree", 80);
    assert_eq!(out, "line one line two three");
}

#[test]
fn test_truncate_record_info_wide_chars_no_panic() {
    let out = truncate_record_info("修复登录错误 🚀 then more text", 7);
    assert!(display_width(&out) <= 7, "too wide: {out:?}");
    assert!(out.ends_with('…'));
}

#[test]
fn test_format_elapsed() {
    use super::panels::format_elapsed;
    use std::time::Duration;

    assert_eq!(format_elapsed(Duration::from_secs(7)), "7s");
    assert_eq!(format_elapsed(Duration::from_secs(125)), "2m 05s");
}

fn submit_message(app: &mut crate::App, text: &str) {
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

    for c in text.chars() {
        app.handle_key(KeyEvent::new(KeyCode::Char(c), KeyModifiers::empty()));
    }
    app.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::empty()));
}

#[test]
fn test_chat_shows_working_indicator_until_reply_streams() {
    use crate::events::UiCommand;

    let mut app = crate::App::new();
    submit_message(&mut app, "hi");
    let chat = render_rows(&mut app, 120).join("\n");
    assert!(chat.contains("AURA  "), "{chat}");
    assert!(chat.contains("thinking…"), "{chat}");

    app.process_command(UiCommand::StartStreaming);
    assert!(render_rows(&mut app, 120).join("\n").contains("thinking…"));

    app.process_command(UiCommand::AppendText("hello".to_string()));
    assert!(!render_rows(&mut app, 120).join("\n").contains("thinking…"));

    app.process_command(UiCommand::Complete);
    assert!(app.working_label().is_none());
}

#[test]
fn test_working_indicator_names_running_tool() {
    use crate::events::{ToolData, UiCommand};

    let mut app = crate::App::new();
    submit_message(&mut app, "read it");
    app.process_command(UiCommand::ShowTool(ToolData {
        id: "t1".to_string(),
        name: "read_file".to_string(),
        args: String::new(),
    }));
    assert_eq!(app.working_label().as_deref(), Some("running read_file"));

    app.process_command(UiCommand::CompleteTool {
        id: "t1".to_string(),
        result: "no such file".to_string(),
        success: false,
    });
    assert_eq!(app.working_label().as_deref(), Some("thinking"));
}

#[test]
fn test_truncate_record_info_zero_width() {
    assert_eq!(truncate_record_info("anything", 0), "");
    assert_eq!(truncate_record_info("", 20), "");
}

fn render_buffer(app: &mut crate::App, width: u16) -> ratatui::buffer::Buffer {
    let mut terminal =
        ratatui::Terminal::new(ratatui::backend::TestBackend::new(width, 30)).unwrap();
    terminal
        .draw(|frame| super::render(frame, app, &test_theme()))
        .unwrap();
    terminal.backend().buffer().clone()
}

/// Column of `needle` in row `y`, and the cell style there.
fn find_cell(buffer: &ratatui::buffer::Buffer, y: u16, needle: &str) -> Option<(u16, Style)> {
    let needle: Vec<String> = needle.chars().map(String::from).collect();
    let width = buffer.area.width;
    let n = u16::try_from(needle.len()).ok()?;
    let col = (0..width.saturating_sub(n)).find(|&x| {
        needle
            .iter()
            .zip(x..)
            .all(|(c, cx)| buffer[(cx, y)].symbol() == c)
    })?;
    Some((col, buffer[(col, y)].style()))
}

#[test]
fn test_scrolled_chat_stays_put_while_reply_streams() {
    use crate::components::{Message, MessageRole};
    use crate::events::UiCommand;
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

    let mut app = crate::App::new();
    for i in 0..40 {
        app.add_message(Message::new(MessageRole::User, &format!("note {i:02}")));
    }
    render_rows(&mut app, 100);
    app.scroll_up(12);
    let notes = |rows: Vec<String>| -> Vec<String> {
        rows.into_iter()
            .filter(|row| row.contains("note "))
            .collect()
    };
    let before = render_rows(&mut app, 100);
    assert!(before.join("\n").contains("↓ 12 lines below"));

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
    let screen = render_rows(&mut app, 100).join("\n");
    assert!(screen.contains("draft reply"), "{screen}");
    assert!(screen.contains("next"), "{screen}");

    let narrow = render_rows(&mut app, 60).join("\n");
    assert!(narrow.contains("draft reply"), "{narrow}");
}

#[test]
fn test_command_input_is_highlighted() {
    let theme = test_theme();
    let mut app = app_with_model(None);
    type_text(&mut app, "!echo -n hi");
    let buffer = render_buffer(&mut app, 120);
    let input_y = buffer.area.height - 2;

    let (_, program) = find_cell(&buffer, input_y, "echo").unwrap();
    assert_eq!(program.fg, Some(theme.colors.success));
    assert!(program.add_modifier.contains(Modifier::BOLD));
    let (_, flag) = find_cell(&buffer, input_y, "-n").unwrap();
    assert_eq!(flag.fg, Some(theme.colors.warning));
}

#[test]
fn test_prose_input_stays_muted() {
    let theme = test_theme();
    let mut app = app_with_model(None);
    type_text(&mut app, "please explain");
    let buffer = render_buffer(&mut app, 120);

    let (_, style) = find_cell(&buffer, buffer.area.height - 2, "please").unwrap();
    assert_eq!(style.fg, Some(theme.colors.muted));
}

#[test]
fn test_command_output_renders_like_a_terminal() {
    use crate::events::UiCommand;

    let theme = test_theme();
    let mut app = app_with_model(None);
    app.set_shell_prompt("[u@h d]$ ");
    submit_message(&mut app, "!ls -la");
    app.process_command(UiCommand::CommandFinished {
        output: "total 0\ndrwxr-xr-x  2 u u  40 \x1b[01;34msrc\x1b[0m\n".to_string(),
        exit_code: None,
    });
    app.process_command(UiCommand::Complete);

    let buffer = render_buffer(&mut app, 120);
    let rows = render_rows(&mut app, 120);
    let echo = rows.iter().position(|r| r.contains("ls -la")).unwrap();
    assert!(rows[echo - 1].contains("YOU  "), "{:?}", rows[echo - 1]);
    assert!(!rows.join("\n").contains("[u@h d]$ ls"));
    let total = &rows[echo + 1];
    let listing = &rows[echo + 2];
    assert!(total.contains("total 0"), "{total:?}");
    assert!(listing.contains("drwxr-xr-x  2 u u  40 src"), "{listing:?}");
    assert!(!rows.join("\n").contains('\x1b'));
    let column = |row: &str, needle: &str| row.find(needle).map(|i| row[..i].chars().count());
    assert_eq!(
        column(total, "total"),
        column(&rows[echo], "ls -la"),
        "output starts under the command"
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
    submit_message(&mut app, "!false");
    app.process_command(UiCommand::CommandFinished {
        output: String::new(),
        exit_code: Some(1),
    });

    let rows = render_rows(&mut app, 120);
    let echo = rows.iter().position(|r| r.contains("YOU  ")).unwrap() + 1;
    assert!(rows[echo].contains("false"), "{:?}", rows[echo]);
    assert!(rows[echo + 1].contains("exit 1"), "{:?}", rows[echo + 1]);
}
