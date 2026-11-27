//! BENCH's wrench at the right end of the agents panel's header bar.

use super::*;
use crate::app::{AgentsTab, HoverTarget};
use crossterm::event::{MouseButton, MouseEventKind};

/// The cell the wrench is drawn in on the bar's labels row: its column
/// and row.
fn wrench_cell(buffer: &ratatui::buffer::Buffer) -> (u16, u16) {
    let row = row_of(buffer, "🔧");
    let (column, _) = find_cell(buffer, row, "🔧").unwrap();
    (column, row)
}

#[test]
fn the_agents_bar_ends_in_benchs_wrench_instead_of_its_label() {
    let mut app = crate::App::new();
    for width in [120, 160] {
        let rows = render_rows(&mut app, width);
        let tabs = rows
            .iter()
            .position(|row| row.contains("🔧"))
            .unwrap_or_else(|| panic!("no wrench in:\n{}", rows.join("\n")));
        let bar = &rows[tabs];
        assert!(!bar.contains("BENCH") && !bar.contains("BEN "), "{bar}");
        let agents_end = bar.find("ARC").or_else(|| bar.find("ARCHIVE")).unwrap();
        let wrench = bar.find("🔧").unwrap();
        assert!(agents_end < wrench, "the wrench comes after ARCHIVE: {bar}");
        // Nothing but its second column, its cell's padding and the frame
        // between the wrench and the panel's right border.
        let after: String = bar[wrench + "🔧".len()..].chars().take(3).collect();
        assert_eq!(after, "  │", "{bar}");
        assert!(rows.iter().all(|row| !row.contains("BENCH")), "{rows:#?}");
    }
}

#[test]
fn clicking_the_wrench_opens_bench_and_lights_it() {
    let mut app = crate::App::new();
    let buffer = render_buffer(&mut app, 120);
    let (column, row) = wrench_cell(&buffer);
    assert_ne!(app.agents_tab(), AgentsTab::Bench);
    mouse_at(
        &mut app,
        MouseEventKind::Down(MouseButton::Left),
        (column, row),
    );
    mouse_at(
        &mut app,
        MouseEventKind::Up(MouseButton::Left),
        (column, row),
    );
    assert_eq!(app.agents_tab(), AgentsTab::Bench);

    let buffer = render_buffer(&mut app, 120);
    let (column, row) = wrench_cell(&buffer);
    let lit = buffer[(column, row)].style();
    assert!(lit.add_modifier.contains(Modifier::BOLD), "{lit:?}");
    assert_eq!(
        lit.bg,
        Some(test_theme().colors.selection),
        "focused and shown"
    );
}

#[test]
fn the_wrench_is_named_bench_while_the_pointer_is_over_it() {
    let mut app = crate::App::new();
    let buffer = render_buffer(&mut app, 120);
    assert!(
        (0..buffer.area.height).all(|y| find_cell(&buffer, y, "Bench").is_none()),
        "no name until hovered"
    );
    let (column, row) = wrench_cell(&buffer);
    mouse_at(&mut app, MouseEventKind::Moved, (column, row));
    assert_eq!(app.hover(), Some(HoverTarget::AgentTab(AgentsTab::Bench)));

    let buffer = render_buffer(&mut app, 120);
    let (column, row) = wrench_cell(&buffer);
    let colors = test_theme().colors;
    assert_eq!(buffer[(column, row)].style().bg, Some(colors.surface_hover));
    // Under the bar, its right end under the wrench's cell.
    let (name_at, name) = find_cell(&buffer, row + 2, "Bench").expect("the tooltip");
    assert_eq!(name.bg, Some(colors.surface_hover));
    assert_eq!(name.fg, Some(colors.foreground));
    assert!(
        name_at <= column && column < name_at + 7,
        "{name_at} {column}"
    );
}
