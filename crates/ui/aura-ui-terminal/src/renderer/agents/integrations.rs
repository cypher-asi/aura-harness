//! The Integrations app's list: every integration with a coloured dot
//! for its state, its icon and name, and what it is; the selected one
//! also shows its status, its last result, what it provides, and its
//! actions.
//!
//! ```text
//!  ● ⑂ GitHub
//!      Repositories, pull requests and pushes
//!      gh login; GitHub App set for cloud pushes
//!      Provides  pull requests on the REPO tab · pushes from cloud machines
//!      Connect  Configure  Test  Disconnect
//!
//!  ○ ☁ boxd
//!      Cloud machines for cloud agents
//! ```
//!
//! The dot is the state: connected in the success colour, not connected
//! muted and hollow, failed in the error colour, an action running in
//! the warning colour.

use crate::app::IntegrationButton;
use crate::events::{IntegrationState, IntegrationView};
use crate::renderer::text::display_width;
use crate::{App, Theme};
use ratatui::{
    layout::Rect,
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::Paragraph,
    Frame,
};

/// Columns before a row's second line, under its name.
const INDENT: &str = "     ";

/// The dot for `state` and its colour.
pub(crate) fn state_dot(state: IntegrationState, theme: &Theme) -> (&'static str, Color) {
    match state {
        IntegrationState::Connected => ("●", theme.colors.success),
        IntegrationState::NotConnected => ("○", theme.colors.muted),
        IntegrationState::Failed => ("●", theme.colors.error),
        IntegrationState::Busy => ("●", theme.colors.warning),
    }
}

/// Draw the list into `body`, recording where rows and actions fell.
pub(super) fn render_integrations(frame: &mut Frame, body: Rect, app: &mut App, theme: &Theme) {
    let width = usize::from(body.width);
    let mut lines: Vec<Line<'static>> = Vec::new();
    let mut rows = Vec::new();
    let mut buttons = Vec::new();
    if app.integrations().is_empty() {
        let text = if app.integrations_loaded() {
            " No integrations."
        } else {
            " Loading…"
        };
        lines.push(Line::styled(text, Style::default().fg(theme.colors.muted)));
    }
    let selected = app.selected_integration();
    for (index, view) in app.integrations().iter().enumerate() {
        if index > 0 {
            lines.push(Line::default());
        }
        let top = lines.len();
        lines.extend(row_lines(view, index == selected, width, theme));
        if index == selected {
            let y = body.y + u16::try_from(lines.len()).unwrap_or(u16::MAX);
            lines.extend(action_lines((body.x, y, body.width), &mut buttons, theme));
        }
        let height = u16::try_from(lines.len() - top).unwrap_or(u16::MAX);
        let y = body
            .y
            .saturating_add(u16::try_from(top).unwrap_or(u16::MAX));
        rows.push((Rect::new(body.x, y, body.width, height), index));
    }
    let visible = |rect: &Rect| rect.y < body.bottom();
    rows.retain(|(rect, _)| visible(rect));
    buttons.retain(|(rect, _)| visible(rect));
    app.set_integration_areas(rows, buttons);
    frame.render_widget(Paragraph::new(lines), body);
}

/// A row's lines: its name line and description, and for the selected
/// row its status, last result and what it provides.
fn row_lines(
    view: &IntegrationView,
    selected: bool,
    width: usize,
    theme: &Theme,
) -> Vec<Line<'static>> {
    let (dot, color) = state_dot(view.state, theme);
    let name_style = Style::default().fg(theme.colors.foreground);
    let name_style = if selected {
        name_style.add_modifier(Modifier::BOLD)
    } else {
        name_style
    };
    let muted = Style::default().fg(theme.colors.muted);
    let mut lines = vec![
        Line::from(vec![
            Span::raw(" "),
            Span::styled(dot, Style::default().fg(color)),
            Span::raw(" "),
            Span::styled(format!("{} ", view.icon), name_style),
            Span::styled(view.name.clone(), name_style),
        ]),
        Line::styled(cut(&format!("{INDENT}{}", view.description), width), muted),
    ];
    if !selected {
        return lines;
    }
    let status_color = match view.state {
        IntegrationState::Failed => theme.colors.error,
        IntegrationState::Connected => theme.colors.success,
        _ => theme.colors.muted,
    };
    let status = Style::default().fg(status_color);
    lines.push(Line::styled(
        cut(&format!("{INDENT}{}", view.status), width),
        status,
    ));
    if let Some(note) = view.note.as_deref().filter(|note| !note.is_empty()) {
        lines.push(Line::styled(cut(&format!("{INDENT}{note}"), width), muted));
    }
    if !view.provides.is_empty() {
        let provides = format!("{INDENT}Provides  {}", view.provides.join(" · "));
        lines.push(Line::styled(cut(&provides, width), muted));
    }
    lines
}

/// The actions, `Connect` and so on, flowing onto as many lines as
/// the column needs from row `y`, each span recorded where it fell.
fn action_lines(
    (x, y, width): (u16, u16, u16),
    buttons: &mut Vec<(Rect, IntegrationButton)>,
    theme: &Theme,
) -> Vec<Line<'static>> {
    let indent = u16::try_from(INDENT.len()).unwrap_or(0);
    let style = Style::default().fg(theme.colors.primary);
    let mut lines = vec![vec![Span::raw(INDENT)]];
    let mut column = indent;
    for button in IntegrationButton::ALL {
        let text = button.label().1.to_string();
        let cells = u16::try_from(display_width(&text)).unwrap_or(u16::MAX);
        if column > indent && column + cells > width {
            lines.push(vec![Span::raw(INDENT)]);
            column = indent;
        }
        let row = y + u16::try_from(lines.len() - 1).unwrap_or(0);
        buttons.push((Rect::new(x + column, row, cells, 1), button));
        if let Some(line) = lines.last_mut() {
            line.push(Span::styled(text, style));
            line.push(Span::raw("  "));
        }
        column += cells + 2;
    }
    lines.into_iter().map(Line::from).collect()
}

/// `text` cut to `width` columns with an ellipsis.
fn cut(text: &str, width: usize) -> String {
    crate::components::truncate_line(text, width)
}
