use super::*;
use crate::app::HoverTarget;
use crossterm::event::{MouseButton, MouseEventKind};

#[test]
fn test_placement_switch_lights_under_the_pointer_and_turns_once_clicked() {
    let mut app = app_with_model(None);
    let (tx, _rx) = tokio::sync::mpsc::channel(16);
    app.link.event_tx = Some(tx);
    let colors = test_theme().colors;
    let buffer = render_buffer(&mut app, 120);
    let footer = buffer.area.height - 1;
    let (house, idle) = find_cell(&buffer, footer, "⌂ local").expect("the switch ends the footer");
    assert_eq!(idle.fg, Some(colors.muted));
    assert_ne!(idle.bg, Some(colors.surface_hover));

    // Lit like a button, a column either side of `⌂ local`.
    mouse_at(&mut app, MouseEventKind::Moved, (house, footer));
    assert_eq!(app.hover(), Some(HoverTarget::PlacementSwitch));
    let buffer = render_buffer(&mut app, 120);
    for x in house - 1..=house + 7 {
        let bg = buffer[(x, footer)].style().bg;
        assert_eq!(bg, Some(colors.surface_hover), "column {x}");
    }
    assert_ne!(
        buffer[(house - 2, footer)].style().bg,
        Some(colors.surface_hover)
    );
    assert_eq!(buffer[(house, footer)].style().fg, Some(colors.foreground));

    // The click shows the move on the next frame, unlit, and it turns.
    mouse_at(
        &mut app,
        MouseEventKind::Down(MouseButton::Left),
        (house, footer),
    );
    let buffer = render_buffer(&mut app, 120);
    let (glyph, moving) =
        find_cell(&buffer, footer, "◐ moving to cloud").expect("the move shows at once");
    assert_eq!(moving.fg, Some(colors.success));
    assert_ne!(moving.bg, Some(colors.surface_hover));
    for _ in 0..3 {
        app.tick();
    }
    let buffer = render_buffer(&mut app, 120);
    assert_eq!(buffer[(glyph, footer)].symbol(), "◓", "a quarter turn on");
}
