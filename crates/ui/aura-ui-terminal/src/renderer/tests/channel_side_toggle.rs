use super::channels::{channel_info_app, channels_app, info_row};
use super::*;
use crossterm::event::{KeyModifiers, MouseButton, MouseEvent, MouseEventKind};

/// A mouse event of `kind` on the header's hamburger button, drawn at
/// `cell`.
fn on_toggle(cell: Rect, kind: MouseEventKind) -> MouseEvent {
    MouseEvent {
        kind,
        column: cell.x + 1,
        row: cell.y,
        modifiers: KeyModifiers::empty(),
    }
}

#[test]
fn test_header_hamburger_hides_and_restores_the_channel_side_panel() {
    use crate::app::{HoverTarget, PanelFocus};

    let mut app = channel_info_app(None);
    // The agent's own layout, which the channel's button leaves alone.
    app.panels.record_visible = true;
    app.panels.sandbox_visible = false;
    app.panels.side_column_hidden = Some((true, false));
    app.focus = PanelFocus::Sandbox;
    let rows = render_rows(&mut app, 140);
    assert!(rows[0].trim_end().ends_with('☰'), "{:?}", rows[0]);
    info_row(&rows, "COORDINATOR");
    let side = app.areas.sandbox.expect("the channel's panel");
    let chat = app.areas.chat.expect("the chat").width;
    let cell = app.side_toggle_area().expect("drawn");
    let click = on_toggle(cell, MouseEventKind::Down(MouseButton::Left));
    app.handle_mouse(on_toggle(cell, MouseEventKind::Moved));
    assert_eq!(app.hover(), Some(HoverTarget::SideToggle));

    app.handle_mouse(click);
    assert!(!app.side_column_shown());
    assert_eq!(app.focus(), PanelFocus::Chat, "off the hidden panel");
    let hidden = render_rows(&mut app, 140);
    let screen = hidden.join("\n");
    assert!(!screen.contains("COORDINATOR"), "{screen}");
    assert_eq!(app.areas.sandbox, None);
    assert!(
        app.areas.chat.expect("the chat").width >= chat + side.width,
        "the chat takes the panel's room"
    );
    assert_eq!(app.side_toggle_area(), Some(cell), "the button stays put");

    app.handle_mouse(click);
    assert!(
        app.sandbox_panel_visible(),
        "the channel's panel comes back"
    );
    let shown = render_rows(&mut app, 140);
    info_row(&shown, "COORDINATOR");
    assert_eq!(app.areas.sandbox, Some(side));
    assert_eq!(app.areas.chat.map(|chat| chat.width), Some(chat));
    assert_eq!(
        (
            app.panels.record_visible,
            app.panels.sandbox_visible,
            app.panels.side_column_hidden
        ),
        (true, false, Some((true, false))),
        "the agent's layout is untouched"
    );
}

#[test]
fn test_header_hamburger_in_a_channel_restores_an_expanded_chat() {
    let mut app = channel_info_app(None);
    app.toggle_chat_expanded();
    render_rows(&mut app, 140);
    assert!(!app.side_column_shown());
    let cell = app.side_toggle_area().expect("drawn");
    app.handle_mouse(on_toggle(cell, MouseEventKind::Down(MouseButton::Left)));
    assert!(!app.chat_expanded());
    assert!(app.sandbox_panel_visible(), "the channel's panel shows");
}

#[test]
fn test_a_channel_panel_the_hamburger_hid_leaves_no_member_rows_to_hover() {
    use crate::app::{HoverTarget, SidePanelTab};
    let moved = |column: usize, row: usize| MouseEvent {
        kind: MouseEventKind::Moved,
        column: u16::try_from(column).unwrap(),
        row: u16::try_from(row).unwrap(),
        modifiers: KeyModifiers::empty(),
    };

    let mut app = channels_app();
    assert_eq!(app.side_tab(), SidePanelTab::ChannelMembers);
    let rows = render_rows(&mut app, 140);
    let (row, column) = rows
        .iter()
        .enumerate()
        .find_map(|(y, text)| {
            text.find("subagent")
                .map(|x| (y, text[..x].chars().count()))
        })
        .unwrap_or_else(|| panic!("no member row in:\n{}", rows.join("\n")));
    app.handle_mouse(moved(column, row));
    assert_eq!(app.hover(), Some(HoverTarget::ChannelMemberRow(1)));
    app.handle_mouse(moved(column, row + 1));
    assert_eq!(
        app.hover(),
        Some(HoverTarget::ChannelMemberRow(1)),
        "its status row is the same member's"
    );

    app.toggle_side_column();
    let hidden = render_rows(&mut app, 140);
    assert!(
        !hidden.join("\n").contains("subagent"),
        "{}",
        hidden.join("\n")
    );
    app.handle_mouse(moved(column, row));
    assert!(
        !matches!(app.hover(), Some(HoverTarget::ChannelMemberRow(_))),
        "{:?}",
        app.hover()
    );
}
