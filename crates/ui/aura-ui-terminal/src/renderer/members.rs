//! The Kernel panel's MEMBERS tab: the agent on screen, which leads its
//! run, then every agent the run brought in, each under the one that
//! started it, with how it is doing and how long it has worked.

use super::chrome::{render_scrollbar, ScrollView};
use super::processes::{format_elapsed, status_icon};
use super::side_list::{
    empty_line, name_style, render_empty, render_hint, split_hint, Tone, MEMBERS_HINT,
};
use crate::app::{HoverTarget, MemberRow, PanelFocus};
use crate::components::{cell_style, list_row, now_ms, CellState};
use crate::{App, Theme};
use ratatui::{layout::Rect, text::Line, widgets::Paragraph, Frame};

/// Draw the members into `inner`, the body below the tab bar of the
/// panel bordered by `area`.
pub(super) fn render_members(
    frame: &mut Frame,
    area: Rect,
    inner: Rect,
    app: &mut App,
    theme: &Theme,
) {
    let is_focused = app.focus() == PanelFocus::Records;
    if app.members().is_empty() {
        render_empty(frame, inner, "No agent on screen.", theme);
        return;
    }

    let (rows_area, hint) = split_hint(inner, is_focused);
    let visible = usize::from(rows_area.height);
    let scroll = app.sync_members_scroll(visible);
    let hovered = match app.hover() {
        Some(HoverTarget::MemberRow(index)) => Some(index),
        _ => None,
    };
    let selected = app.selected_member();
    let now = now_ms();
    let width = usize::from(inner.width);
    let mut lines: Vec<Line> = app
        .members()
        .iter()
        .enumerate()
        .skip(scroll)
        .take(visible)
        .map(|(i, member)| {
            let state = CellState::of(is_focused && i == selected, hovered == Some(i));
            let cell = cell_style(
                theme,
                state,
                name_style(Tone::active_if(member.status.is_live()), theme),
            );
            let icon = status_icon(app, member.status, theme);
            let time = member_time(member, now);
            list_row(width, member.depth, icon, &member.label, cell, Some(&time))
        })
        .collect();
    if app.members().len() == 1 && visible >= 2 {
        // The lead alone: say who would join.
        lines.push(Line::from(""));
        lines.push(empty_line("No agents joined yet.", inner.width, theme));
    }
    let view = ScrollView {
        total: app.members().len(),
        visible,
        first: scroll,
    };
    app.set_member_rows_area(rows_area, scroll);
    frame.render_widget(Paragraph::new(lines), rows_area);
    render_scrollbar(frame, area, rows_area, view, is_focused, theme);
    if let Some(hint) = hint {
        render_hint(frame, hint, MEMBERS_HINT, theme);
    }
}

/// `lead` for the agent on screen; how a member joined and how long it
/// has worked, or worked, for the rest: `task 4m12s`.
fn member_time(member: &MemberRow, now_ms: u64) -> String {
    let Some(kind) = member.kind else {
        return "lead".to_string();
    };
    let elapsed = member
        .ended_ms
        .unwrap_or(now_ms)
        .saturating_sub(member.started_ms);
    format!("{} {}", kind.label(), format_elapsed(elapsed))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::events::{AgentSummary, ProcessKind, ProcessStatus, ProcessSummary, UiCommand};
    use crate::RecordsTab;
    use ratatui::{backend::TestBackend, Terminal};

    fn render_rows(app: &mut App, width: u16, height: u16) -> Vec<String> {
        let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
        terminal
            .draw(|frame| crate::renderer::render(frame, app, &Theme::cyber()))
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

    fn app_with_members() -> App {
        let mut app = App::new();
        app.process_command(UiCommand::SetAgents(vec![AgentSummary {
            id: "aa".into(),
            name: "Lead".into(),
            ..AgentSummary::default()
        }]));
        app.process_command(UiCommand::SetActiveAgent("aa".into()));
        let task = |id: &str, owner: &str, status: ProcessStatus| ProcessSummary {
            id: format!("task:{id}"),
            kind: ProcessKind::Task,
            label: format!("{id}-worker (explorer)"),
            status,
            owner_agent_id: owner.into(),
            started_ms: 1_000,
            ended_ms: (!status.is_live()).then_some(5_000),
            jump_agent_id: id.into(),
            ..ProcessSummary::default()
        };
        app.process_command(UiCommand::SetProcesses(vec![
            task("bb", "aa", ProcessStatus::Exited),
            task("cc", "bb", ProcessStatus::Failed),
        ]));
        app
    }

    #[test]
    fn the_members_tab_lists_the_run_under_its_lead() {
        let mut app = app_with_members();
        app.select_pane_tab(crate::PaneTab::Records(RecordsTab::Members));
        let rows = render_rows(&mut app, 240, 30);
        let screen = rows.join("\n");
        assert!(
            screen.contains("│  KERNEL  │ MEMBERS  │ POLICIES │"),
            "{screen}"
        );
        assert!(screen.contains("3 AGENTS │"), "{screen}");
        let lead = rows.iter().find(|row| row.contains("Lead")).unwrap();
        assert!(lead.contains("lead"), "{lead}");
        let child = rows.iter().find(|row| row.contains("bb-worker")).unwrap();
        assert!(child.contains("✓") && child.contains("task 4s"), "{child}");
        let grandchild = rows.iter().find(|row| row.contains("cc-worker")).unwrap();
        assert!(grandchild.contains("✗"), "{grandchild}");
        let indent = |row: &str| row.find('✓').or_else(|| row.find('✗')).unwrap();
        assert!(
            indent(grandchild) > indent(child),
            "nested under its starter"
        );
        assert!(screen.contains("o chat"), "the focused tab shows its keys");
    }

    #[test]
    fn the_kernel_tab_keeps_its_record_count_beside_the_tabs() {
        let mut app = app_with_members();
        let rows = render_rows(&mut app, 240, 30);
        let screen = rows.join("\n");
        assert_eq!(app.records_tab(), RecordsTab::Kernel);
        assert!(
            screen.contains("│  KERNEL  │ MEMBERS  │ POLICIES │"),
            "{screen}"
        );
        assert!(screen.contains("0 RECORDS │"), "{screen}");
        assert!(!screen.contains("bb-worker"), "{screen}");
    }

    #[test]
    fn a_lead_alone_says_who_would_join() {
        let mut app = App::new();
        app.process_command(UiCommand::SetActiveAgent("aa".into()));
        app.select_pane_tab(crate::PaneTab::Records(RecordsTab::Members));
        let screen = render_rows(&mut app, 240, 30).join("\n");
        assert!(screen.contains("No agents joined yet."), "{screen}");
        assert!(screen.contains("1 AGENT │"), "{screen}");
    }

    #[test]
    fn hosted_kernel_bars_hand_the_front_end_every_tab() {
        let mut app = app_with_members();
        app.set_host_chrome(true);
        app.select_pane_tab(crate::PaneTab::Records(RecordsTab::Members));
        render_rows(&mut app, 240, 30);
        let chrome = app
            .pane_chrome()
            .iter()
            .find(|chrome| chrome.pane == crate::Pane::Records)
            .expect("the kernel bar is handed over");
        assert_eq!(
            chrome.tabs,
            [
                crate::PaneTab::Records(RecordsTab::Kernel),
                crate::PaneTab::Records(RecordsTab::Members),
                crate::PaneTab::Records(RecordsTab::Policies),
                crate::PaneTab::Records(RecordsTab::Inbox),
            ]
        );
        assert_eq!(
            chrome.active,
            Some(crate::PaneTab::Records(RecordsTab::Members))
        );
        assert_eq!(chrome.title, None);
        assert_eq!(chrome.info.as_deref(), Some("3 AGENTS"));
    }
}
