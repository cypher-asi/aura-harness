//! The model selector: a Ready / Sign-in tab row, the filter, and the
//! models on the tab grouped by route.

use super::super::text::centered_rect;
use crate::{app::ModelPicker, events::ModelOptionView, App, Theme};
use ratatui::{
    layout::Rect,
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, Paragraph},
    Frame,
};

/// Rows of the model selector that are not the list: the tab row, the
/// filter row, the blank lines around the list, the key hints and the
/// two borders.
const MODEL_SELECTOR_CHROME_ROWS: usize = 7;

/// A row of the list with the index of its model (headings and
/// separators have none), so the window can follow the cursor.
type ListRow<'a> = (Line<'a>, Option<usize>);

/// Render the model selector: a Ready / Sign-in tab row, the filter, and
/// the models on the tab grouped by route, scrolled to keep the cursor in
/// view.
pub(super) fn render_model_selector(frame: &mut Frame, app: &mut App, theme: &Theme) {
    let picker = app.model_picker();
    let models = picker.visible(app.models());
    let muted = Style::default().fg(theme.colors.muted);

    let tabs = tab_row(app, picker, theme);
    let filter = filter_line(picker, theme);
    let rows = list_rows(app, picker, &models, theme);

    let area = frame.area();
    let modal_width = 64.min(area.width.saturating_sub(4));
    let max_height = usize::from(area.height.saturating_sub(2));
    let room = max_height
        .saturating_sub(MODEL_SELECTOR_CHROME_ROWS)
        .max(1)
        .min(rows.len());

    let mut lines = vec![tabs, filter, Line::from("")];
    // The line of the selector each model row is on, with its index.
    let hits = push_window(&mut lines, rows, room, picker.cursor(), muted);
    lines.push(Line::from(""));
    let hints = if modal_width >= 62 {
        "↑/↓ move  Tab switch  type to filter  Enter select  Esc close"
    } else {
        "↑/↓ Tab type Enter Esc"
    };
    lines.push(Line::from(Span::styled(hints, muted)));

    let modal_height = u16::try_from(lines.len() + 2)
        .unwrap_or(u16::MAX)
        .min(area.height.saturating_sub(2));
    let modal_area = centered_rect(modal_width, modal_height, area);
    frame.render_widget(Clear, modal_area);

    let block = Block::default()
        .title(selector_title(picker))
        .borders(Borders::ALL)
        .border_style(muted);
    frame.render_widget(Paragraph::new(lines).block(block), modal_area);

    let rows = clickable_rows(hits, modal_area, modal_height);
    app.set_model_picker_layout(crate::app::PickerLayout {
        modal: modal_area,
        rows,
    });
}

/// `Ready (3)  Sign in (2)`, the tab shown underlined.
fn tab_row<'a>(app: &App, picker: &ModelPicker, theme: &Theme) -> Line<'a> {
    let muted = Style::default().fg(theme.colors.muted);
    let (ready, sign_in) = picker.counts(app.models());
    let tab = |label: &str, count: usize, active: bool| {
        let style = if active {
            Style::default()
                .fg(theme.colors.primary)
                .add_modifier(Modifier::BOLD | Modifier::UNDERLINED)
        } else {
            muted
        };
        Span::styled(format!("{label} ({count})"), style)
    };
    let showing_ready = picker.tab() == crate::app::ModelTab::Ready;
    Line::from(vec![
        tab("Ready", ready, showing_ready),
        Span::raw("  "),
        tab("Sign in", sign_in, !showing_ready),
    ])
}

/// The filter typed so far, or how to start one.
fn filter_line<'a>(picker: &ModelPicker, theme: &Theme) -> Line<'a> {
    if picker.filter().is_empty() {
        Line::from(Span::styled(
            "Type to filter",
            Style::default().fg(theme.colors.muted),
        ))
    } else {
        Line::from(Span::styled(
            format!("/ {}█", picker.filter()),
            Style::default().fg(theme.colors.foreground),
        ))
    }
}

/// The list: each route's heading over its models, a blank line between
/// routes, or a note when nothing is listed.
fn list_rows<'a>(
    app: &App,
    picker: &ModelPicker,
    models: &[&'a ModelOptionView],
    theme: &Theme,
) -> Vec<ListRow<'a>> {
    let muted = Style::default().fg(theme.colors.muted);
    let mut rows: Vec<ListRow<'a>> = Vec::new();
    let mut last_group: Option<&str> = None;
    for (idx, &model) in models.iter().enumerate() {
        if last_group != Some(model.group.as_str()) {
            if last_group.is_some() {
                rows.push((Line::from(""), None));
            }
            rows.push((Line::from(Span::styled(model.group.as_str(), muted)), None));
            last_group = Some(model.group.as_str());
        }
        rows.push((
            model_row(app, model, idx == picker.cursor(), theme),
            Some(idx),
        ));
    }
    if rows.is_empty() {
        let showing_ready = picker.tab() == crate::app::ModelTab::Ready;
        let empty = if picker.filter().is_empty() && !showing_ready {
            "Everything is signed in"
        } else {
            "No models match"
        };
        rows.push((Line::from(Span::styled(empty, muted)), None));
    }
    rows
}

/// Push the `room` rows of `rows` around the cursor onto `lines`, with
/// `↑ more` / `↓ more` markers where rows are hidden. Returns the line
/// each model row is on, with its index.
fn push_window<'a>(
    lines: &mut Vec<Line<'a>>,
    rows: Vec<ListRow<'a>>,
    room: usize,
    cursor: usize,
    muted: Style,
) -> Vec<(usize, usize)> {
    let cursor_row = rows
        .iter()
        .position(|(_, idx)| *idx == Some(cursor))
        .unwrap_or(0);
    let start = list_window_start(rows.len(), room, cursor_row);
    let total = rows.len();
    // Markers need a row each beside the cursor's; in a window too small
    // for that, every row is a list row.
    let markers = room >= 3;

    let mut hits: Vec<(usize, usize)> = Vec::new();
    for (offset, (line, idx)) in rows.into_iter().skip(start).take(room).enumerate() {
        if markers && offset == 0 && start > 0 {
            lines.push(Line::from(Span::styled("  ↑ more", muted)));
        } else if markers && offset + 1 == room && start + room < total {
            lines.push(Line::from(Span::styled("  ↓ more", muted)));
        } else {
            if let Some(idx) = idx {
                hits.push((lines.len(), idx));
            }
            lines.push(line);
        }
    }
    hits
}

/// The selector's title: what the model is picked for.
fn selector_title(picker: &ModelPicker) -> String {
    match picker.target() {
        crate::app::PickerTarget::Session => " Select model ".to_string(),
        crate::app::PickerTarget::BenchDifficulty { arm, difficulty } => format!(
            " Model for {} work · arm {} ",
            crate::app::DIFFICULTIES
                .get(difficulty)
                .copied()
                .unwrap_or("medium"),
            crate::events::arm_letter(arm)
        ),
        crate::app::PickerTarget::BenchClassifier => {
            " Model that classifies the prompts ".to_string()
        }
    }
}

/// The screen row of each model row inside the modal, with its index.
fn clickable_rows(
    hits: Vec<(usize, usize)>,
    modal_area: Rect,
    modal_height: u16,
) -> Vec<(u16, usize)> {
    // Lines past the bottom border are cut, so only rows inside it click.
    let shown = usize::from(modal_height.saturating_sub(2));
    hits.into_iter()
        .filter(|(line, _)| *line < shown)
        .filter_map(|(line, idx)| {
            let y = modal_area
                .y
                .checked_add(1)?
                .checked_add(u16::try_from(line).ok()?)?;
            Some((y, idx))
        })
        .collect()
}

/// First row of a `room`-row window over `total` rows that shows
/// `cursor`: the earliest window that has it, with one row below it to
/// spare, since the window's last row turns into a `↓ more` marker when
/// rows are hidden past it. The selector keeps no scroll state, so the
/// window is worked out again on every frame from the cursor alone.
fn list_window_start(total: usize, room: usize, cursor: usize) -> usize {
    if total <= room {
        return 0;
    }
    if room < 3 {
        // No room for markers around the cursor: put it first.
        return cursor.min(total - room);
    }
    (cursor + 2).saturating_sub(room).min(total - room)
}

/// One model's row: cursor marker, current check, label and what it
/// still needs (`not pulled`, `/login <provider>`).
fn model_row<'a>(app: &App, model: &'a ModelOptionView, selected: bool, theme: &Theme) -> Line<'a> {
    let current = model.id == app.picker_marked();
    let marker = if selected { "› " } else { "  " };
    let check = if current { "● " } else { "  " };
    let note = if model.local && !model.installed {
        "  not pulled".to_string()
    } else if model.available {
        String::new()
    } else {
        match model.provider.as_deref() {
            Some(provider) if provider != "zos" => format!("  /login {provider}"),
            _ => "  login required".to_string(),
        }
    };
    let label_color = if selected {
        theme.colors.primary
    } else {
        theme.colors.foreground
    };
    Line::from(vec![
        Span::styled(marker, Style::default().fg(theme.colors.primary)),
        Span::styled(check, Style::default().fg(theme.colors.success)),
        Span::styled(model.label.as_str(), Style::default().fg(label_color)),
        Span::styled(note, Style::default().fg(theme.colors.warning)),
    ])
}

#[cfg(test)]
mod window_tests {
    use super::list_window_start;

    /// Whatever the room, the cursor's row is in the window and is not a
    /// row a `↑ more` / `↓ more` marker takes.
    #[test]
    fn the_cursor_row_is_always_shown() {
        for total in 1..40 {
            for room in 1..=total {
                for cursor in 0..total {
                    let start = list_window_start(total, room, cursor);
                    assert!(start + room <= total, "{total} {room} {cursor}");
                    assert!(
                        (start..start + room).contains(&cursor),
                        "{total} {room} {cursor}: window {start}"
                    );
                    if room >= 3 {
                        let offset = cursor - start;
                        assert!(offset != 0 || start == 0, "{total} {room} {cursor}");
                        assert!(
                            offset + 1 != room || start + room == total,
                            "{total} {room} {cursor}"
                        );
                    }
                }
            }
        }
    }
}
