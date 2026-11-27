use super::*;
use ratatui::style::Modifier;

fn theme() -> Theme {
    Theme::cyber()
}

fn screen(width: u16, height: u16) -> Buffer {
    Buffer::empty(Rect::new(0, 0, width, height))
}

fn row(buf: &Buffer, y: u16) -> String {
    cells(buf, y, 0, buf.area.width)
}

/// Row `y` from column `from` up to `to`.
fn cells(buf: &Buffer, y: u16, from: u16, to: u16) -> String {
    (from..to).map(|x| buf[(x, y)].symbol()).collect()
}

/// Whether row `y` of `frame` holds nothing between its borders.
fn blank_inside(buf: &Buffer, y: u16, frame: Rect) -> bool {
    cells(buf, y, frame.x + 1, frame.right() - 1)
        .trim()
        .is_empty()
}

fn body(count: usize) -> Vec<Line<'static>> {
    (0..count)
        .map(|i| Line::from(format!("line {i}")))
        .collect()
}

fn draw(modal: Modal<'_>, buf: &mut Buffer, lines: Vec<Line<'_>>) -> ModalArea {
    let area = buf.area;
    modal.render(buf, area, lines)
}

#[test]
fn the_backdrop_empties_every_cell_and_darkens_it() {
    let theme = theme();
    let mut buf = screen(10, 3);
    buf.set_string(0, 1, "underneath", Style::default().fg(Color::Red));
    buf[(4, 2)].set_skip(true);
    let area = buf.area;
    backdrop(&mut buf, area, &theme);
    for cell in &buf.content {
        assert_eq!(cell.symbol(), " ");
        assert_eq!(cell.fg, Color::Reset);
        assert_eq!(cell.bg, theme.colors.backdrop);
        assert!(cell.modifier.is_empty());
        assert!(!cell.skip, "an image's cells are drawn over too");
    }
}

#[test]
fn a_dialog_is_centered_framed_and_padded() {
    let theme = theme();
    let mut buf = screen(80, 24);
    let area = draw(Modal::new("HELP", &theme), &mut buf, body(3));

    assert_eq!(area.frame, Rect::new(8, 8, 64, 7));
    assert_eq!(area.body, Rect::new(11, 10, 58, 3));
    assert!(
        cells(&buf, 8, 8, 72).starts_with("┌ HELP ─"),
        "{}",
        row(&buf, 8)
    );
    assert!(blank_inside(&buf, 9, area.frame), "a blank row on top");
    assert!(
        cells(&buf, 10, 8, 72).starts_with("│  line 0"),
        "{}",
        row(&buf, 10)
    );
    assert!(blank_inside(&buf, 13, area.frame), "and below");
    assert!(
        cells(&buf, 14, 8, 72).starts_with("└───"),
        "{}",
        row(&buf, 14)
    );

    let inside = &buf[(20, 12)];
    assert_eq!(inside.bg, theme.colors.background);
    let title = &buf[(10, 8)];
    assert_eq!(title.fg, theme.colors.primary);
    assert!(title.modifier.contains(Modifier::BOLD));
    assert_eq!(buf[(8, 8)].fg, theme.colors.muted);
    assert_eq!(buf[(0, 0)].bg, theme.colors.backdrop, "the rest is dark");
}

#[test]
fn meta_sits_at_the_right_end_of_the_top_border() {
    let theme = theme();
    let mut buf = screen(80, 24);
    let modal = Modal::new("APPROVAL REQUIRED", &theme)
        .meta("1 of 3")
        .tone(Tone::Attention);
    let area = draw(modal, &mut buf, body(1));
    let top = row(&buf, area.frame.y);
    assert!(top.contains("┌ APPROVAL REQUIRED ─"), "{top}");
    assert!(top.trim_end().ends_with("─ 1 of 3 ┐"), "{top}");
    assert_eq!(
        buf[(area.frame.x, area.frame.y)].fg,
        theme.colors.warning,
        "the agent waits on it"
    );
}

#[test]
fn a_narrow_screen_narrows_the_box_and_a_short_one_cuts_the_body() {
    let theme = theme();
    let modal = Modal::new("HELP", &theme);
    let small = Rect::new(0, 0, 30, 10);
    assert_eq!(modal.body_width(small), 20);
    assert_eq!(modal.body_room(small), 4);

    let mut buf = screen(30, 10);
    let area = draw(modal, &mut buf, body(9));
    assert_eq!(area.frame, Rect::new(2, 1, 26, 8));
    assert_eq!(area.body.height, 4);
    assert!(row(&buf, 6).contains("line 3"));
    assert!(!row(&buf, 7).contains("line 4"), "cut to the room");

    for (width, height) in [(0, 0), (1, 1), (3, 2), (7, 4)] {
        let mut buf = screen(width, height);
        draw(Modal::new("HELP", &theme), &mut buf, body(3));
    }
}

#[test]
fn a_status_shows_under_the_body_and_grows_the_box_downward() {
    let theme = theme();
    let mut buf = screen(80, 24);
    let bare = draw(Modal::new("FOLDER", &theme), &mut buf, body(3));

    let mut buf = screen(80, 24);
    let red = theme.colors.error;
    let modal = Modal::new("FOLDER", &theme).status("✗ Cannot open folder", red);
    assert_eq!(modal.body_room(buf.area), 24 - 2 - 4 - 2);
    let area = draw(modal, &mut buf, body(3));
    assert_eq!(area.frame.y, bare.frame.y, "the box keeps still");
    assert_eq!(area.frame.height, bare.frame.height + 2);
    let status = area.body.bottom() + 1;
    assert!(blank_inside(&buf, area.body.bottom(), area.frame));
    assert!(row(&buf, status).contains("│  ✗ Cannot open folder"));
    assert_eq!(buf[(area.body.x, status)].fg, red);
    assert!(row(&buf, area.frame.bottom() - 1).starts_with("        └"));
}

#[test]
fn a_long_status_wraps_to_three_rows() {
    let theme = theme();
    let words = "word ".repeat(60);
    let modal = Modal::new("FOLDER", &theme).status(words, theme.colors.error);
    let mut buf = screen(80, 24);
    let area = draw(modal, &mut buf, body(1));
    assert_eq!(area.frame.height, 2 + 2 + 1 + 1 + 3);
    let last = row(&buf, area.body.bottom() + 3);
    assert!(last.contains("word …"), "{last}");
}

#[test]
fn a_list_longer_than_it_shows_has_a_scrollbar_on_the_right_border() {
    let theme = theme();
    let scroll = ListScroll {
        top: 1,
        visible: 4,
        total: 40,
        first: 0,
    };
    let mut buf = screen(80, 24);
    let area = draw(
        Modal::new("MODELS", &theme).scroll(scroll),
        &mut buf,
        body(5),
    );
    let border = area.frame.right() - 1;
    let column: String = (area.body.y..area.body.bottom())
        .map(|y| buf[(border, y)].symbol())
        .collect();
    assert_eq!(column.chars().next(), Some('│'), "the row above the list");
    assert!(column.contains('┃'), "{column}");

    let fits = ListScroll { total: 4, ..scroll };
    let mut buf = screen(80, 24);
    let area = draw(Modal::new("MODELS", &theme).scroll(fits), &mut buf, body(5));
    let border = area.frame.right() - 1;
    assert!((area.body.y..area.body.bottom()).all(|y| buf[(border, y)].symbol() == "│"));
}

#[test]
fn a_selected_list_item_is_filled_across_its_width() {
    let theme = theme();
    let spans = || {
        vec![
            Span::styled("● ", Style::default().fg(theme.colors.success)),
            Span::raw("Model 00"),
        ]
    };
    let plain = list_item(spans(), false, 20, &theme);
    assert_eq!(plain.to_string(), "  ● Model 00");
    assert_eq!(plain.spans[1].style.fg, Some(theme.colors.success));

    let selected = list_item(spans(), true, 20, &theme);
    assert_eq!(selected.to_string(), "› ● Model 00        ");
    for span in &selected.spans {
        assert_eq!(span.style.bg, Some(theme.colors.selection));
        assert_eq!(span.style.fg, Some(theme.colors.on_selection));
    }
}

#[test]
fn the_caret_stays_in_view() {
    let theme = theme();
    let style = Style::default();
    let lead = || vec![Span::raw("    ")];
    let line = caret_line(lead(), "zode", 4, 20, style, &theme);
    assert_eq!(line.to_string(), "    zode█");
    let line = caret_line(lead(), "zode", 2, 20, style, &theme);
    assert_eq!(line.to_string(), "    zo█de");
    let line = caret_line(lead(), "abcdefghij", 10, 9, style, &theme);
    assert_eq!(line.to_string(), "    ghij█");
    let line = caret_line(lead(), "né", 2, 20, style, &theme);
    assert_eq!(line.to_string(), "    n█é", "a cursor inside a character");
}

#[test]
fn a_choice_line_names_its_key() {
    let line = choice_line('y', "Allow once", Color::Green);
    assert_eq!(line.to_string(), "[Y] Allow once");
    assert!(line.spans[0].style.add_modifier.contains(Modifier::BOLD));
    assert_eq!(line.spans[1].style.fg, Some(Color::Green));
}

#[test]
fn a_menu_frame_has_the_dialog_title_and_its_keys_below() {
    let theme = theme();
    let mut buf = screen(40, 4);
    let block = popup_block(Some("COMMANDS"), Some("Esc close"), &theme);
    ratatui::widgets::Widget::render(block, buf.area, &mut buf);
    assert!(row(&buf, 0).starts_with("┌ COMMANDS ─"), "{}", row(&buf, 0));
    assert!(row(&buf, 3).ends_with("─ Esc close ┘"), "{}", row(&buf, 3));
    assert_eq!(buf[(2, 0)].fg, theme.colors.primary);
    assert_eq!(buf[(30, 3)].fg, theme.colors.muted);
}
