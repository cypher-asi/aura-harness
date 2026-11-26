use super::*;

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
