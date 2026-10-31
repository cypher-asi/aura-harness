use super::inline::parse_inline;
use super::render_markdown;
use super::wrap::wrap_spans;
use crate::Theme;
use ratatui::{
    style::{Color, Modifier, Style},
    text::{Line, Span},
};

fn theme() -> Theme {
    Theme::cyber()
}

fn base() -> Style {
    Style::default().fg(Color::White)
}

fn render(text: &str, width: usize) -> Vec<Line<'static>> {
    render_markdown(text, width, base(), &theme())
}

fn texts(lines: &[Line<'_>]) -> Vec<String> {
    lines
        .iter()
        .map(|line| line.spans.iter().map(|s| s.content.as_ref()).collect())
        .collect()
}

fn span_style(spans: &[Span<'_>], content: &str) -> Style {
    spans
        .iter()
        .find(|s| s.content == content)
        .unwrap_or_else(|| panic!("no span {content:?} in {spans:?}"))
        .style
}

fn has(style: Style, modifier: Modifier) -> bool {
    style.add_modifier.contains(modifier)
}

// ============================================================================
// Inline
// ============================================================================

#[test]
fn inline_bold_italic_strike_and_code() {
    let spans = parse_inline(
        "a **bold** *it* _em_ ***both*** ~~gone~~ `x * y`",
        base(),
        &theme(),
    );
    let text: String = spans.iter().map(|s| s.content.as_ref()).collect();
    assert_eq!(text, "a bold it em both gone x * y");
    assert!(has(span_style(&spans, "bold"), Modifier::BOLD));
    assert!(has(span_style(&spans, "it"), Modifier::ITALIC));
    assert!(has(span_style(&spans, "em"), Modifier::ITALIC));
    let both = span_style(&spans, "both");
    assert!(has(both, Modifier::BOLD) && has(both, Modifier::ITALIC));
    assert!(has(span_style(&spans, "gone"), Modifier::CROSSED_OUT));
    assert_eq!(span_style(&spans, "x * y").fg, Some(theme().colors.primary));
}

#[test]
fn inline_nested_emphasis() {
    let spans = parse_inline("**bold *both* bold**", base(), &theme());
    let both = span_style(&spans, "both");
    assert!(has(both, Modifier::BOLD) && has(both, Modifier::ITALIC));
    assert!(!has(span_style(&spans, "bold "), Modifier::ITALIC));
}

#[test]
fn inline_leaves_non_emphasis_literal() {
    for text in [
        "snake_case_name and __init__ma",
        "2 * 3 * 4",
        "*args, **kwargs",
        "**unclosed",
        "~approx 5",
        "price \\*not italic\\*",
    ] {
        let spans = parse_inline(text, base(), &theme());
        let rendered: String = spans.iter().map(|s| s.content.as_ref()).collect();
        let expected = text.replace("\\*", "*");
        assert_eq!(rendered, expected, "{text}");
        assert!(
            spans.iter().all(|s| s.style == base()),
            "{text} got styled: {spans:?}"
        );
    }
}

#[test]
fn inline_emphasis_markers_inside_code_are_literal() {
    let spans = parse_inline("**use `a**b` here**", base(), &theme());
    let text: String = spans.iter().map(|s| s.content.as_ref()).collect();
    assert_eq!(text, "use a**b here");
    assert!(has(span_style(&spans, "use "), Modifier::BOLD));
}

#[test]
fn inline_links_show_label_and_url() {
    let spans = parse_inline(
        "see [the docs](https://example.com \"Docs\") or <https://a.io>",
        base(),
        &theme(),
    );
    let text: String = spans.iter().map(|s| s.content.as_ref()).collect();
    assert_eq!(text, "see the docs (https://example.com) or https://a.io");
    assert!(has(span_style(&spans, "the docs"), Modifier::UNDERLINED));
    assert!(has(
        span_style(&spans, "https://a.io"),
        Modifier::UNDERLINED
    ));
}

#[test]
fn inline_link_with_url_as_label_shows_url_once() {
    let spans = parse_inline("[https://x.dev](https://x.dev)", base(), &theme());
    let text: String = spans.iter().map(|s| s.content.as_ref()).collect();
    assert_eq!(text, "https://x.dev");
}

// ============================================================================
// Blocks
// ============================================================================

#[test]
fn headings_drop_hashes_and_are_bold() {
    let lines = render("# Title\n### Sub ###\n#hashtag", 40);
    assert_eq!(texts(&lines), ["Title", "Sub", "#hashtag"]);
    assert!(has(lines[0].spans[0].style, Modifier::BOLD));
    assert!(has(lines[0].spans[0].style, Modifier::UNDERLINED));
    assert!(has(lines[1].spans[0].style, Modifier::BOLD));
    assert!(!has(lines[2].spans[0].style, Modifier::BOLD));
}

#[test]
fn rules_span_the_width() {
    let lines = render("above\n---\n* * *", 10);
    assert_eq!(texts(&lines), ["above", "──────────", "──────────"]);
}

#[test]
fn blockquotes_get_a_bar_and_wrap_inside_it() {
    let lines = render("> quoted words here\n>> deeper", 12);
    assert_eq!(texts(&lines), ["│ quoted", "│ words here", "│ │ deeper"]);
    assert!(has(lines[0].spans[1].style, Modifier::ITALIC));
}

#[test]
fn blockquote_can_hold_a_list() {
    let lines = render("> - item", 20);
    assert_eq!(texts(&lines), ["│ • item"]);
}

#[test]
fn bullets_nest_and_wrapped_lines_hang_under_the_text() {
    let lines = render("- one two three\n  - nested\n* star\n+ plus", 10);
    assert_eq!(
        texts(&lines),
        ["• one two", "  three", "  ◦ nested", "• star", "• plus"]
    );
}

#[test]
fn ordered_lists_keep_their_numbers() {
    let lines = render("1. first item\n10) tenth", 9);
    assert_eq!(texts(&lines), ["1. first", "   item", "10) tenth"]);
}

#[test]
fn task_lists_show_checkboxes() {
    let lines = render("- [ ] todo\n- [x] done", 40);
    assert_eq!(texts(&lines), ["• [ ] todo", "• [✓] done"]);
    assert_eq!(lines[1].spans[1].style.fg, Some(theme().colors.success));
}

#[test]
fn list_lookalikes_are_paragraphs() {
    let lines = render("-5 degrees\n+1 from me", 40);
    assert_eq!(texts(&lines), ["-5 degrees", "+1 from me"]);
}

#[test]
fn wrapping_happens_after_parsing_so_markers_never_split() {
    let lines = render("aaaa **bold words that wrap** end", 12);
    assert_eq!(texts(&lines), ["aaaa bold", "words that", "wrap end"]);
    for line in &lines[..2] {
        assert!(line.spans.iter().any(|s| has(s.style, Modifier::BOLD)));
    }
}

#[test]
fn blank_lines_are_preserved() {
    let lines = render("a\n\nb", 20);
    assert_eq!(texts(&lines), ["a", "", "b"]);
    assert!(lines[1].spans.is_empty());
}

#[test]
fn indented_text_keeps_its_indent_when_wrapped() {
    let lines = render("   indented text here", 12);
    assert_eq!(texts(&lines), ["   indented", "   text here"]);
}

#[test]
fn zero_width_does_not_wrap() {
    let lines = render("- a long list item that would wrap", 0);
    assert_eq!(texts(&lines), ["• a long list item that would wrap"]);
}

// ============================================================================
// Tables
// ============================================================================

#[test]
fn table_renders_with_borders_and_alignment() {
    let md = "| Name | Qty |\n|:-----|----:|\n| `a` | 1 |\n| bb | 22 |";
    let lines = render(md, 40);
    assert_eq!(
        texts(&lines),
        [
            "┌──────┬─────┐",
            "│ Name │ Qty │",
            "├──────┼─────┤",
            "│ a    │   1 │",
            "│ bb   │  22 │",
            "└──────┴─────┘",
        ]
    );
    let header: Vec<_> = lines[1]
        .spans
        .iter()
        .filter(|s| s.content == "Name")
        .collect();
    assert!(has(header[0].style, Modifier::BOLD));
}

#[test]
fn table_without_outer_pipes_and_with_escaped_pipe() {
    let md = "a | b\n--- | :-:\nx \\| y | z";
    let lines = render(md, 40);
    assert_eq!(texts(&lines)[3], "│ x | y │ z │");
}

#[test]
fn wide_table_wraps_cells_to_fit() {
    let md = "| k | description |\n|---|---|\n| a | one two three four five six |";
    let lines = render(md, 20);
    for line in texts(&lines) {
        assert!(
            unicode_width::UnicodeWidthStr::width(line.as_str()) <= 20,
            "{line:?} overflows"
        );
    }
    let body: Vec<String> = texts(&lines)[3..lines.len() - 1].to_vec();
    assert!(body.len() > 1, "long cell should wrap: {body:?}");
    assert!(body[0].starts_with("│ a │ one"));
    assert!(body[1].starts_with("│   │ "));
}

#[test]
fn table_needs_a_separator_row() {
    let lines = render("a | b\nc | d", 40);
    assert_eq!(texts(&lines), ["a | b", "c | d"]);
}

#[test]
fn table_ends_at_first_non_row_line() {
    let lines = render("| a |\n|---|\n| 1 |\nafter", 40);
    assert_eq!(texts(&lines).last().map(String::as_str), Some("after"));
    assert_eq!(lines.len(), 6);
}

// ============================================================================
// Wrapping
// ============================================================================

#[test]
fn wrap_spans_splits_words_longer_than_a_line() {
    let spans = vec![Span::styled("abcdefghij", base())];
    let lines = wrap_spans(&spans, 4, &[], &[]);
    assert_eq!(texts(&lines), ["abcd", "efgh", "ij"]);
}

#[test]
fn wrap_spans_applies_prefixes() {
    let spans = vec![Span::raw("one two three")];
    let lines = wrap_spans(&spans, 7, &[Span::raw("> ")], &[Span::raw("  ")]);
    assert_eq!(texts(&lines), ["> one", "  two", "  three"]);
}
