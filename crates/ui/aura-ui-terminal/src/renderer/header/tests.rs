use super::*;
use ratatui::{backend::TestBackend, buffer::Buffer, style::Modifier, Terminal};

const TABS: [&str; 2] = ["CHAT", "TERMINAL"];

fn draw<T: Copy + PartialEq>(width: u16, height: u16, header: &PanelHeader<'_, T>) -> Buffer {
    let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
    terminal
        .draw(|frame| {
            let (bar, _) = split(frame.area(), header.hosted);
            render_panel_header(frame, bar, header, &Theme::cyber());
        })
        .unwrap();
    terminal.backend().buffer().clone()
}

fn row(buffer: &Buffer, y: u16) -> String {
    (0..buffer.area.width)
        .map(|x| buffer[(x, y)].symbol())
        .collect()
}

fn chat_header(width: u16, focused: bool, hovered: Option<usize>) -> PanelHeader<'static, usize> {
    PanelHeader {
        layout: TabLayout::new([0, 1], |i| TABS[i], |i| TABS[i], width),
        active: Some(0),
        hovered,
        title: None,
        info: None,
        close: None,
        expand: None,
        scope: None,
        icon_tab: None,
        focused,
        hosted: false,
    }
}

#[test]
fn tabs_sit_between_dividers_that_join_the_frame() {
    let buffer = draw(40, 10, &chat_header(40, true, None));
    assert_eq!(row(&buffer, 0), "┌───────────────────┬──────────────────┐");
    assert_eq!(row(&buffer, 1), "│       CHAT        │     TERMINAL     │");
    assert_eq!(row(&buffer, 2), "└───────────────────┴──────────────────┘");
}

#[test]
fn the_active_tab_is_filled_with_inverted_text() {
    let colors = Theme::cyber().colors;
    let focused = draw(40, 10, &chat_header(40, true, Some(1)));
    let active = focused[(1, 1)].style();
    assert_eq!(active.bg, Some(colors.selection));
    assert_eq!(active.fg, Some(colors.on_selection));
    let hovered = focused[(25, 1)].style();
    assert_eq!(hovered.bg, Some(colors.surface_hover));
    assert_eq!(hovered.fg, Some(colors.secondary));

    let unfocused = draw(40, 10, &chat_header(40, false, None));
    let active = unfocused[(1, 1)].style();
    assert_eq!(active.bg, Some(Color::Reset));
    assert_eq!(active.fg, Some(colors.foreground));
    assert!(active.add_modifier.contains(Modifier::BOLD));
    let idle = unfocused[(25, 1)].style();
    assert_eq!(idle.bg, Some(Color::Reset));
    assert_eq!(idle.fg, Some(colors.muted));
    assert_eq!(unfocused[(0, 0)].style().fg, Some(colors.faint));
    assert_eq!(focused[(0, 0)].style().fg, Some(colors.success));
}

#[test]
fn short_panels_get_the_tab_row_only() {
    let buffer = draw(40, 5, &chat_header(40, true, None));
    assert_eq!(row(&buffer, 0), "│       CHAT        │     TERMINAL     │");
    assert_eq!(split(Rect::new(0, 0, 40, 5), false).1.y, 1);
}

#[test]
fn hosted_bars_keep_only_their_frame_for_the_front_end() {
    let header = PanelHeader {
        hosted: true,
        ..chat_header(20, true, None)
    };
    let buffer = draw(20, 10, &header);
    assert_eq!(row(&buffer, 0), "┌──────────────────┐");
    assert_eq!(row(&buffer, 1), "│                  │");
    assert_eq!(split(Rect::new(0, 0, 20, 10), true).1.y, 2);
    assert_eq!(split(Rect::new(0, 0, 20, 5), true).1.y, 1);
}

#[test]
fn hosted_tabbed_bars_tell_the_front_end_their_tabs() {
    let area = Rect::new(4, 1, 30, 12);
    let bar = Rect::new(4, 1, 30, 2);
    let tabs = PanelHeader {
        layout: ChatTab::layout(30),
        active: Some(ChatTab::Terminal),
        hovered: None,
        title: None,
        info: None,
        close: None,
        expand: None,
        scope: None,
        icon_tab: None,
        focused: true,
        hosted: true,
    };
    let chrome = tabs.chrome(Pane::Chat, area, bar);
    assert_eq!(
        chrome.tabs,
        [
            PaneTab::Chat(ChatTab::Chat),
            PaneTab::Chat(ChatTab::Terminal),
            PaneTab::Chat(ChatTab::Graph),
            PaneTab::Chat(ChatTab::Time),
            PaneTab::Chat(ChatTab::Log)
        ]
    );
    assert_eq!(chrome.active, Some(PaneTab::Chat(ChatTab::Terminal)));
    assert_eq!((chrome.area, chrome.bar), (area, bar));
    assert!(chrome.focused);
}

#[test]
fn hosted_titled_bars_tell_the_front_end_their_title_and_info() {
    let area = Rect::new(4, 1, 30, 12);
    let bar = Rect::new(4, 1, 30, 2);
    let titled = PanelHeader::<()> {
        layout: TabLayout::untabbed(30),
        active: None,
        hovered: None,
        title: Some("KERNEL"),
        info: Some(HeaderInfo::count(128, "RECORDS", Color::Gray)),
        close: None,
        expand: None,
        scope: None,
        icon_tab: None,
        focused: false,
        hosted: true,
    };
    let chrome = titled.chrome(Pane::Records, area, bar);
    assert!(!chrome.closable);
    assert!(
        PanelHeader {
            close: Some(false),
            ..titled
        }
        .chrome(Pane::Chat, area, bar)
        .closable
    );
    assert!(chrome.tabs.is_empty());
    assert_eq!(chrome.title.as_deref(), Some("KERNEL"));
    assert_eq!(chrome.info.as_deref(), Some("128 RECORDS"));
    assert_eq!(chrome.info_short.as_deref(), Some("128"));
    assert_eq!(chrome.info_color, Color::Gray);
}

#[test]
fn untabbed_bars_show_the_title_and_shorten_the_info() {
    let header = |width| PanelHeader::<()> {
        layout: TabLayout::untabbed(width),
        active: None,
        hovered: None,
        title: Some("KERNEL"),
        info: Some(HeaderInfo::count(128, "RECORDS", Color::Gray)),
        close: None,
        expand: None,
        scope: None,
        icon_tab: None,
        focused: false,
        hosted: false,
    };
    let wide = draw(30, 10, &header(30));
    assert_eq!(row(&wide, 1), "│ KERNEL         128 RECORDS │");
    assert_eq!(wide[(2, 1)].style().bg, Some(Color::Reset));
    assert_eq!(wide[(10, 1)].style().bg, Some(Color::Reset));
    let narrow = draw(16, 10, &header(16));
    assert_eq!(row(&narrow, 1), "│ KERNEL   128 │");
    let tiny = draw(10, 10, &header(10));
    assert_eq!(row(&tiny, 1), "│ KERNEL │");
}

#[test]
fn tabs_with_an_info_cell_keep_the_count_on_the_right() {
    const LABELS: [&str; 2] = ["KERNEL", "MEMBERS"];
    let header = |width| PanelHeader {
        layout: TabLayout::with_info([0, 1], |i| LABELS[i], |i| &LABELS[i][..3], width),
        active: Some(0),
        hovered: None,
        title: None,
        info: Some(HeaderInfo::count(128, "RECORDS", Color::Gray)),
        close: None,
        expand: None,
        scope: None,
        icon_tab: None,
        focused: false,
        hosted: false,
    };
    let wide = draw(40, 10, &header(40));
    assert_eq!(row(&wide, 0), "┌─────────┬─────────┬──────────────────┐");
    assert_eq!(row(&wide, 1), "│ KERNEL  │ MEMBERS │      128 RECORDS │");
    assert_eq!(row(&wide, 2), "└─────────┴─────────┴──────────────────┘");
    let narrow = draw(28, 10, &header(28));
    assert_eq!(row(&narrow, 1), "│ KERNEL  │ MEMBERS │  128 │");
    let tight = draw(23, 10, &header(23));
    assert_eq!(
        row(&tight, 1),
        "│  KERNEL  │ MEMBERS  │",
        "no room left for it"
    );
}

#[test]
fn closable_bars_end_in_a_close_button_lit_under_the_pointer() {
    let header = |hovered| PanelHeader::<()> {
        layout: TabLayout::untabbed(30),
        active: None,
        hovered: None,
        title: Some("RECORD #60"),
        info: Some(HeaderInfo::count(128, "LINES", Color::Gray)),
        close: Some(hovered),
        expand: None,
        scope: None,
        icon_tab: None,
        focused: true,
        hosted: false,
    };
    let idle = draw(30, 10, &header(false));
    assert_eq!(row(&idle, 1), "│ RECORD #60     128 LINES ✕ │");
    assert_eq!(
        close_cell(Rect::new(0, 0, 30, 3)),
        Some(Rect::new(26, 1, 3, 1))
    );
    let colors = Theme::cyber().colors;
    assert_eq!(idle[(27, 1)].style().fg, Some(colors.muted));
    let lit = draw(30, 10, &header(true));
    assert_eq!(lit[(27, 1)].style().bg, Some(colors.surface_hover));
}

#[test]
fn tabbed_bars_end_in_an_expand_button_of_their_own() {
    let bar = Rect::new(0, 0, 40, 3);
    let tabs = tabs_beside_expand(bar);
    assert_eq!(tabs, 36);
    let header = |expanded, hovered| PanelHeader {
        expand: Some(ExpandButton {
            expanded,
            hovered,
            cell: expand_cell(bar, false),
        }),
        ..chat_header(tabs, true, None)
    };
    let idle = draw(40, 10, &header(false, false));
    assert_eq!(row(&idle, 0), "┌─────────────────┬────────────────┬───┐");
    assert_eq!(row(&idle, 1), "│      CHAT       │    TERMINAL    │ ⤢ │");
    assert_eq!(row(&idle, 2), "└─────────────────┴────────────────┴───┘");
    assert_eq!(expand_cell(bar, false), Some(Rect::new(36, 1, 3, 1)));
    let colors = Theme::cyber().colors;
    assert_eq!(idle[(37, 1)].style().fg, Some(colors.muted));

    let lit = draw(40, 10, &header(true, true));
    assert_eq!(lit[(37, 1)].symbol(), "⤡", "expanded, it restores");
    assert_eq!(lit[(36, 1)].style().bg, Some(colors.surface_hover));

    assert_eq!(tabs_beside_expand(Rect::new(0, 0, 16, 3)), 16);
    assert_eq!(expand_cell(Rect::new(0, 0, 16, 3), false), None);
}

/// Where the side panel's bar `width` columns wide puts its buttons,
/// HOST's icon among them.
fn side_end(width: u16, (scope, expand): (bool, bool)) -> BarEnd {
    let icon = SidePanelTab::Host.icon();
    bar_end(Rect::new(0, 0, width, 3), (icon, scope, expand), |width| {
        SidePanelTab::layout(width, false)
    })
}

/// The side panel's header bar `width` columns wide, ending in HOST's
/// icon and the buttons asked for.
fn side_header(
    width: u16,
    buttons: (bool, bool),
    (on, active): (bool, SidePanelTab),
) -> PanelHeader<'static, SidePanelTab> {
    let end = side_end(width, buttons);
    PanelHeader {
        layout: SidePanelTab::layout(end.tabs, false),
        active: Some(active),
        hovered: None,
        title: None,
        info: None,
        close: None,
        expand: buttons.1.then_some(ExpandButton {
            expanded: false,
            hovered: false,
            cell: end.expand,
        }),
        scope: buttons.0.then_some(ScopeButton {
            on,
            hovered: false,
            cell: end.scope,
        }),
        icon_tab: Some(IconTab {
            tab: SidePanelTab::Host,
            icon: SidePanelTab::Host.icon().unwrap(),
            tooltip: SidePanelTab::Host.tooltip(),
            cell: end.icon,
        }),
        focused: false,
        hosted: false,
    }
}

/// [`side_header`] drawn on a screen ten rows tall.
fn draw_side(width: u16, buttons: (bool, bool), state: (bool, SidePanelTab)) -> Buffer {
    draw(width, 10, &side_header(width, buttons, state))
}

#[test]
fn scoped_bars_end_in_a_star_button_lit_while_on() {
    let end = side_end(35, (true, false));
    assert_eq!(end.tabs, 26);
    assert_eq!(end.icon, Some(Rect::new(26, 1, 4, 1)));
    assert_eq!(end.scope, Some(Rect::new(31, 1, 3, 1)));
    assert_eq!(end.expand, None);
    let off = draw_side(35, (true, false), (false, SidePanelTab::Sandbox));
    assert_eq!(row(&off, 0), "┌────┬────┬────┬────┬────┬────┬───┐");
    assert_eq!(row(&off, 1), "│SBX │REPO│PRC │TRC │TSK │ 🖥  │ * │");
    assert_eq!(row(&off, 2), "└────┴────┴────┴────┴────┴────┴───┘");
    let colors = Theme::cyber().colors;
    assert_eq!(off[(32, 1)].style().fg, Some(colors.muted));

    let on = draw_side(35, (true, false), (true, SidePanelTab::Sandbox));
    assert_eq!(on[(32, 1)].style().fg, Some(colors.foreground));
    assert!(on[(32, 1)].style().add_modifier.contains(Modifier::BOLD));

    let narrow = side_end(16, (true, false));
    assert_eq!((narrow.tabs, narrow.icon, narrow.scope), (16, None, None));
}

#[test]
fn hosts_icon_is_a_tab_lit_while_shown_and_centered() {
    let colors = Theme::cyber().colors;
    let off = draw_side(35, (true, false), (false, SidePanelTab::Sandbox));
    assert_eq!(off[(27, 1)].symbol(), "🖥");
    assert_eq!(off[(27, 1)].style().fg, Some(colors.muted));
    let shown = draw_side(35, (true, false), (false, SidePanelTab::Host));
    for x in 26..30 {
        let fg = shown[(x, 1)].style().fg;
        assert_eq!(fg, Some(colors.foreground), "column {x}");
    }
    assert!(shown[(27, 1)].style().add_modifier.contains(Modifier::BOLD));
    assert_eq!(
        shown[(6, 1)].style().fg,
        Some(colors.muted),
        "the REPO label is not lit"
    );
}

#[test]
fn the_expand_button_sits_right_of_the_star_button() {
    let end = side_end(44, (true, true));
    assert_eq!(end.tabs, 31);
    assert_eq!(end.icon, Some(Rect::new(31, 1, 4, 1)));
    assert_eq!(end.scope, Some(Rect::new(36, 1, 3, 1)));
    assert_eq!(end.expand, Some(Rect::new(40, 1, 3, 1)));
    let buffer = draw_side(44, (true, true), (false, SidePanelTab::Sandbox));
    assert_eq!(
        row(&buffer, 0),
        "┌─────┬─────┬─────┬─────┬─────┬────┬───┬───┐"
    );
    assert_eq!(
        row(&buffer, 1),
        "│ SBX │REPO │ PRC │TRACE│TASKS│ 🖥  │ * │ ⤢ │"
    );
    assert_eq!(
        row(&buffer, 2),
        "└─────┴─────┴─────┴─────┴─────┴────┴───┴───┘"
    );
}

#[test]
fn narrow_bars_tighten_their_buttons_then_drop_the_expand_button() {
    // Five short labels and their dividers need 21 columns: 34 leaves
    // them that beside HOST's icon and two buttons, padded, 33 does
    // not. Tight ones leave them room down to 28; at 27 the expand
    // button goes, and under 20 there are no buttons.
    let end = side_end(34, (true, true));
    assert_eq!(end.tabs, 21);
    assert_eq!(end.icon, Some(Rect::new(21, 1, 4, 1)));
    assert_eq!(end.scope, Some(Rect::new(26, 1, 3, 1)));
    assert_eq!(end.expand, Some(Rect::new(30, 1, 3, 1)));
    let header = |width| side_header(width, (true, true), (false, SidePanelTab::Sandbox));
    let buffer = draw(34, 10, &header(34));
    assert_eq!(row(&buffer, 1), "│SBX│RPO│PRC│TRC│TSK│ 🖥  │ * │ ⤢ │");

    let end = side_end(33, (true, true));
    assert_eq!(end.tabs, 26);
    assert_eq!(end.icon, Some(Rect::new(26, 1, 2, 1)));
    assert_eq!(end.scope, Some(Rect::new(29, 1, 1, 1)));
    assert_eq!(end.expand, Some(Rect::new(31, 1, 1, 1)));
    let buffer = draw(33, 10, &header(33));
    assert_eq!(row(&buffer, 0), "┌────┬────┬────┬────┬────┬──┬─┬─┐");
    assert_eq!(row(&buffer, 1), "│SBX │REPO│PRC │TRC │TSK │🖥 │*│⤢│");
    assert_eq!(row(&buffer, 2), "└────┴────┴────┴────┴────┴──┴─┴─┘");

    let end = side_end(28, (true, true));
    assert_eq!(end.tabs, 21);
    assert_eq!(end.expand, Some(Rect::new(26, 1, 1, 1)));

    let end = side_end(27, (true, true));
    assert_eq!(end.expand, None, "the expand button goes first");
    assert_eq!((end.tabs, end.scope), (22, Some(Rect::new(25, 1, 1, 1))));
    assert_eq!(end.icon, Some(Rect::new(22, 1, 2, 1)));

    let bare = side_end(19, (true, true));
    assert_eq!(
        (bare.tabs, bare.icon, bare.scope, bare.expand),
        (19, None, None, None)
    );
}

#[test]
fn tabs_too_many_for_a_tight_button_keep_a_padded_one() {
    // Five short labels and their dividers need 21 columns, more than
    // a 21-column bar leaves beside even one tight button.
    const SHORT: [&str; 5] = ["ONE", "TWO", "SIX", "TEN", "ART"];
    let five = |width| TabLayout::new([0, 1, 2, 3, 4], |i| SHORT[i], |i| SHORT[i], width);
    let clipped = bar_end(Rect::new(0, 0, 21, 3), (None, true, true), five);
    assert_eq!(clipped.expand, None);
    assert_eq!(
        (clipped.tabs, clipped.scope),
        (17, Some(Rect::new(17, 1, 3, 1))),
        "tabs that fit beside nothing keep a padded button"
    );
}

#[test]
fn the_expand_button_sits_left_of_a_close_button() {
    let header = PanelHeader::<()> {
        layout: TabLayout::untabbed(30),
        active: None,
        hovered: None,
        title: Some("RECORD #60"),
        info: Some(HeaderInfo::count(128, "LINES", Color::Gray)),
        close: Some(false),
        expand: Some(ExpandButton {
            expanded: false,
            hovered: false,
            cell: expand_cell(Rect::new(0, 0, 30, 3), true),
        }),
        scope: None,
        icon_tab: None,
        focused: true,
        hosted: false,
    };
    let buffer = draw(30, 10, &header);
    assert_eq!(row(&buffer, 1), "│ RECORD #60   128 LINES ⤢ ✕ │");
    assert_eq!(
        expand_cell(Rect::new(0, 0, 30, 3), true),
        Some(Rect::new(24, 1, 2, 1))
    );
}
