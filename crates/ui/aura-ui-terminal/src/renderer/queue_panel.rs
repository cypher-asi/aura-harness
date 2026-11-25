//! The queued-messages panel, between the Chat body and the input, in
//! the same column of borders: the Chat body's bottom border is the line
//! above it, its own bottom border the line above the input.
//!
//! ```text
//! ├──────────────────────────────────────────────┤
//! │ 2 Queued                                   — │
//! │ first queued message text, cut to fit…       │
//! │ second queued    Send Now ⏎  ✎  ✕            │
//! ├──────────────────────────────────────────────┤
//! │ → Add a follow-up                            │
//! ```
//!
//! The header gives the count and a toggle, `—` to fold the panel to the
//! header, `+` to open it. Each entry is one row; the row under the
//! pointer, or the one the keyboard is on, is lit and shows its actions.
//! Only messages have `✎`. Past [`MAX_VISIBLE_ROWS`] entries a `+N more`
//! row counts the rest, and the rows on screen follow the keyboard. While
//! an entry is dragged its row stays lit and a rail marks where it lands.
//!
//! The panel records where it drew its header, rows and actions on the
//! [`App`] for the mouse.

use super::hud::truncate;
use super::input::join_divider;
use super::panels::{border_color, DROP_RAIL};
use super::text::display_width;
use crate::app::queue_panel::{QueueAreas, QueuePart, QueueRowArea, QueueSpot, MAX_VISIBLE_ROWS};
use crate::app::PanelFocus;
use crate::{App, Theme};
use ratatui::{
    layout::Rect,
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, Paragraph},
    Frame,
};

/// Rows the border adds: only the bottom one, since the Chat body's
/// bottom border sits on top.
const BORDER_ROWS: u16 = 1;
/// The send-now action's label.
const SEND_NOW: &str = "Send Now ⏎";
/// The edit action's glyph.
const EDIT: &str = "✎";
/// The remove action's glyph.
const REMOVE: &str = "✕";
/// Columns from the send-now label's start to the row's end: the label,
/// two spaces, `✎`, two spaces, `✕` and the closing pad.
const ACTIONS_WIDTH: u16 = 17;
/// Narrowest text the row keeps beside its actions; narrower rows leave
/// the actions to the keyboard.
const MIN_TEXT_WIDTH: u16 = 8;

/// Rows the panel wants: none for an empty queue, the header alone when
/// folded, else the header, the entries on screen and a `+N more` row
/// when some are hidden.
#[must_use]
pub(super) fn height(app: &App) -> u16 {
    let len = app.queue().len();
    // With no channel open the chat shows no agent's queue.
    if len == 0 || app.channel_app_empty() {
        return 0;
    }
    if app.queue_collapsed() {
        return BORDER_ROWS + 1;
    }
    let shown = len.min(MAX_VISIBLE_ROWS);
    let more = usize::from(len > shown);
    u16::try_from(shown + more)
        .unwrap_or(u16::MAX)
        .saturating_add(BORDER_ROWS + 1)
}

/// How the entry at one row is drawn.
#[derive(Clone, Copy)]
struct RowLook {
    selected: bool,
    lit: bool,
    drop_target: bool,
    hovered_part: Option<QueuePart>,
}

/// Render the panel into `area`, which [`height`] sized, and record where
/// its parts went.
pub(super) fn render(frame: &mut Frame, area: Rect, app: &mut App, theme: &Theme) {
    let len = app.queue().len();
    if len == 0 || app.channel_app_empty() || area.height < BORDER_ROWS + 1 || area.width < 6 {
        app.set_queue_areas(QueueAreas::default());
        return;
    }
    let block = Block::default()
        .borders(Borders::LEFT | Borders::RIGHT | Borders::BOTTOM)
        .border_style(Style::default().fg(border_color(app.focus() == PanelFocus::Chat, theme)));
    let inner = block.inner(area);
    frame.render_widget(Clear, area);
    frame.render_widget(block, area);
    join_divider(frame, area);

    let header = Rect { height: 1, ..inner };
    frame.render_widget(Paragraph::new(header_line(app, inner.width, theme)), header);
    let mut areas = QueueAreas {
        header: Some(header),
        rows: Vec::new(),
    };

    let room = usize::from(inner.height.saturating_sub(1));
    if !app.queue_collapsed() && room > 0 {
        let mut shown = len.min(MAX_VISIBLE_ROWS).min(room);
        if len > shown && shown == room && shown > 1 {
            // Keep a row to say how many are hidden.
            shown -= 1;
        }
        let first = app.queue_first_visible(shown);
        let mut y = header.y + 1;
        for index in first..(first + shown).min(len) {
            let row = Rect { y, ..header };
            if let Some(drawn) = render_row(frame, row, index, app, theme) {
                areas.rows.push(drawn);
            }
            y += 1;
        }
        let hidden = len.saturating_sub(areas.rows.len());
        if hidden > 0 && y < inner.bottom() {
            let more = Line::from(Span::styled(
                format!(" +{hidden} more"),
                Style::default().fg(theme.colors.muted),
            ));
            frame.render_widget(Paragraph::new(more), Rect { y, ..header });
        }
    }
    app.set_queue_areas(areas);
}

/// ` 2 Queued` at the left, with the keys while the panel has the
/// keyboard, and the toggle at the right.
fn header_line(app: &App, width: u16, theme: &Theme) -> Line<'static> {
    let count = format!(" {} Queued", app.queue().len());
    let toggle = if app.queue_collapsed() { "+" } else { "—" };
    let toggle_color = if app.queue_hover() == Some(QueueSpot::Header) {
        theme.colors.foreground
    } else {
        theme.colors.muted
    };
    let hint = if app.queue_selected().is_some() {
        "  Enter edit · Ctrl+Enter send · Del remove · Esc back"
    } else {
        ""
    };
    let width = usize::from(width);
    let fixed = display_width(&count) + display_width(toggle) + 1;
    let hint = if fixed + display_width(hint) < width {
        hint
    } else {
        ""
    };
    let fill = width.saturating_sub(fixed + display_width(hint));
    Line::from(vec![
        Span::styled(
            count,
            Style::default()
                .fg(theme.colors.foreground)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(hint, Style::default().fg(theme.colors.muted)),
        Span::raw(" ".repeat(fill)),
        Span::styled(toggle, Style::default().fg(toggle_color)),
        Span::raw(" "),
    ])
}

fn row_look(app: &App, index: usize, id: u64) -> RowLook {
    let drag = app.queue_drag();
    let hovered_part = match app.queue_hover() {
        Some(QueueSpot::Row { id: at, part }) if at == id => Some(part),
        _ => None,
    };
    RowLook {
        selected: app.queue_selected() == Some(index) || app.queue_editing_index() == Some(index),
        lit: hovered_part.is_some() || drag.is_some_and(|d| d.from == index),
        drop_target: drag.is_some_and(|d| d.to == index && d.from != index),
        hovered_part,
    }
}

/// Draw entry `index` on `row` and return where its parts went: the
/// actions only when they are drawn, so a press where they would be is a
/// press on the row's text. `None` when there is no such entry.
fn render_row(
    frame: &mut Frame,
    row: Rect,
    index: usize,
    app: &App,
    theme: &Theme,
) -> Option<QueueRowArea> {
    let item = app.queue().get(index)?;
    let look = row_look(app, index, item.id);
    let colors = &theme.colors;
    let (bg, text_fg) = if look.selected {
        (Some(colors.selection), colors.on_selection)
    } else if look.lit {
        (Some(colors.surface_hover), colors.foreground)
    } else if item.editable {
        (None, colors.text)
    } else {
        (None, colors.muted)
    };
    let base = bg.map_or_else(Style::default, |bg| Style::default().bg(bg));

    let has_actions = row.width >= ACTIONS_WIDTH + MIN_TEXT_WIDTH + 2;
    let show_actions = has_actions && (look.selected || look.lit);
    let right = row.x + row.width;
    let area = QueueRowArea {
        id: item.id,
        row,
        send_now: show_actions.then(|| Rect {
            x: right - ACTIONS_WIDTH,
            width: 10,
            ..row
        }),
        edit: (show_actions && item.editable).then(|| Rect {
            x: right - 6,
            width: 3,
            ..row
        }),
        remove: show_actions.then(|| Rect {
            x: right - 3,
            width: 3,
            ..row
        }),
    };

    let tail = if show_actions { ACTIONS_WIDTH + 1 } else { 1 };
    let text_width = usize::from(row.width.saturating_sub(1 + tail));
    let text = truncate(&item.text.replace('\n', "↵"), text_width);
    let pad = text_width.saturating_sub(display_width(&text));
    let lead = if look.drop_target {
        Span::styled(DROP_RAIL, base.fg(colors.success))
    } else {
        Span::styled(" ", base)
    };
    let mut spans = vec![
        lead,
        Span::styled(text, base.fg(text_fg)),
        Span::styled(" ".repeat(pad), base),
    ];
    if show_actions {
        let action = |part: QueuePart| {
            let color = if look.hovered_part == Some(part) {
                colors.foreground
            } else if look.selected {
                colors.on_selection
            } else {
                colors.muted
            };
            base.fg(color)
        };
        let edit = if item.editable { EDIT } else { " " };
        spans.extend([
            Span::styled(" ", base),
            Span::styled(SEND_NOW, action(QueuePart::SendNow)),
            Span::styled("  ", base),
            Span::styled(edit, action(QueuePart::Edit)),
            Span::styled("  ", base),
            Span::styled(REMOVE, action(QueuePart::Remove)),
        ]);
    }
    spans.push(Span::styled(" ", base));
    frame.render_widget(Paragraph::new(Line::from(spans)), row);
    Some(area)
}

/// The input box's title while it holds a queued message being edited.
#[must_use]
pub(super) fn editing_title(color: Color) -> Line<'static> {
    Line::from(Span::styled(
        " Editing queued message · Esc to cancel ",
        Style::default().fg(color),
    ))
}

#[cfg(test)]
mod tests {
    use crate::events::{QueuedItemView, UiCommand};
    use crate::{App, Theme};
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseEvent, MouseEventKind};

    fn queue_of(texts: &[&str]) -> UiCommand {
        UiCommand::SetQueue(
            texts
                .iter()
                .zip(1..)
                .map(|(text, id)| QueuedItemView {
                    id,
                    text: (*text).to_string(),
                    editable: !text.starts_with('/'),
                })
                .collect(),
        )
    }

    fn rows(app: &mut App) -> Vec<String> {
        let mut terminal =
            ratatui::Terminal::new(ratatui::backend::TestBackend::new(160, 30)).unwrap();
        terminal
            .draw(|frame| super::super::render(frame, app, &Theme::cyber()))
            .unwrap();
        let buffer = terminal.backend().buffer();
        (0..buffer.area.height)
            .map(|y| {
                (0..buffer.area.width)
                    .map(|x| buffer[(x, y)].symbol())
                    .collect()
            })
            .collect()
    }

    fn row_with<'a>(rows: &'a [String], needle: &str) -> Option<(usize, &'a String)> {
        rows.iter()
            .enumerate()
            .find(|(_, row)| row.contains(needle))
    }

    #[test]
    fn the_panel_shows_the_count_and_the_entries_above_the_input() {
        let mut app = App::new();
        app.process_command(queue_of(&["first message", "second message", "/compact"]));
        let rows = rows(&mut app);

        let (header, header_row) = row_with(&rows, "3 Queued").expect("header");
        assert!(header_row.contains(" — │"), "{header_row:?}");
        let (first, _) = row_with(&rows, "first message").expect("first row");
        let (second, _) = row_with(&rows, "second message").expect("second row");
        let (third, _) = row_with(&rows, "/compact").expect("third row");
        assert_eq!([first, second, third], [header + 1, header + 2, header + 3]);
        let (prompt, _) = row_with(&rows, "→ ").expect("the input");
        assert!(prompt > third, "the panel sits above the input");
        assert!(rows[header - 1].contains('├'), "{:?}", rows[header - 1]);
        assert!(
            !rows.iter().any(|row| row.contains("Send Now")),
            "actions wait for the pointer"
        );
    }

    #[test]
    fn no_panel_without_queued_work() {
        let mut app = App::new();
        assert!(row_with(&rows(&mut app), "Queued").is_none());
        app.process_command(queue_of(&["one"]));
        assert!(row_with(&rows(&mut app), "1 Queued").is_some());
        app.process_command(queue_of(&[]));
        assert!(row_with(&rows(&mut app), "Queued").is_none());
    }

    #[test]
    fn a_folded_panel_shows_only_its_header() {
        let mut app = App::new();
        app.process_command(queue_of(&["first message"]));
        app.toggle_queue_collapsed();
        let rows = rows(&mut app);

        let (_, header) = row_with(&rows, "1 Queued").expect("header");
        assert!(header.contains(" + │"), "{header:?}");
        assert!(row_with(&rows, "first message").is_none());
    }

    #[test]
    fn long_queues_show_five_rows_and_count_the_rest() {
        let mut app = App::new();
        app.process_command(queue_of(&["m1", "m2", "m3", "m4", "m5", "m6", "m7"]));
        let rows = rows(&mut app);

        assert!(row_with(&rows, "7 Queued").is_some());
        assert!(row_with(&rows, " m5 ").is_some());
        assert!(row_with(&rows, " m6 ").is_none());
        assert!(row_with(&rows, "+2 more").is_some());
    }

    #[test]
    fn the_hovered_row_shows_its_actions_and_long_text_is_cut() {
        let mut app = App::new();
        let long = "word ".repeat(40);
        app.process_command(queue_of(&[&long, "/compact"]));
        let _ = rows(&mut app);
        let area = app.queue_areas().rows[0].row;
        let row = usize::from(area.y);
        app.handle_mouse(MouseEvent {
            kind: MouseEventKind::Moved,
            column: area.x + 2,
            row: area.y,
            modifiers: KeyModifiers::empty(),
        });
        let rows = rows(&mut app);

        let shown = &rows[row];
        assert!(shown.contains("…"), "{shown:?}");
        assert!(shown.contains("Send Now ⏎  ✎  ✕"), "{shown:?}");
        let (_, label) = row_with(&rows, "/compact").expect("label row");
        assert!(!label.contains("Send Now"), "{label:?}");
    }

    #[test]
    fn the_keyboard_row_shows_actions_without_a_pencil_for_labels() {
        let mut app = App::new();
        app.process_command(queue_of(&["message", "/compact"]));
        app.handle_key(KeyEvent::new(KeyCode::Up, KeyModifiers::SHIFT));
        let rows = rows(&mut app);

        let (_, label) = row_with(&rows, "/compact").expect("label row");
        assert!(label.contains("Send Now ⏎     ✕"), "{label:?}");
        assert!(
            row_with(&rows, "Esc back").is_some(),
            "the header lists the keys"
        );
    }

    #[test]
    fn the_input_says_when_it_holds_a_queued_message() {
        let mut app = App::new();
        app.process_command(queue_of(&["message"]));
        app.start_queue_edit(1);
        let rows = rows(&mut app);
        assert!(row_with(&rows, "Editing queued message · Esc to cancel").is_some());
    }

    #[test]
    fn actions_are_clickable_only_while_they_are_drawn() {
        let mut app = App::new();
        app.process_command(queue_of(&["first message", "second message"]));
        let _ = rows(&mut app);
        let hidden = app.queue_areas().rows[0];
        assert_eq!(hidden.id, 1, "rows are named by entry id");
        assert_eq!(
            (hidden.send_now, hidden.edit, hidden.remove),
            (None, None, None),
            "no hit zones for actions not on screen"
        );

        app.handle_mouse(MouseEvent {
            kind: MouseEventKind::Moved,
            column: hidden.row.x + 2,
            row: hidden.row.y,
            modifiers: KeyModifiers::empty(),
        });
        let _ = rows(&mut app);
        let shown = app.queue_areas().rows[0];
        assert!(shown.send_now.is_some() && shown.edit.is_some() && shown.remove.is_some());
        let other = app.queue_areas().rows[1];
        assert!(other.send_now.is_none(), "only the hovered row has actions");
    }
}
