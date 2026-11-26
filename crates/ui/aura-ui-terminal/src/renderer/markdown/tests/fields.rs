use super::*;

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
    let (lines, flow) = super::super::render_markdown_flow(md, 40, base(), &theme(), Measure::GRID);
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
