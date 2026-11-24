use super::inline::parse_inline;
use super::render_markdown;
use super::wrap::wrap_spans;
use crate::renderer::text::Measure;
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
    let code = span_style(&spans, "x * y");
    assert_eq!(code.fg, Some(theme().colors.secondary));
    assert_eq!(code.bg, Some(theme().colors.surface));
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
    assert_eq!(texts(&lines), ["Title", "", "Sub", "", "#hashtag"]);
    assert!(has(lines[0].spans[0].style, Modifier::BOLD));
    assert!(has(lines[0].spans[0].style, Modifier::UNDERLINED));
    assert!(has(lines[2].spans[0].style, Modifier::BOLD));
    assert_eq!(lines[2].spans[0].style.fg, Some(theme().colors.foreground));
    assert!(!has(lines[4].spans[0].style, Modifier::BOLD));
}

#[test]
fn headings_get_one_blank_line_above_and_below() {
    let lines = render("intro\n## Next\nbody\n\n## Last", 40);
    assert_eq!(texts(&lines), ["intro", "", "Next", "", "body", "", "Last"]);
}

#[test]
fn a_list_is_parted_from_the_paragraphs_around_it() {
    let lines = render("Here is what changed:\n- one\n- two\nThat is all.", 40);
    assert_eq!(
        texts(&lines),
        [
            "Here is what changed:",
            "",
            "• one",
            "• two",
            "",
            "That is all."
        ]
    );
}

#[test]
fn blocks_already_parted_by_the_text_get_no_second_blank_line() {
    let lines = render("intro\n\n- one\n\n## Next\n\nbody", 40);
    assert_eq!(
        texts(&lines),
        ["intro", "", "• one", "", "Next", "", "body"]
    );
}

#[test]
fn indented_lines_after_a_list_item_continue_it() {
    let lines = render("- one\n  more of one\n- two", 40);
    assert_eq!(texts(&lines), ["• one", "  more of one", "", "• two"]);
}

#[test]
fn short_list_items_stay_tight() {
    let lines = render("- one\n- two\n- three", 40);
    assert_eq!(texts(&lines), ["• one", "• two", "• three"]);
}

#[test]
fn items_that_wrap_are_parted_from_the_next_item() {
    let md = "1. **First.** a point long enough to wrap\n\
              2. **Second.** another point that wraps too\n\
              3. short";
    let (lines, flow) = super::render_markdown_flow(md, 24, base(), &theme(), Measure::GRID);
    let texts = texts(&lines);
    let blanks: Vec<usize> = (0..texts.len()).filter(|&i| texts[i].is_empty()).collect();
    let starts: Vec<usize> = (0..texts.len())
        .filter(|&i| texts[i].starts_with(char::is_numeric))
        .collect();
    assert_eq!(starts.len(), 3, "{texts:?}");
    assert_eq!(blanks, [starts[1] - 1, starts[2] - 1], "{texts:?}");
    assert!(blanks.iter().all(|&i| flow[i] == Some(0)));
}

#[test]
fn an_item_after_a_short_one_gets_no_gap_even_when_it_wraps() {
    let lines = render("- a\n- a point long enough to wrap\n- b", 16);
    assert_eq!(
        texts(&lines),
        ["• a", "• a point long", "  enough to wrap", "", "• b"]
    );
}

#[test]
fn loose_lists_get_no_second_gap() {
    let lines = render("- a point long enough to wrap\n\n- b", 16);
    assert_eq!(
        texts(&lines),
        ["• a point long", "  enough to wrap", "", "• b"]
    );
}

#[test]
fn nested_items_stay_tight_under_their_parent() {
    let lines = render("- top\n  - x\n  - y\n- next", 40);
    assert_eq!(texts(&lines), ["• top", "  ◦ x", "  ◦ y", "", "• next"]);
}

#[test]
fn quotes_and_tables_are_parted_from_text() {
    let lines = render("said:\n> quoted\nthen\n| a |\n|---|\n| 1 |", 40);
    let texts = texts(&lines);
    assert_eq!(texts[..5], ["said:", "", "│ quoted", "", "then"]);
    assert_eq!(texts[5], "");
    assert!(texts[6].starts_with('┌'), "{texts:?}");
}

#[test]
fn rules_part_blocks_without_blank_lines() {
    let lines = render("above\n---\n- below", 10);
    assert_eq!(texts(&lines), ["above", "──────────", "• below"]);
    let lines = render("---\n# Next", 10);
    assert_eq!(texts(&lines), ["──────────", "", "Next"], "a heading still");
}

#[test]
fn blank_lines_between_blocks_are_prose() {
    let (lines, flow) =
        super::render_markdown_flow("text\n- item", 40, base(), &theme(), Measure::GRID);
    assert_eq!(texts(&lines), ["text", "", "• item"]);
    assert_eq!(flow, [Some(0), Some(0), Some(2)]);
}

#[test]
fn bold_body_text_is_brighter_than_the_body() {
    let theme = theme();
    let body = Style::default().fg(theme.colors.text);
    let spans = parse_inline("plain **strong** [**link**](https://x.dev)", body, &theme);
    assert_eq!(span_style(&spans, "plain ").fg, Some(theme.colors.text));
    assert_eq!(
        span_style(&spans, "strong").fg,
        Some(theme.colors.foreground)
    );
    assert_eq!(span_style(&spans, "link").fg, Some(theme.colors.warning));
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
        ["• one two", "  three", "  ◦ nested", "", "• star", "• plus"]
    );
}

#[test]
fn ordered_lists_keep_their_numbers() {
    let lines = render("1. first item\n10) tenth", 9);
    assert_eq!(texts(&lines), ["1. first", "   item", "", "10) tenth"]);
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
// Lists of fields
// ============================================================================

/// The style of the first span of `line` that holds `text`. The muted
/// bullet and a muted label are one span, so labels are found inside one.
fn style_containing(line: &Line<'_>, text: &str) -> Style {
    line.spans
        .iter()
        .find(|s| s.content.contains(text))
        .unwrap_or_else(|| panic!("no span with {text:?} in {line:?}"))
        .style
}

#[test]
fn a_list_of_fields_written_loose_is_drawn_tight() {
    let lines = render("- Path: `a.py`\n\n- Name: `f`\n\n- Type: function", 40);
    assert_eq!(
        texts(&lines),
        ["• Path: a.py", "• Name: f", "• Type: function"]
    );
}

#[test]
fn records_are_parted_where_the_first_label_comes_back() {
    let md = "## New Interfaces\n- Path: `m.py`\n\n- Name: `f`\n\n- Type: function\n\n\
              - Path: `m.py`\n\n- Name: `C`\n\n- Type: class";
    let lines = render(md, 40);
    assert_eq!(
        texts(&lines),
        [
            "New Interfaces",
            "",
            "• Path: m.py",
            "• Name: f",
            "• Type: function",
            "",
            "• Path: m.py",
            "• Name: C",
            "• Type: class",
        ]
    );
}

#[test]
fn records_written_tight_get_a_blank_row_between_them() {
    let md = "- File: a.rs\n- Change: added x\n- File: b.rs\n- Change: removed y";
    let (lines, flow) = super::render_markdown_flow(md, 40, base(), &theme(), Measure::GRID);
    assert_eq!(
        texts(&lines),
        [
            "• File: a.rs",
            "• Change: added x",
            "",
            "• File: b.rs",
            "• Change: removed y"
        ]
    );
    assert_eq!(flow, [Some(2), Some(2), Some(0), Some(2), Some(2)]);
}

#[test]
fn a_label_on_every_item_does_not_make_records() {
    let lines = render("- Note: a\n\n- Note: b\n\n- Note: c", 40);
    assert_eq!(texts(&lines), ["• Note: a", "• Note: b", "• Note: c"]);
}

#[test]
fn an_item_that_is_not_a_field_keeps_the_blank_lines_around_it() {
    let lines = render("- Path: a\n\n- just text\n\n- Name: b", 40);
    assert_eq!(
        texts(&lines),
        ["• Path: a", "", "• just text", "", "• Name: b"]
    );
    assert_eq!(
        style_containing(&lines[0], "Path:").fg,
        Some(theme().colors.muted)
    );
    let lines = render("- intro\n\n- A: 1\n\n- B: 2", 40);
    assert_eq!(texts(&lines), ["• intro", "", "• A: 1", "• B: 2"]);
}

#[test]
fn fields_above_an_item_still_being_written_stay_put() {
    let lines = render("- Path: a\n\n- Name: f\n\n- Ty", 40);
    assert_eq!(texts(&lines), ["• Path: a", "• Name: f", "", "• Ty"]);
    let lines = render("- Path: a\n- Name: f\n- Ty", 40);
    for (row, label) in [(0, "Path:"), (1, "Name:")] {
        assert_eq!(
            style_containing(&lines[row], label).fg,
            Some(theme().colors.muted),
            "{label}"
        );
    }
}

#[test]
fn a_record_is_parted_as_soon_as_its_first_field_arrives() {
    let lines = render("- Path: a\n- Name: f\n- Path: b", 40);
    assert_eq!(texts(&lines), ["• Path: a", "• Name: f", "", "• Path: b"]);
}

#[test]
fn field_labels_are_muted_and_values_keep_their_style() {
    let theme = theme();
    let lines = render("- Path: `a.py`\n- Type: function", 40);
    assert_eq!(
        style_containing(&lines[0], "Path:").fg,
        Some(theme.colors.muted)
    );
    let code = parse_inline("`a.py`", base(), &theme);
    assert_eq!(span_style(&lines[0].spans, "a.py"), code[0].style);
    assert_eq!(
        style_containing(&lines[1], "function").fg,
        Some(Color::White)
    );
}

#[test]
fn bold_labels_keep_their_emphasis() {
    let lines = render("- **Path:** a\n- **Name:** b", 40);
    assert_eq!(texts(&lines), ["• Path: a", "• Name: b"]);
    let label = style_containing(&lines[0], "Path:");
    assert!(has(label, Modifier::BOLD));
    assert_ne!(label.fg, Some(theme().colors.muted));
}

#[test]
fn colons_in_urls_times_and_sentences_do_not_make_labels() {
    let lines = render("- https://x.dev\n\n- 10:30 standup", 40);
    assert_eq!(texts(&lines), ["• https://x.dev", "", "• 10:30 standup"]);
    let lines = render("- the fix was simple and quick: one line\n\n- b: c", 60);
    assert_eq!(
        texts(&lines),
        ["• the fix was simple and quick: one line", "", "• b: c"]
    );
}

#[test]
fn a_field_keeps_the_lines_that_continue_it() {
    let lines = render("- Path: a\n  more of a\n\n- Name: b", 40);
    assert_eq!(texts(&lines), ["• Path: a", "  more of a", "• Name: b"]);
}

#[test]
fn blank_lines_around_a_list_of_fields_stay() {
    let lines = render("intro\n\n- A: 1\n\n- B: 2\n\nafter", 40);
    assert_eq!(
        texts(&lines),
        ["intro", "", "• A: 1", "• B: 2", "", "after"]
    );
}

#[test]
fn numbered_fields_are_drawn_tight_too() {
    let lines = render("1. Path: a\n\n2. Name: b", 40);
    assert_eq!(texts(&lines), ["1. Path: a", "2. Name: b"]);
}

#[test]
fn task_items_are_not_fields() {
    let lines = render("- [ ] Path: a\n\n- [ ] Name: b", 40);
    assert_eq!(texts(&lines), ["• [ ] Path: a", "", "• [ ] Name: b"]);
}

#[test]
fn a_long_value_wraps_under_the_label() {
    let lines = render(
        "- Name: f\n\n- Description: returns the uuid of the device",
        24,
    );
    assert_eq!(
        texts(&lines),
        [
            "• Name: f",
            "• Description: returns",
            "  the uuid of the device"
        ]
    );
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
    assert_eq!(
        texts(&lines)[5],
        "",
        "a blank line parts the table from the text"
    );
    assert_eq!(lines.len(), 7);
}

// ============================================================================
// Wrapping
// ============================================================================

#[test]
fn wrap_spans_splits_words_longer_than_a_line() {
    let spans = vec![Span::styled("abcdefghij", base())];
    let lines = wrap_spans(&spans, 4, (&[], &[]), Measure::GRID);
    assert_eq!(texts(&lines), ["abcd", "efgh", "ij"]);
}

#[test]
fn wrap_spans_applies_prefixes() {
    let spans = vec![Span::raw("one two three")];
    let lines = wrap_spans(
        &spans,
        7,
        (&[Span::raw("> ")], &[Span::raw("  ")]),
        Measure::GRID,
    );
    assert_eq!(texts(&lines), ["> one", "  two", "  three"]);
}
