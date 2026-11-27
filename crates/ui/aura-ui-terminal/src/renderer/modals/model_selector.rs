//! The model selector: a Ready / Sign-in tab row, the filter once one is
//! typed, and the models on the tab grouped by route.

use super::with_notice;
use crate::app::{ModelPicker, ModelTab, PickerLayout, PickerTarget};
use crate::components::{caret_line, list_item, ListScroll, Modal, ModalArea};
use crate::{events::ModelOptionView, App, Theme};
use ratatui::{
    style::{Modifier, Style},
    text::{Line, Span},
    Frame,
};

/// Lines above the list: the tab row, the filter row and a blank line.
const HEAD_LINES: usize = 3;

/// A row of the list with the index of its model (headings and
/// separators have none), so the window can follow the cursor.
type ListRow<'a> = (Line<'a>, Option<usize>);

/// Draw the selector, its list scrolled to keep the cursor in view, and
/// record the rows a click picks.
pub(super) fn render(frame: &mut Frame, app: &mut App, theme: &Theme) -> ModalArea {
    let screen = frame.area();
    let picker = app.model_picker();
    let modal = with_notice(Modal::new(selector_title(picker), theme), app, theme);
    let width = modal.body_width(screen);
    let models = picker.visible(app.models());
    let rows = list_rows(app, picker, &models, width, theme);
    let total = rows.len();
    let room = modal.body_room(screen).saturating_sub(HEAD_LINES).max(1);
    let visible = room.min(total);
    let cursor_row = rows
        .iter()
        .position(|(_, idx)| *idx == Some(picker.cursor()))
        .unwrap_or(0);
    let first = window_start(total, visible, cursor_row);
    let mut lines = vec![
        tab_row(app, picker, theme),
        filter_line(picker, width, theme),
        Line::default(),
    ];
    let mut hits = Vec::new();
    for (line, idx) in rows.into_iter().skip(first).take(visible) {
        if let Some(idx) = idx {
            hits.push((lines.len(), idx));
        }
        lines.push(line);
    }
    let modal = modal.scroll(ListScroll {
        top: HEAD_LINES,
        visible,
        total,
        first,
    });
    let area = modal.render(frame.buffer_mut(), screen, lines);
    let rows = clickable_rows(hits, area);
    app.set_model_picker_layout(PickerLayout {
        modal: area.frame,
        rows,
    });
    area
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
    let showing_ready = picker.tab() == ModelTab::Ready;
    Line::from(vec![
        tab("Ready", ready, showing_ready),
        Span::raw("  "),
        tab("Sign in", sign_in, !showing_ready),
    ])
}

/// The filter typed so far after a `/`, or a blank line before one is.
fn filter_line<'a>(picker: &ModelPicker, width: usize, theme: &Theme) -> Line<'a> {
    if picker.filter().is_empty() {
        return Line::default();
    }
    let lead = vec![Span::styled("/ ", Style::default().fg(theme.colors.muted))];
    let text = Style::default().fg(theme.colors.foreground);
    caret_line(
        lead,
        picker.filter(),
        picker.filter().len(),
        width,
        text,
        theme,
    )
}

/// The list: each route's heading over its models, a blank line between
/// routes, or what the tab holds when no model is listed.
fn list_rows<'a>(
    app: &App,
    picker: &ModelPicker,
    models: &[&'a ModelOptionView],
    width: usize,
    theme: &Theme,
) -> Vec<ListRow<'a>> {
    let muted = Style::default().fg(theme.colors.muted);
    let mut rows: Vec<ListRow<'a>> = Vec::new();
    let mut last_group: Option<&str> = None;
    for (idx, &model) in models.iter().enumerate() {
        if last_group != Some(model.group.as_str()) {
            if last_group.is_some() {
                rows.push((Line::default(), None));
            }
            rows.push((Line::from(Span::styled(model.group.as_str(), muted)), None));
            last_group = Some(model.group.as_str());
        }
        let selected = idx == picker.cursor();
        rows.push((model_row(app, model, selected, width, theme), Some(idx)));
    }
    if rows.is_empty() {
        let showing_ready = picker.tab() == ModelTab::Ready;
        let empty = if picker.filter().is_empty() && !showing_ready {
            "Everything is signed in"
        } else {
            "No models match"
        };
        rows.push((Line::from(Span::styled(empty, muted)), None));
    }
    rows
}

/// One model's row: the current one's check, the label and what it still
/// needs (`not pulled`, `/login <provider>`).
fn model_row<'a>(
    app: &App,
    model: &'a ModelOptionView,
    selected: bool,
    width: usize,
    theme: &Theme,
) -> Line<'a> {
    let current = model.id == app.picker_marked();
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
    let spans = vec![
        Span::styled(check, Style::default().fg(theme.colors.success)),
        Span::styled(
            model.label.as_str(),
            Style::default().fg(theme.colors.foreground),
        ),
        Span::styled(note, Style::default().fg(theme.colors.warning)),
    ];
    list_item(spans, selected, width, theme)
}

/// The selector's title: what the model is picked for.
fn selector_title(picker: &ModelPicker) -> String {
    match picker.target() {
        PickerTarget::Session => "SELECT MODEL".to_string(),
        PickerTarget::BenchDifficulty { arm, difficulty } => format!(
            "MODEL FOR {} WORK · ARM {}",
            crate::app::DIFFICULTIES
                .get(difficulty)
                .copied()
                .unwrap_or("medium")
                .to_uppercase(),
            crate::events::arm_letter(arm)
        ),
        PickerTarget::BenchClassifier => "MODEL THAT CLASSIFIES THE PROMPTS".to_string(),
    }
}

/// The screen row of each model row drawn, with its index. Line `n` of
/// the body is on row `body.y + n`; lines past the body are cut, so only
/// rows inside it click.
fn clickable_rows(hits: Vec<(usize, usize)>, area: ModalArea) -> Vec<(u16, usize)> {
    let shown = usize::from(area.body.height);
    hits.into_iter()
        .filter(|(line, _)| *line < shown)
        .filter_map(|(line, idx)| Some((area.body.y.checked_add(u16::try_from(line).ok()?)?, idx)))
        .collect()
}

/// First row of a `visible`-row window over `total` rows that shows the
/// cursor's row, with a row under it to spare while the window has two.
/// The selector keeps no scroll state, so the window is worked out again
/// on every frame from the cursor alone.
fn window_start(total: usize, visible: usize, cursor: usize) -> usize {
    let spare = usize::from(visible > 1);
    (cursor + 1 + spare)
        .saturating_sub(visible)
        .min(total.saturating_sub(visible))
}

#[cfg(test)]
mod window_tests {
    use super::window_start;

    /// Whatever the room, the window fits the list and holds the cursor's
    /// row, with a row under it while there is one and the room for it.
    #[test]
    fn the_cursor_row_is_always_shown() {
        for total in 1..40 {
            for visible in 1..=total {
                for cursor in 0..total {
                    let start = window_start(total, visible, cursor);
                    let end = start + visible;
                    assert!(end <= total, "{total} {visible} {cursor}");
                    assert!(
                        (start..end).contains(&cursor),
                        "{total} {visible} {cursor}: window {start}"
                    );
                    if visible > 1 && cursor + 1 < total {
                        assert!(cursor + 1 < end, "{total} {visible} {cursor}");
                    }
                }
            }
        }
    }
}
