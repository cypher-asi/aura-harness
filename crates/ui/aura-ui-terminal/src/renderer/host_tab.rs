//! The HOST tab and the footer's placement switch.
//!
//! ```text
//! boxd · aura-re36-3f9c
//! ● ready · up 2h 14m · sleeps in 4m
//!
//! MACHINE
//!   Region    fra
//!   Size      4 vCPU · 16.0 GiB · 100 GiB disk
//! RESOURCES
//!   CPU       ████░░░░░░  4 cores · 37% · load 1.20 0.90 0.40
//! COST
//!   Now       €0.0500/h est.
//!
//! [p] pull changes  [s] sleep  [m] move
//! ```

use super::side_list::render_empty;
use super::text::display_width;
use crate::app::{switch_label, switch_rect, SideScope, SWITCH_GAP};
use crate::host::{HostFact, HostView, PlacementView};
use crate::{App, Theme};
use ratatui::{
    layout::Rect,
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::Paragraph,
    Frame,
};

/// Columns the labels of facts take.
const LABEL_WIDTH: usize = 10;
/// Cells of a use bar.
const BAR_WIDTH: usize = 10;

/// Take the switch's cells off the right of the footer `row` and draw it
/// there. Returns the row left for the status and stats.
pub(super) fn split_footer(frame: &mut Frame, row: Rect, app: &App, theme: &Theme) -> Rect {
    let state = app.placement();
    let label = switch_label(&state);
    let Some(rect) = switch_rect(row, &label) else {
        return row;
    };
    let color = match (state.moving.is_some(), state.placement) {
        (true, _) => theme.colors.success,
        (false, PlacementView::Cloud) => theme.colors.primary,
        (false, PlacementView::Local) => theme.colors.muted,
    };
    frame.render_widget(
        Paragraph::new(Line::from(Span::styled(label, Style::default().fg(color)))),
        rect,
    );
    Rect {
        width: row.width.saturating_sub(rect.width + SWITCH_GAP),
        ..row
    }
}

fn bar(percent: u8, theme: &Theme, warn: bool) -> Vec<Span<'static>> {
    let filled = (usize::from(percent) * BAR_WIDTH)
        .div_ceil(100)
        .min(BAR_WIDTH);
    let color = if warn {
        theme.colors.warning
    } else {
        theme.colors.success
    };
    vec![
        Span::styled("█".repeat(filled), Style::default().fg(color)),
        Span::styled(
            "░".repeat(BAR_WIDTH - filled),
            Style::default().fg(theme.colors.faint),
        ),
        Span::raw(" "),
    ]
}

fn fact_line(fact: &HostFact, theme: &Theme) -> Line<'static> {
    let mut label = fact.label.clone();
    let pad = LABEL_WIDTH.saturating_sub(display_width(&label));
    label.push_str(&" ".repeat(pad));
    let mut spans = vec![
        Span::raw("  "),
        Span::styled(label, Style::default().fg(theme.colors.muted)),
    ];
    if let Some(percent) = fact.percent {
        spans.extend(bar(percent, theme, fact.warn));
    }
    let color = if fact.warn {
        theme.colors.warning
    } else {
        theme.colors.foreground
    };
    spans.push(Span::styled(fact.value.clone(), Style::default().fg(color)));
    Line::from(spans)
}

/// Every line of `view`, the agent's host or (in project scope) every
/// host of the project.
fn host_lines(view: &HostView, project: bool, theme: &Theme) -> Vec<Line<'static>> {
    let heading = Style::default()
        .fg(theme.colors.primary)
        .add_modifier(Modifier::BOLD);
    let mut lines = vec![
        Line::from(Span::styled(view.title.clone(), heading)),
        Line::from(Span::styled(
            view.status.clone(),
            Style::default().fg(theme.colors.success),
        )),
    ];
    if let Some(at) = &view.as_of {
        lines.push(Line::from(Span::styled(
            format!("asleep · last known at {at}"),
            Style::default().fg(theme.colors.muted),
        )));
    }
    if project && !view.project.is_empty() {
        lines.push(Line::default());
        lines.push(Line::from(Span::styled(
            "PROJECT",
            Style::default().fg(theme.colors.secondary),
        )));
        for row in &view.project {
            let cost = if row.per_hour.is_empty() {
                String::new()
            } else {
                format!(" · {} · {}", row.per_hour, row.total)
            };
            let owner = row
                .owner
                .as_ref()
                .map(|owner| format!(" · {owner}"))
                .unwrap_or_default();
            lines.push(Line::from(vec![
                Span::raw("  "),
                Span::styled(
                    row.agent.clone(),
                    Style::default().fg(theme.colors.foreground),
                ),
                Span::styled(
                    format!("  {} · {}{cost}{owner}", row.host, row.state),
                    Style::default().fg(theme.colors.muted),
                ),
            ]));
        }
        if let Some(total) = &view.project_total {
            lines.push(Line::from(Span::styled(
                format!("  {total}"),
                Style::default().fg(theme.colors.foreground),
            )));
        }
    }
    for section in &view.sections {
        lines.push(Line::default());
        lines.push(Line::from(Span::styled(
            section.title.clone(),
            Style::default().fg(theme.colors.secondary),
        )));
        lines.extend(section.facts.iter().map(|fact| fact_line(fact, theme)));
    }
    if !view.actions.is_empty() {
        lines.push(Line::default());
        let mut spans = Vec::new();
        for action in &view.actions {
            spans.push(Span::styled(
                format!("[{}]", action.key()),
                Style::default().fg(theme.colors.primary),
            ));
            spans.push(Span::styled(
                format!(" {}  ", action.label()),
                Style::default().fg(theme.colors.muted),
            ));
        }
        lines.push(Line::from(spans));
    }
    lines
}

/// Draw the HOST tab into `inner`.
pub(super) fn render_host(frame: &mut Frame, inner: Rect, app: &App, theme: &Theme) {
    let Some(view) = app.host_view() else {
        render_empty(frame, inner, "Reading the host…", theme);
        return;
    };
    let lines = host_lines(view, app.side_scope() == SideScope::Project, theme);
    let visible = usize::from(inner.height);
    let first = app.host_scroll().min(lines.len().saturating_sub(visible));
    let shown: Vec<Line> = lines.into_iter().skip(first).take(visible).collect();
    frame.render_widget(Paragraph::new(shown), inner);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::host::{HostAction, HostRow, HostSection};

    fn text(lines: &[Line<'_>]) -> Vec<String> {
        lines
            .iter()
            .map(|line| line.spans.iter().map(|s| s.content.as_ref()).collect())
            .collect()
    }

    #[test]
    fn a_view_lists_its_sections_bars_and_buttons() {
        let view = HostView {
            agent_id: "aa".into(),
            placement: PlacementView::Cloud,
            title: "boxd · aura-re36-3f9c".into(),
            status: "● ready".into(),
            sections: vec![HostSection {
                title: "RESOURCES".into(),
                facts: vec![HostFact::new("Memory", "6.0 GiB of 16.0 GiB").with_percent(37.5)],
            }],
            actions: vec![HostAction::Pull, HostAction::Sleep],
            project: vec![HostRow {
                agent_id: "aa".into(),
                agent: "RE36".into(),
                host: "aura-re36-3f9c".into(),
                state: "ready".into(),
                per_hour: "€0.05/h".into(),
                total: "€0.12".into(),
                owner: Some("Alice".into()),
            }],
            project_total: Some("1 cloud machine".into()),
            as_of: None,
        };
        let lines = text(&host_lines(&view, false, &Theme::default()));
        assert_eq!(lines[0], "boxd · aura-re36-3f9c");
        assert!(lines.iter().any(|l| l == "RESOURCES"));
        assert!(
            lines
                .iter()
                .any(|l| l.contains("Memory    ████░░░░░░ 6.0 GiB of 16.0 GiB")),
            "{lines:?}"
        );
        assert!(lines
            .last()
            .is_some_and(|l| l.contains("[p] pull changes") && l.contains("[s] sleep")));
        assert!(
            !lines.iter().any(|l| l == "PROJECT"),
            "project rows only in project scope"
        );
        let project = text(&host_lines(&view, true, &Theme::default()));
        assert!(
            project
                .iter()
                .any(|l| l.contains("RE36  aura-re36-3f9c · ready · €0.05/h · €0.12 · Alice")),
            "{project:?}"
        );
    }
}
