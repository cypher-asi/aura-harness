//! The HOST tab and the footer's placement switch.
//!
//! The tab is an info document; a fact near its limit (`warn`) shows in
//! the accent, bar and value.
//!
//! ```text
//! boxd · aura-re36-3f9c
//! ● ready · up 2h 14m · sleeps in 4m
//!
//! MACHINE
//!   Region    fra
//!   Size      4 vCPU · 16.0 GiB · 100 GiB disk
//!
//! RESOURCES
//!   CPU       ████░░░░░░ 4 cores · 37% · load 1.20 0.90 0.40
//!
//! COST
//!   Now       €0.0500/h est.
//!
//! [p] pull changes  [s] sleep  [m] move
//! ```

use super::chrome::Body;
use super::side_list::render_empty;
use crate::app::{switch_label, switch_rect, PanelFocus, SideScope, SWITCH_GAP};
use crate::components::{Info, Value};
use crate::host::{HostView, PlacementView};
use crate::{App, Theme};
use ratatui::{
    layout::Rect,
    style::Style,
    text::{Line, Span},
    widgets::Paragraph,
    Frame,
};

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

/// Every line of `view`, the agent's host or (in project scope) every
/// host of the project, for a body `width` columns wide.
fn host_lines(view: &HostView, project: bool, width: usize, theme: &Theme) -> Vec<Line<'static>> {
    let mut info = Info::new(width, theme);
    info.title(&view.title);
    info.status(&view.status);
    if let Some(at) = &view.as_of {
        info.note(&format!("asleep · last known at {at}"));
    }
    if project && !view.project.is_empty() {
        info.section("PROJECT", None);
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
            let detail = format!("{} · {}{cost}{owner}", row.host, row.state);
            info.item(&row.agent, Some(&detail));
        }
        if let Some(total) = &view.project_total {
            info.item(total, None);
        }
    }
    for section in &view.sections {
        info.section(&section.title, None);
        for fact in &section.facts {
            let kind = Value::flag_if(fact.warn);
            match fact.percent {
                Some(percent) => info.bar(&fact.label, percent, &fact.value, kind),
                None => info.fact(&fact.label, &fact.value, kind),
            }
        }
    }
    let actions: Vec<(char, &str)> = view
        .actions
        .iter()
        .map(|action| (action.key(), action.label()))
        .collect();
    info.actions(&actions);
    info.into_lines()
}

/// Draw the HOST tab into `inner`, the body below the tab bar of the
/// panel bordered by `area`.
pub(super) fn render_host(
    frame: &mut Frame,
    area: Rect,
    inner: Rect,
    app: &mut App,
    theme: &Theme,
) {
    let Some(view) = app.host_view() else {
        render_empty(frame, inner, "Reading the host…", theme);
        return;
    };
    let body = Body::new(area, inner, app.focus() == PanelFocus::Sandbox);
    let project = app.side_scope() == SideScope::Project;
    let lines = host_lines(view, project, body.width(), theme);
    let first = app.sync_host_scroll(lines.len(), body.visible());
    body.draw_document(frame, lines, first, theme);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::host::{HostAction, HostFact, HostRow, HostSection};
    use ratatui::style::Modifier;

    fn text(lines: &[Line<'_>]) -> Vec<String> {
        lines
            .iter()
            .map(|line| line.spans.iter().map(|s| s.content.as_ref()).collect())
            .collect()
    }

    fn view() -> HostView {
        HostView {
            agent_id: "aa".into(),
            placement: PlacementView::Cloud,
            title: "boxd · aura-re36-3f9c".into(),
            status: "● ready".into(),
            sections: vec![HostSection {
                title: "RESOURCES".into(),
                facts: vec![
                    HostFact::new("Memory", "6.0 GiB of 16.0 GiB").with_percent(37.5),
                    HostFact::new("Disk", "91 GiB of 100 GiB").with_percent(91.0),
                ],
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
        }
    }

    #[test]
    fn a_near_full_use_and_its_bar_take_the_accent() {
        let theme = Theme::cyber();
        let lines = host_lines(&view(), false, 80, &theme);
        let title = &lines[0].spans[0];
        assert_eq!(title.style.fg, Some(theme.colors.foreground));
        assert!(title.style.add_modifier.contains(Modifier::BOLD));
        let disk = lines
            .iter()
            .find(|line| line.spans.iter().any(|span| span.content.contains("Disk")))
            .expect("the disk fact");
        let fg = |content: &str| {
            disk.spans
                .iter()
                .find(|span| span.content.as_ref() == content)
                .and_then(|span| span.style.fg)
        };
        assert_eq!(fg("██████████"), Some(theme.colors.success));
        assert_eq!(fg("91 GiB of 100 GiB"), Some(theme.colors.success));
    }

    #[test]
    fn a_long_view_scrolls_inside_a_scrollbar_with_the_tabs_hint() {
        use crate::events::{AgentSummary, UiCommand};
        use crate::{PaneTab, SidePanelTab};
        use ratatui::{backend::TestBackend, Terminal};

        let mut app = App::new();
        app.process_command(UiCommand::SetAgents(vec![AgentSummary {
            id: "aa".into(),
            name: "Lead".into(),
            ..AgentSummary::default()
        }]));
        app.process_command(UiCommand::SetActiveAgent("aa".into()));
        let mut long = view();
        long.sections = (0..12)
            .map(|n| HostSection {
                title: format!("SECTION {n}"),
                facts: vec![HostFact::new("Region", "fra")],
            })
            .collect();
        app.process_command(UiCommand::SetHost(Box::new(long)));
        app.select_pane_tab(PaneTab::Side(SidePanelTab::Host));
        fn screen(app: &mut App) -> String {
            let mut terminal = Terminal::new(TestBackend::new(160, 40)).unwrap();
            terminal
                .draw(|frame| crate::renderer::render(frame, app, &Theme::cyber()))
                .unwrap();
            let buffer = terminal.backend().buffer();
            (0..buffer.area.height)
                .map(|y| {
                    (0..buffer.area.width)
                        .map(|x| buffer[(x, y)].symbol())
                        .collect::<String>()
                })
                .collect::<Vec<_>>()
                .join("\n")
        }
        let all = screen(&mut app);
        assert!(all.contains("boxd · aura-re36-3f9c"), "{all}");
        assert!(all.contains("↑↓ scroll  ⇧←→ tabs"), "{all}");
        assert!(all.contains('┃'), "the scrollbar's thumb: {all}");
        assert!(!all.contains("[p] pull changes"), "the buttons are below");

        app.handle_key(crossterm::event::KeyEvent::from(
            crossterm::event::KeyCode::End,
        ));
        let all = screen(&mut app);
        assert!(all.contains("[p] pull changes"), "{all}");
        assert!(!all.contains("boxd · aura-re36-3f9c"), "{all}");
    }

    #[test]
    fn a_view_lists_its_sections_bars_and_buttons() {
        let view = view();
        let lines = text(&host_lines(&view, false, 80, &Theme::default()));
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
        let project = text(&host_lines(&view, true, 80, &Theme::default()));
        assert!(
            project
                .iter()
                .any(|l| l.contains("RE36  aura-re36-3f9c · ready · €0.05/h · €0.12 · Alice")),
            "{project:?}"
        );
    }
}
