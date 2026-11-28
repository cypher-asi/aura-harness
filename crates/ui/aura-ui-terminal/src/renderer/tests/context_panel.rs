//! The context ring in the footer and the CONTEXT panel it opens.

use super::*;
use crate::events::{CallStats, ContextView, UiCommand};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEventKind};

/// An agent whose last call counted 50k of a 200k window, its parts as
/// the loop measured them.
fn app_with_context() -> crate::App {
    let mut app = crate::App::new();
    app.process_command(UiCommand::CallStats(CallStats {
        model: "aura-claude-sonnet-4-6".to_string(),
        input_tokens: 10_000,
        cache_read_tokens: 40_000,
        output_tokens: 900,
        context_limit: Some(200_000),
        ..CallStats::default()
    }));
    app.process_command(UiCommand::SetContext(ContextView {
        system_tokens: 3_000,
        tools_tokens: 8_000,
        mcp_tokens: 1_000,
        subagents_tokens: 500,
        skills_tokens: 0,
        conversation_tokens: 30_000,
        total_tokens: 42_500,
        limit_tokens: Some(200_000),
    }));
    app
}

/// The rows of the screen with `needle`, joined, for messages.
fn rows_with<'a>(rows: &'a [String], needle: &str) -> Vec<&'a String> {
    rows.iter().filter(|row| row.contains(needle)).collect()
}

#[test]
fn test_the_footer_shows_a_ring_that_fills_with_the_window() {
    let mut app = app_with_context();
    let rows = render_rows(&mut app, 170);
    let footer = rows_with(&rows, "context used");
    assert!(
        footer.iter().any(|row| row.contains("◔ 25% context used")),
        "{}",
        rows.join("\n")
    );
    let ring = app.context_ring_area().expect("the ring's area");
    let row = &rows[usize::from(ring.y)];
    let at = row.chars().take(usize::from(ring.x)).count();
    assert!(
        row.chars()
            .skip(at)
            .collect::<String>()
            .starts_with("◔ 25% context used"),
        "the area starts at the ring: {row:?}"
    );
}

#[test]
fn test_clicking_the_ring_opens_the_panel_with_every_part() {
    let mut app = app_with_context();
    render_rows(&mut app, 170);
    let ring = app.context_ring_area().expect("the ring's area");
    mouse_at(
        &mut app,
        MouseEventKind::Down(MouseButton::Left),
        (ring.x, ring.y),
    );
    assert_eq!(app.dialog(), Some(crate::app::Dialog::Context));

    let rows = render_rows(&mut app, 170);
    let screen = rows.join("\n");
    assert!(screen.contains("CONTEXT"), "{screen}");
    assert!(screen.contains("◔ 25% of 200k"), "the meta line: {screen}");
    assert!(
        screen.contains("50.0k of 200k tokens, as the model counted them"),
        "{screen}"
    );
    for (part, tokens, share) in [
        ("System prompt", "3.0k", "1.5%"),
        ("Tools", "8.0k", "4.0%"),
        ("MCP tools", "1.0k", "0.5%"),
        ("Subagents", "500", "0.2%"),
        // The rest of the model's count: 50k less the parts above.
        ("Messages", "37.5k", "18.7%"),
        ("Free space", "150k", "75.0%"),
    ] {
        let row = rows_with(&rows, part);
        let row = row
            .first()
            .unwrap_or_else(|| panic!("no {part}:\n{screen}"));
        assert!(
            row.contains(tokens) && row.contains(share),
            "{part}: {row:?}"
        );
    }
    assert!(
        !screen.contains("Skills"),
        "a part with nothing is left out"
    );

    // Esc closes it.
    app.handle_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::empty()));
    assert_eq!(app.dialog(), None);
}

#[test]
fn test_the_panels_bar_is_coloured_by_part_and_free_space_is_faint() {
    let mut app = app_with_context();
    run_slash(&mut app, "/context");
    let buffer = render_buffer(&mut app, 170);
    let theme = test_theme();
    // The bar: the row of block cells under USED.
    let bar_row = (0..buffer.area.height)
        .find(|&y| {
            (0..buffer.area.width).any(|x| buffer[(x, y)].symbol() == "█")
                && (0..buffer.area.width).any(|x| buffer[(x, y)].symbol() == "░")
        })
        .expect("the bar");
    let cells: Vec<_> = (0..buffer.area.width)
        .map(|x| &buffer[(x, bar_row)])
        .filter(|cell| matches!(cell.symbol(), "█" | "░"))
        .collect();
    let free = cells.iter().filter(|c| c.symbol() == "░").count();
    let messages = cells
        .iter()
        .filter(|c| c.fg == theme.colors.success)
        .count();
    assert!(
        free * 100 / cells.len() >= 70,
        "free is three quarters: {free}/{}",
        cells.len()
    );
    assert!(messages > 0, "messages have their colour");
    assert!(cells
        .iter()
        .filter(|c| c.symbol() == "░")
        .all(|c| c.fg == theme.colors.faint));
}

#[test]
fn test_before_any_call_the_panel_says_nothing_filled_it_yet() {
    let mut app = crate::App::new();
    run_slash(&mut app, "/context");
    let screen = render_rows(&mut app, 140).join("\n");
    assert!(screen.contains("CONTEXT"), "{screen}");
    assert!(screen.contains("Nothing yet"), "{screen}");
    assert_eq!(app.context_ring_area(), None, "no ring without a count");
}
