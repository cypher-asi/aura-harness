use super::*;

fn text(line: &Line<'_>) -> String {
    line.spans
        .iter()
        .map(|span| span.content.as_ref())
        .collect()
}

fn span<'a>(line: &'a Line<'_>, content: &str) -> &'a Span<'a> {
    line.spans
        .iter()
        .find(|span| span.content.trim() == content)
        .unwrap_or_else(|| panic!("no {content:?} in {:?}", text(line)))
}

#[test]
fn a_section_title_is_gray_regular_and_flush_left() {
    let theme = Theme::cyber();
    let mut info = Info::new(40, &theme);
    info.section("STATUS", Some("2/12"));
    let lines = info.into_lines();
    assert_eq!(lines.len(), 1, "no blank row above the first section");
    assert_eq!(text(&lines[0]), "STATUS  2/12");
    let title = span(&lines[0], "STATUS");
    assert_eq!(title.style.fg, Some(theme.colors.secondary));
    assert!(!title.style.add_modifier.contains(Modifier::BOLD));
    assert_eq!(span(&lines[0], "2/12").style.fg, Some(theme.colors.muted));
}

#[test]
fn every_later_section_follows_one_blank_row() {
    let theme = Theme::cyber();
    let mut info = Info::new(40, &theme);
    info.title("boxd");
    info.section("MACHINE", None);
    info.fact("Region", "fra", Value::Plain);
    info.section("COST", None);
    let rows: Vec<String> = info.into_lines().iter().map(text).collect();
    assert_eq!(rows, ["boxd", "", "MACHINE", "  Region    fra", "", "COST"]);
}

#[test]
fn a_fact_pads_its_label_and_cuts_its_value() {
    let theme = Theme::cyber();
    let mut info = Info::new(20, &theme);
    info.fact("Region", "frankfurt-am-main", Value::Plain);
    info.fact("A long label", "x", Value::Plain);
    let lines = info.into_lines();
    assert_eq!(text(&lines[0]), "  Region    frankfu…");
    assert_eq!(text(&lines[1]), "  A long l… x", "a space after the label");
    assert_eq!(span(&lines[0], "Region").style.fg, Some(theme.colors.muted));
}

#[test]
fn a_wide_label_column_and_wrapped_values_keep_the_column() {
    let theme = Theme::cyber();
    let mut info = Info::new(24, &theme).label_width(14).wrap_values();
    info.fact("Context hash", &"ab".repeat(8), Value::Quiet);
    info.fact("Agent id", "", Value::Plain);
    let lines = info.into_lines();
    let rows: Vec<String> = lines.iter().map(text).collect();
    assert_eq!(
        rows,
        [
            "  Context hash  abababab",
            "                abababab",
            "  Agent id      —",
        ]
    );
    assert_eq!(span(&lines[2], "—").style.fg, Some(theme.colors.muted));
}

#[test]
fn values_are_plain_unless_flagged_quiet_or_failed() {
    let theme = Theme::cyber();
    let mut info = Info::new(40, &theme);
    for (value, kind) in [
        ("ask", Value::Plain),
        ("yolo", Value::Flag),
        ("0123abcd", Value::Quiet),
        ("denied", Value::Error),
    ] {
        info.fact("x", value, kind);
    }
    let colors: Vec<_> = info
        .into_lines()
        .iter()
        .map(|line| line.spans.last().and_then(|span| span.style.fg))
        .collect();
    let c = &theme.colors;
    assert_eq!(
        colors,
        [
            Some(c.foreground),
            Some(c.success),
            Some(c.muted),
            Some(c.error)
        ]
    );
    assert_eq!(Value::flag_if(true), Value::Flag);
    assert_eq!(Value::flag_if(false), Value::Plain);
}

#[test]
fn a_bar_fills_in_the_accent() {
    let theme = Theme::cyber();
    let mut info = Info::new(60, &theme);
    info.bar("Disk", 91, "91 GiB of 100 GiB", Value::Flag);
    let lines = info.into_lines();
    assert_eq!(text(&lines[0]), "  Disk      ██████████ 91 GiB of 100 GiB");
    assert_eq!(
        span(&lines[0], "██████████").style.fg,
        Some(theme.colors.success)
    );
    let mut info = Info::new(60, &theme);
    info.bar("CPU", 37, "37%", Value::Plain);
    let lines = info.into_lines();
    assert_eq!(text(&lines[0]), "  CPU       ████░░░░░░ 37%");
    assert_eq!(span(&lines[0], "░░░░░░").style.fg, Some(theme.colors.faint));
}

#[test]
fn a_marked_item_colors_only_its_glyph() {
    let theme = Theme::cyber();
    let mut info = Info::new(40, &theme);
    info.marked(("✓", theme.colors.success), ("read", 6), "on   rule");
    let lines = info.into_lines();
    assert_eq!(text(&lines[0]), "  ✓ read    on   rule");
    assert_eq!(span(&lines[0], "✓").style.fg, Some(theme.colors.success));
    assert_eq!(
        span(&lines[0], "read").style.fg,
        Some(theme.colors.foreground)
    );
    assert_eq!(
        span(&lines[0], "on   rule").style.fg,
        Some(theme.colors.muted)
    );
}

#[test]
fn items_and_prose_are_indented_and_lists_say_none() {
    let theme = Theme::cyber();
    let mut info = Info::new(16, &theme);
    info.items(&[]);
    info.item("Zode", Some("~/code/zode"));
    info.text("SpawnAgent, ControlAgent, ReadAgent", Value::Plain);
    let rows: Vec<String> = info.into_lines().iter().map(text).collect();
    assert_eq!(
        rows,
        [
            "  none",
            "  Zode  ~/code/…",
            "  SpawnAgent,",
            "  ControlAgent,",
            "  ReadAgent",
        ]
    );
}

#[test]
fn actions_follow_a_blank_row() {
    let theme = Theme::cyber();
    let mut info = Info::new(40, &theme);
    info.status("● ready");
    info.actions(&[('p', "pull changes"), ('s', "sleep")]);
    let lines = info.into_lines();
    assert_eq!(text(&lines[2]), "[p] pull changes  [s] sleep");
    assert_eq!(span(&lines[2], "[p]").style.fg, Some(theme.colors.primary));
    assert_eq!(span(&lines[2], "sleep").style.fg, Some(theme.colors.muted));
    assert_eq!(
        span(&lines[0], "● ready").style.fg,
        Some(theme.colors.success)
    );
}

#[test]
fn a_cut_line_keeps_its_spacing_and_loses_its_breaks() {
    assert_eq!(truncate_line("on   rule", 20), "on   rule");
    assert_eq!(truncate_line("one\ntwo  ", 20), "one two");
    assert_eq!(truncate_line("↑↓ scroll  ⇧←→ tabs", 12), "↑↓ scroll  …");
    assert_eq!(truncate_line("abc", 0), "");
}

#[test]
fn words_longer_than_a_line_are_broken_at_its_edge() {
    assert_eq!(wrap("abcdefgh ij", 3), ["abc", "def", "gh", "ij"]);
    assert_eq!(wrap("", 3), Vec::<String>::new());
}

#[test]
fn a_marked_section_colors_only_its_dot() {
    let theme = Theme::cyber();
    let mut info = Info::new(40, &theme);
    info.item("before", None);
    info.marked_section(StatusDot::Failed, "FAILED", Some("a1b2c3d"));
    let lines = info.into_lines();
    assert!(text(&lines[1]).is_empty(), "a blank row before it");
    assert_eq!(text(&lines[2]), "● FAILED  a1b2c3d");
    assert_eq!(span(&lines[2], "●").style.fg, Some(theme.colors.error));
    let title = span(&lines[2], "FAILED");
    assert_eq!(title.style.fg, Some(theme.colors.secondary));
    assert!(lines[2]
        .spans
        .iter()
        .all(|span| !span.style.add_modifier.contains(Modifier::BOLD)));
}

#[test]
fn a_link_is_an_indented_plain_line_whose_index_comes_back() {
    let theme = Theme::cyber();
    let mut info = Info::new(20, &theme);
    info.section("BRANCH", None);
    let at = info.link("aura/a-very-long-branch-name");
    let lines = info.into_lines();
    assert_eq!(at, 1);
    assert!(
        text(&lines[at]).starts_with("  aura/"),
        "{:?}",
        text(&lines[at])
    );
    assert!(text(&lines[at]).ends_with('…'));
    let link = &lines[at].spans[1];
    assert_eq!(link.style.fg, Some(theme.colors.foreground));
    assert!(link.style.add_modifier.contains(Modifier::UNDERLINED));
    assert!(!link.style.add_modifier.contains(Modifier::BOLD));
}
