use super::markdown::{
    find_next_marker, parse_content_segments, parse_markdown_inline, parse_markdown_line,
    ContentSegment, MarkerType,
};
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
