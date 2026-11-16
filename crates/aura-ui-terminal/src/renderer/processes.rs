//! Processes tab: every agent turn, spawned agent and background command,
//! and the detail view of one of them.

use super::panels::{
    aura_icon, cell_style, list_row, now_ms, render_scrollbar, truncate_record_info, CellState,
    ScrollView, StatusIcon,
};
use super::text::wrap_words;
use crate::app::{HoverTarget, PanelFocus};
use crate::events::{ProcessStatus, ProcessSummary};
use crate::{App, Theme};
use ratatui::{
    layout::Rect,
    style::{Color, Style},
    text::{Line, Span},
    widgets::Paragraph,
    Frame,
};

/// Hint shown at the bottom of the tab when the panel has focus.
const PROCESSES_HINT: &str = "⏎ details  o agent  k kill  l log  ⇧← files";

/// Draw the Processes list into `inner`, the body below the tab bar of the
/// panel bordered by `area`.
pub(super) fn render_processes(
    frame: &mut Frame,
    area: Rect,
    inner: Rect,
    app: &mut App,
    theme: &Theme,
) {
    let is_focused = app.focus() == PanelFocus::Sandbox;
    if app.processes().is_empty() {
        let lines: Vec<Line> = [
            "  Nothing running.",
            "  Agent turns, spawned agents",
            "  and background commands",
            "  appear here.",
        ]
        .into_iter()
        .map(|text| Line::from(Span::styled(text, Style::default().fg(theme.colors.muted))))
        .collect();
        frame.render_widget(Paragraph::new(lines), inner);
        return;
    }

    let show_hint = is_focused && inner.height >= 4;
    let rows_area = Rect {
        height: inner.height.saturating_sub(u16::from(show_hint)),
        ..inner
    };
    let visible = rows_area.height as usize;
    let scroll = app.sync_processes_scroll(visible);
    let hovered = match app.hover() {
        Some(HoverTarget::ProcessRow(index)) => Some(index),
        _ => None,
    };
    let selected = app.selected_process();
    let now = now_ms();
    let width = inner.width as usize;
    let lines: Vec<Line> = app
        .processes()
        .iter()
        .enumerate()
        .skip(scroll)
        .take(visible)
        .map(|(i, process)| {
            let state = CellState::of(is_focused && i == selected, hovered == Some(i));
            let cell = cell_style(
                theme,
                state,
                Style::default().fg(label_color(process, theme)),
            );
            let icon = status_icon(app, process.status, theme);
            let time = format!(
                "{} {}",
                process.kind.label(),
                format_elapsed(process_elapsed_ms(process, now))
            );
            list_row(
                width,
                process.depth,
                icon,
                &process.label,
                cell,
                Some(&time),
            )
        })
        .collect();
    let view = ScrollView {
        total: app.processes().len(),
        visible,
        first: scroll,
    };
    app.set_process_rows_area(rows_area, scroll);
    frame.render_widget(Paragraph::new(lines), rows_area);
    render_scrollbar(frame, area, rows_area, view, is_focused, theme);

    if show_hint {
        let hint_area = Rect {
            y: inner.y + inner.height - 1,
            height: 1,
            ..inner
        };
        let hint = truncate_record_info(PROCESSES_HINT, width.saturating_sub(1));
        frame.render_widget(
            Paragraph::new(Line::from(Span::styled(
                format!(" {hint}"),
                Style::default().fg(theme.colors.muted),
            ))),
            hint_area,
        );
    }
}

fn label_color(process: &ProcessSummary, theme: &Theme) -> Color {
    if process.status.is_live() {
        theme.colors.foreground
    } else {
        theme.colors.muted
    }
}

/// Spinning ring while running; a glyph for every other state.
fn status_icon(app: &App, status: ProcessStatus, theme: &Theme) -> StatusIcon {
    let (glyph, color) = status_glyph(status, theme);
    if status == ProcessStatus::Running {
        aura_icon(app)
    } else {
        StatusIcon::new(glyph, color)
    }
}

/// Glyph and color of a process state, used in the list and the detail
/// view's title.
pub(super) const fn status_glyph(status: ProcessStatus, theme: &Theme) -> (&'static str, Color) {
    match status {
        ProcessStatus::Running => ("●", theme.colors.pending),
        ProcessStatus::Idle => ("◦", theme.colors.muted),
        ProcessStatus::Paused => ("‖", theme.colors.warning),
        ProcessStatus::Exited => ("✓", theme.colors.success),
        ProcessStatus::Failed => ("✗", theme.colors.error),
        ProcessStatus::Killed => ("■", theme.colors.muted),
    }
}

/// How long the process ran, or has been running.
fn process_elapsed_ms(process: &ProcessSummary, now_ms: u64) -> u64 {
    process
        .ended_ms
        .unwrap_or(now_ms)
        .saturating_sub(process.started_ms)
}

/// `8s`, `4m12s`, `2h05m`.
pub(super) fn format_elapsed(ms: u64) -> String {
    let secs = ms / 1000;
    match secs {
        s if s < 60 => format!("{s}s"),
        s if s < 3600 => format!("{}m{:02}s", s / 60, s % 60),
        s => format!("{}h{:02}m", s / 3600, (s % 3600) / 60),
    }
}

/// `1730650000123 (09:06:40)` in local time.
fn format_timestamp(ms: u64) -> String {
    let local = i64::try_from(ms)
        .ok()
        .and_then(chrono::DateTime::from_timestamp_millis)
        .map(|utc| {
            utc.with_timezone(&chrono::Local)
                .format("%H:%M:%S")
                .to_string()
        })
        .unwrap_or_default();
    format!("{ms} ({local})")
}

/// Every line of a process's details, in the layout of a record's: plain
/// fields, then sections, then the full command or prompt wrapped to
/// `width`.
pub(super) fn process_detail_lines<'a>(
    process: &ProcessSummary,
    width: u16,
    theme: &Theme,
) -> Vec<Line<'a>> {
    let muted = Style::default().fg(theme.colors.muted);
    let field = |name: &str, value: String, color: Color| {
        Line::from(vec![
            Span::styled(format!("{name:<13}"), muted),
            Span::styled(value, Style::default().fg(color)),
        ])
    };
    let section = |title: &str| Line::from(Span::styled(format!("── {title} ──"), muted));
    let (_, status_color) = status_glyph(process.status, theme);
    let fg = theme.colors.foreground;
    let id_color = theme.colors.secondary;

    let mut lines = vec![
        field("process_id:", process.id.clone(), id_color),
        field("kind:", process.kind.label().to_string(), fg),
        field("status:", process.status.label().to_string(), status_color),
    ];

    lines.push(Line::from(""));
    lines.push(section("Owner"));
    lines.push(field("agent:", process.owner_name.clone(), fg));
    lines.push(field("agent_id:", process.owner_agent_id.clone(), id_color));
    for (depth, ancestor) in process.parent_chain.iter().enumerate() {
        let name = if depth == 0 { "spawned by:" } else { "" };
        lines.push(field(name, ancestor.clone(), id_color));
    }

    lines.push(Line::from(""));
    lines.push(section("Timing"));
    lines.push(field("started:", format_timestamp(process.started_ms), fg));
    if let Some(ended) = process.ended_ms {
        lines.push(field("ended:", format_timestamp(ended), fg));
    }
    let elapsed = process_elapsed_ms(process, now_ms());
    lines.push(field("duration:", format_elapsed(elapsed), fg));

    let origin = [
        ("tool_use_id:", process.tool_use_id.clone()),
        ("spawn_tx:", process.spawn_tx.clone()),
    ];
    if origin.iter().any(|(_, value)| value.is_some()) {
        lines.push(Line::from(""));
        lines.push(section("Origin"));
        for (name, value) in origin {
            if let Some(value) = value {
                lines.push(field(name, value, id_color));
            }
        }
    }

    let runtime = [
        ("pid:", process.pid.map(|pid| pid.to_string())),
        ("cwd:", process.cwd.clone()),
        ("model:", process.model.clone()),
        ("turns:", process.turns.map(|n| n.to_string())),
        ("queued:", process.queued.map(|n| n.to_string())),
        ("exit_code:", process.exit_code.map(|code| code.to_string())),
        (
            "log:",
            process.log_path.as_ref().map(|p| p.display().to_string()),
        ),
    ];
    if runtime.iter().any(|(_, value)| value.is_some()) {
        lines.push(Line::from(""));
        lines.push(section("Process"));
        for (name, value) in runtime {
            if let Some(value) = value {
                lines.push(field(name, value, fg));
            }
        }
    }

    if !process.detail.is_empty() {
        lines.push(Line::from(""));
        lines.push(Line::from(Span::styled(
            if matches!(
                process.kind,
                crate::events::ProcessKind::Shell | crate::events::ProcessKind::Command
            ) {
                "Command:"
            } else {
                "Prompt:"
            },
            muted,
        )));
        let max = usize::from(width.saturating_sub(4)).max(1);
        for line in process.detail.lines() {
            if line.is_empty() {
                lines.push(Line::from(""));
                continue;
            }
            for wrapped in wrap_words(line, max) {
                lines.push(Line::from(Span::styled(
                    format!("  {wrapped}"),
                    Style::default().fg(fg),
                )));
            }
        }
    }
    lines
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn elapsed_is_compact() {
        assert_eq!(format_elapsed(8_400), "8s");
        assert_eq!(format_elapsed(252_000), "4m12s");
        assert_eq!(format_elapsed(7_500_000), "2h05m");
    }

    #[test]
    fn detail_shows_owner_and_command() {
        let process = ProcessSummary {
            id: "cmd-1".into(),
            kind: crate::events::ProcessKind::Command,
            label: "cargo build".into(),
            detail: "cargo build --release".into(),
            owner_agent_id: "abcd".into(),
            owner_name: "Agent".into(),
            parent_chain: vec!["root".into()],
            started_ms: 1_000,
            ended_ms: Some(43_000),
            pid: Some(4242),
            exit_code: Some(0),
            status: ProcessStatus::Exited,
            ..ProcessSummary::default()
        };
        let text: Vec<String> = process_detail_lines(&process, 60, &Theme::cyber())
            .iter()
            .map(|line| line.spans.iter().map(|s| s.content.as_ref()).collect())
            .collect();
        assert!(text
            .iter()
            .any(|l| l.contains("agent_id:") && l.contains("abcd")));
        assert!(text
            .iter()
            .any(|l| l.contains("spawned by:") && l.contains("root")));
        assert!(text
            .iter()
            .any(|l| l.contains("duration:") && l.contains("42s")));
        assert!(text
            .iter()
            .any(|l| l.contains("pid:") && l.contains("4242")));
        assert!(text.iter().any(|l| l == "Command:"));
        assert!(text.iter().any(|l| l.contains("cargo build --release")));
    }
}
