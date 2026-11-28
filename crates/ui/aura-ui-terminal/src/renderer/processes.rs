//! Processes tab: every agent turn, spawned agent and background command,
//! and the detail view of one of them.

use super::agents::aura_icon;
use super::chrome::{Body, ScrollView};
use super::overlays::DETAIL_LABEL_WIDTH;
use super::side_list::{name_style, render_empty, render_hidden, Tone};
use crate::app::{HoverTarget, PanelFocus};
use crate::components::{cell_style, list_row, CellState, Info, StatusIcon, Value};
use crate::events::{ProcessKind, ProcessStatus, ProcessSummary};
use crate::{App, Theme};
use aura_core_types::time::now_ms;
use ratatui::{layout::Rect, style::Color, text::Line, Frame};

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
    if app.processes().is_empty() && app.hidden_processes() > 0 {
        render_hidden(frame, inner, ("processes", app.hidden_processes()), theme);
        return;
    }
    if app.processes().is_empty() {
        render_empty(frame, inner, "Nothing running.", theme);
        return;
    }

    let body = Body::list(app, area, inner, is_focused);
    let visible = body.visible();
    let scroll = app.sync_processes_scroll(visible);
    let hovered = match app.hover() {
        Some(HoverTarget::ProcessRow(index)) => Some(index),
        _ => None,
    };
    let selected = app.selected_process();
    let now = now_ms();
    let width = body.width();
    let lines: Vec<Line> = app
        .processes()
        .iter()
        .enumerate()
        .skip(scroll)
        .take(visible)
        .map(|(i, process)| {
            let state = CellState::of(is_focused && i == selected, hovered == Some(i));
            process_line(app, process, (state, width, now), theme)
        })
        .collect();
    let view = ScrollView {
        total: app.processes().len(),
        visible,
        first: scroll,
    };
    app.set_process_rows_area(body.rows(), scroll);
    body.draw(frame, lines, view, theme);
}

/// One list row: the status icon, the label, then the kind and elapsed
/// time; `(state, width, now)` are the row's cell state, the body width and
/// the current time in ms.
fn process_line<'a>(
    app: &App,
    process: &ProcessSummary,
    (state, width, now): (CellState, usize, u64),
    theme: &Theme,
) -> Line<'a> {
    let cell = cell_style(
        theme,
        state,
        name_style(Tone::active_if(process.status.is_live()), theme),
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
}

/// Spinning ring while running; a glyph for every other state.
pub(super) fn status_icon(app: &App, status: ProcessStatus, theme: &Theme) -> StatusIcon {
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

/// Every line of a process's details, laid out `width` columns wide like
/// a record's: what it is, who started it, when, where it came from, how
/// it runs, then its whole command or prompt.
pub(super) fn process_detail_lines(
    process: &ProcessSummary,
    width: usize,
    theme: &Theme,
) -> Vec<Line<'static>> {
    let mut info = Info::new(width, theme)
        .label_width(DETAIL_LABEL_WIDTH)
        .wrap_values();
    push_identity(&mut info, process);
    push_timing(&mut info, process);
    push_origin(&mut info, process);
    push_runtime(&mut info, process);
    push_detail(&mut info, process);
    info.into_lines()
}

/// PROCESS and OWNER: what it is and who started it.
fn push_identity(info: &mut Info<'_>, process: &ProcessSummary) {
    info.section("PROCESS", None);
    info.fact("Process id", &process.id, Value::Quiet);
    info.fact("Kind", process.kind.label(), Value::Plain);
    let failed = process.status == ProcessStatus::Failed;
    let status = if failed { Value::Error } else { Value::Plain };
    info.fact("Status", process.status.label(), status);

    info.section("OWNER", None);
    info.fact("Agent", &process.owner_name, Value::Plain);
    info.fact("Agent id", &process.owner_agent_id, Value::Quiet);
    for (depth, ancestor) in process.parent_chain.iter().enumerate() {
        let label = if depth == 0 { "Spawned by" } else { "" };
        info.fact(label, ancestor, Value::Quiet);
    }
}

/// TIMING: when it started and ended, and for how long it ran.
fn push_timing(info: &mut Info<'_>, process: &ProcessSummary) {
    info.section("TIMING", None);
    info.fact(
        "Started",
        &format_timestamp(process.started_ms),
        Value::Plain,
    );
    if let Some(ended) = process.ended_ms {
        info.fact("Ended", &format_timestamp(ended), Value::Plain);
    }
    let elapsed = process_elapsed_ms(process, now_ms());
    info.fact("Duration", &format_elapsed(elapsed), Value::Plain);
}

/// ORIGIN: the tool use and spawn transaction it came from, if any.
fn push_origin(info: &mut Info<'_>, process: &ProcessSummary) {
    let origin = [
        ("Tool use id", process.tool_use_id.as_deref()),
        ("Spawn tx", process.spawn_tx.as_deref()),
    ];
    if origin.iter().any(|(_, value)| value.is_some()) {
        info.section("ORIGIN", None);
        for (label, value) in origin {
            if let Some(value) = value {
                info.fact(label, value, Value::Quiet);
            }
        }
    }
}

/// RUNTIME: how it runs, when any of it is known.
fn push_runtime(info: &mut Info<'_>, process: &ProcessSummary) {
    let runtime = [
        ("PID", process.pid.map(|pid| pid.to_string())),
        ("Folder", process.cwd.clone()),
        ("Model", process.model.clone()),
        ("Turns", process.turns.map(|n| n.to_string())),
        ("Queued", process.queued.map(|n| n.to_string())),
        ("Exit code", process.exit_code.map(|code| code.to_string())),
        (
            "Log",
            process.log_path.as_ref().map(|p| p.display().to_string()),
        ),
    ];
    if runtime.iter().any(|(_, value)| value.is_some()) {
        info.section("RUNTIME", None);
        for (label, value) in runtime {
            if let Some(value) = value {
                info.fact(label, &value, Value::Plain);
            }
        }
    }
}

/// COMMAND or PROMPT: its whole command or prompt.
fn push_detail(info: &mut Info<'_>, process: &ProcessSummary) {
    if !process.detail.is_empty() {
        let command = matches!(process.kind, ProcessKind::Shell | ProcessKind::Command);
        info.section(if command { "COMMAND" } else { "PROMPT" }, None);
        info.text(&process.detail, Value::Plain);
    }
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
        for fact in [
            "  Agent id      abcd",
            "  Spawned by    root",
            "  Duration      42s",
            "  PID           4242",
            "COMMAND",
            "  cargo build --release",
        ] {
            assert!(text.iter().any(|l| l == fact), "{fact:?} in {text:#?}");
        }
        assert!(
            !text.iter().any(|l| l.contains('─')),
            "sections are titled, not ruled"
        );
    }
}
