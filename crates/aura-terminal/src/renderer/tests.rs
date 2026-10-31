use super::markdown::{
    find_next_marker, parse_content_segments, parse_markdown_inline, parse_markdown_line,
    ContentSegment, MarkerType,
};
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
// Markdown Parsing Tests
// ========================================================================

#[test]
fn test_parse_markdown_line_plain() {
    let theme = test_theme();
    let base_style = Style::default().fg(Color::White);

    let spans = parse_markdown_line("hello world", base_style, &theme);
    assert!(!spans.is_empty());
    let text: String = spans.iter().map(|s| s.content.as_ref()).collect();
    assert!(text.contains("hello world"));
}

#[test]
fn test_parse_markdown_line_header() {
    let theme = test_theme();
    let base_style = Style::default().fg(Color::White);

    let spans = parse_markdown_line("# Header", base_style, &theme);
    assert!(!spans.is_empty());
    assert!(spans[0].style.add_modifier.contains(Modifier::BOLD));
}

#[test]
fn test_parse_markdown_line_blockquote() {
    let theme = test_theme();
    let base_style = Style::default().fg(Color::White);

    let spans = parse_markdown_line("> quoted text", base_style, &theme);
    assert!(!spans.is_empty());
    assert!(spans[0].style.add_modifier.contains(Modifier::ITALIC));
}

#[test]
fn test_parse_markdown_line_list_item() {
    let theme = test_theme();
    let base_style = Style::default().fg(Color::White);

    let spans = parse_markdown_line("- list item", base_style, &theme);
    assert!(!spans.is_empty());
    let text: String = spans.iter().map(|s| s.content.as_ref()).collect();
    assert!(text.contains('•') || text.contains('-'));
}

#[test]
fn test_parse_markdown_inline_bold() {
    let theme = test_theme();
    let base_style = Style::default().fg(Color::White);

    let spans = parse_markdown_inline("hello **bold** world", base_style, &theme);
    assert!(spans.len() >= 3);
    let bold_span = spans.iter().find(|s| s.content.as_ref() == "bold");
    assert!(bold_span.is_some());
    assert!(bold_span
        .unwrap()
        .style
        .add_modifier
        .contains(Modifier::BOLD));
}

#[test]
fn test_parse_markdown_inline_italic() {
    let theme = test_theme();
    let base_style = Style::default().fg(Color::White);

    let spans = parse_markdown_inline("hello *italic* world", base_style, &theme);
    assert!(spans.len() >= 3);
    let italic_span = spans.iter().find(|s| s.content.as_ref() == "italic");
    assert!(italic_span.is_some());
    assert!(italic_span
        .unwrap()
        .style
        .add_modifier
        .contains(Modifier::ITALIC));
}

#[test]
fn test_parse_markdown_inline_code() {
    let theme = test_theme();
    let base_style = Style::default().fg(Color::White);

    let spans = parse_markdown_inline("run `cargo build`", base_style, &theme);
    assert!(spans.len() >= 2);
    let code_span = spans.iter().find(|s| s.content.as_ref() == "cargo build");
    assert!(code_span.is_some());
}

#[test]
fn test_parse_markdown_inline_unclosed() {
    let theme = test_theme();
    let base_style = Style::default().fg(Color::White);

    let spans = parse_markdown_inline("hello **unclosed", base_style, &theme);
    assert!(!spans.is_empty());
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
// Marker Finding Tests
// ========================================================================

#[test]
fn test_find_next_marker_bold() {
    let result = find_next_marker("hello **bold** world");
    assert!(result.is_some());
    let (pos, marker_type, len) = result.unwrap();
    assert_eq!(pos, 6);
    assert!(matches!(marker_type, MarkerType::Bold));
    assert_eq!(len, 2);
}

#[test]
fn test_find_next_marker_italic() {
    let result = find_next_marker("hello *italic* world");
    assert!(result.is_some());
    let (pos, marker_type, len) = result.unwrap();
    assert_eq!(pos, 6);
    assert!(matches!(marker_type, MarkerType::Italic));
    assert_eq!(len, 1);
}

#[test]
fn test_find_next_marker_code() {
    let result = find_next_marker("run `code` here");
    assert!(result.is_some());
    let (pos, marker_type, len) = result.unwrap();
    assert_eq!(pos, 4);
    assert!(matches!(marker_type, MarkerType::Code));
    assert_eq!(len, 1);
}

#[test]
fn test_find_next_marker_none() {
    let result = find_next_marker("no markers here");
    assert!(result.is_none());
}

#[test]
fn test_find_next_marker_prefers_earliest() {
    let result = find_next_marker("**bold** *italic*");
    assert!(result.is_some());
    let (pos, marker_type, _) = result.unwrap();
    assert_eq!(pos, 0);
    assert!(matches!(marker_type, MarkerType::Bold));
}

#[test]
fn test_find_next_marker_skips_triple_backticks() {
    let result = find_next_marker("```rust");
    assert!(result.is_none());
}

// ========================================================================
// Layout
// ========================================================================

#[test]
fn test_default_layout_shows_agents_column() {
    use crate::events::{AgentSummary, UiCommand};

    let mut app = crate::App::new();
    app.process_command(UiCommand::SetAgents(vec![
        AgentSummary {
            id: "aa".to_string(),
            name: "Terminal Agent".to_string(),
            zns_id: "0://terminal/aa".to_string(),
            is_active: true,
            record_count: 12,
            last_active: "-".to_string(),
        },
        AgentSummary {
            id: "bb".to_string(),
            name: "Researcher".to_string(),
            zns_id: "0://terminal/bb".to_string(),
            is_active: false,
            record_count: 0,
            last_active: "-".to_string(),
        },
    ]));
    app.process_command(UiCommand::SetActiveAgent("aa".to_string()));

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
}

#[test]
fn test_chat_message_text_column_is_aligned() {
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
        row.find(needle)
            .map(|i| row[..i].chars().count())
            .unwrap_or_else(|| panic!("{needle:?} not in {row:?}"))
    };
    let user = rows.iter().find(|r| r.contains("<YOU>")).unwrap();
    let aura = rows.iter().position(|r| r.contains("<AURA>")).unwrap();

    assert_eq!(col(user, "<YOU>"), col(&rows[aura], "<AURA>"));

    let text_col = col(user, "hi");
    assert_eq!(col(&rows[aura], "The task"), text_col);
    assert_eq!(rows[aura + 1].chars().nth(text_col - 1), Some(' '));
    assert_ne!(rows[aura + 1].chars().nth(text_col), Some(' '));
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
fn test_prompt_sits_in_gutter_left_of_chat_aligned_typing() {
    let mut app = app_with_model(None);
    app.set_shell_prompt("[u@h d]$ ");
    type_text(&mut app, "hello");
    let rows = render_rows(&mut app, 120);
    let input_row = &rows[rows.len() - 2];

    // Swarm takes 20% of 120 columns; typing starts past the chat padding
    // (3) and indent (2), the same column the old `> ` chevron led to.
    let typing_col = 24 + 3 + 2;
    let chars: Vec<char> = input_row.chars().collect();
    let prompt: String = chars[typing_col - 9..typing_col].iter().collect();
    let typed: String = chars[typing_col..typing_col + 5].iter().collect();
    assert_eq!(prompt, "[u@h d]$ ", "{input_row:?}");
    assert_eq!(typed, "hello", "{input_row:?}");
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
    assert!(chat.contains("<AURA> "), "{chat}");
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
