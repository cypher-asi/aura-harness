use super::*;

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
    assert_eq!(texts(&lines), ["• one", "  more of one", "• two"]);
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
        super::super::render_markdown_flow("text\n- item", 40, base(), &theme(), Measure::GRID);
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
    assert_eq!(span_style(&spans, "link").fg, Some(theme.colors.muted));
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
