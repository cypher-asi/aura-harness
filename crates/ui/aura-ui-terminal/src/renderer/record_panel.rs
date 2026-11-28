//! The Kernel panel: the kernel's records, the run's members, the policy
//! and the inbox, under a KERNEL | MEMBERS | POLICIES | INBOX header bar.

use super::chrome::{render_body_block, Body, ScrollView};
use super::header::{self, HeaderInfo, PanelHeader};
use super::side_list::render_empty;
use super::text::display_width;
use crate::components::truncate_record_info;
use crate::events::{RecordStatus, RecordSummary};
use crate::{
    app::{HoverTarget, Pane, PanelFocus, RecordsTab},
    App, Theme,
};
use ratatui::{
    layout::Rect,
    style::{Modifier, Style},
    text::{Line, Span},
    Frame,
};

/// Render the Kernel panel: a KERNEL | MEMBERS | POLICIES header bar, with
/// the open tab's count or permissions after the tabs, over the kernel's
/// records, the agents of the run, or the policy the kernel enforces.
pub(super) fn render_record_panel(frame: &mut Frame, area: Rect, app: &mut App, theme: &Theme) {
    let is_focused = app.focus() == PanelFocus::Records;
    let hosted = app.host_chrome();
    let panel = area;
    let (bar, area) = header::split(area, hosted);
    if !hosted {
        app.set_record_tabs_area(bar);
    }
    let tab = app.records_tab();
    let hovered = match app.hover() {
        Some(HoverTarget::RecordTab(tab)) => Some(tab),
        _ => None,
    };
    let info = header_info(app, tab, theme);
    let header = PanelHeader {
        layout: RecordsTab::layout(bar.width),
        active: Some(tab),
        hovered,
        title: None,
        info,
        close: None,
        expand: None,
        scope: None,
        icon_tab: None,
        focused: is_focused,
        hosted,
    };
    header::show(frame, app, (Pane::Records, panel, bar), &header, theme);

    let Some(inner) = render_body_block(frame, area, (is_focused, hosted), theme) else {
        return;
    };
    if tab == RecordsTab::Members {
        super::members::render_members(frame, area, inner, app, theme);
        return;
    }
    if tab == RecordsTab::Policies {
        super::policies::render_policies(frame, area, inner, app, theme);
        return;
    }
    if tab == RecordsTab::Inbox {
        super::inbox::render_inbox(frame, area, inner, app, theme);
        return;
    }

    render_records(frame, area, inner, app, theme);
}

/// The open tab's count or permissions, shown after the tabs.
fn header_info(app: &App, tab: RecordsTab, theme: &Theme) -> Option<HeaderInfo> {
    match tab {
        RecordsTab::Kernel => Some(HeaderInfo::count(
            app.records().len(),
            "RECORDS",
            theme.colors.muted,
        )),
        RecordsTab::Members => {
            let count = app.members().len();
            let unit = if count == 1 { "AGENT" } else { "AGENTS" };
            Some(HeaderInfo::count(count, unit, theme.colors.muted))
        }
        RecordsTab::Policies => super::policies::header_info(app, theme),
        RecordsTab::Inbox => {
            let (unread, threads) = app.inbox_header_counts();
            Some(if unread > 0 {
                HeaderInfo::count(unread, "NEW", theme.colors.warning)
            } else {
                let unit = if threads == 1 { "THREAD" } else { "THREADS" };
                HeaderInfo::count(threads, unit, theme.colors.muted)
            })
        }
    }
}

/// Draw the kernel's records into `inner`, the body below the tab bar of
/// the panel bordered by `area`.
fn render_records(frame: &mut Frame, area: Rect, inner: Rect, app: &mut App, theme: &Theme) {
    let is_focused = app.focus() == PanelFocus::Records;
    let records = app.records();
    if records.is_empty() {
        render_empty_records(frame, inner, theme);
        return;
    }

    let body = Body::new(area, inner, is_focused);
    let visible = body.visible();
    let scroll = app.sync_records_scroll(visible);
    app.set_record_rows_area(body.rows());
    let lines = build_record_lines(app, body.width(), scroll, visible, theme);
    let view = ScrollView {
        total: app.records().len(),
        visible,
        first: scroll,
    };
    body.draw(frame, lines, view, theme);
}

fn render_empty_records(frame: &mut Frame, inner: Rect, theme: &Theme) {
    render_empty(frame, inner, "No records yet.", theme);
}

/// Lines `width` columns wide for the `rows` records starting at index
/// `first`.
fn build_record_lines<'a>(
    app: &App,
    width: usize,
    first: usize,
    rows: usize,
    theme: &Theme,
) -> Vec<Line<'a>> {
    let is_focused = app.focus() == PanelFocus::Records;
    let selected = app.selected_record();
    let hovered = match app.hover() {
        Some(HoverTarget::RecordRow(index)) => Some(index),
        _ => None,
    };
    let mut lines: Vec<Line> = Vec::new();

    for (i, record) in app.records().iter().enumerate().skip(first).take(rows) {
        let highlighted = i == selected && is_focused;
        lines.push(record_line(
            record,
            (highlighted, hovered == Some(i)),
            width,
            theme,
        ));
    }
    lines
}

/// One record row `width` columns wide; `(highlighted, hovered)` say
/// whether it is the focused selection and whether the mouse is over it.
fn record_line<'a>(
    record: &RecordSummary,
    (highlighted, hovered): (bool, bool),
    width: usize,
    theme: &Theme,
) -> Line<'a> {
    let line_style = if highlighted {
        Style::default()
            .fg(theme.colors.primary)
            .add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(theme.colors.muted)
    };

    let hash_style = if highlighted {
        Style::default().fg(theme.colors.secondary)
    } else {
        Style::default().fg(theme.colors.muted)
    };

    let (fixed_spans, detail) = record_spans(record, highlighted, line_style, theme);
    let used: usize = fixed_spans.iter().map(|s| display_width(&s.content)).sum();
    // One column for the separator space before the detail text.
    let available = width.saturating_sub(used + 1);
    let info_display = truncate_record_info(detail, available);

    let mut spans = fixed_spans;
    if !info_display.is_empty() {
        spans.push(Span::styled(format!(" {info_display}"), hash_style));
    }
    if hovered && !highlighted {
        let bg = theme.colors.surface_hover;
        let filled: usize = spans.iter().map(|s| display_width(&s.content)).sum();
        spans.push(Span::raw(" ".repeat(width.saturating_sub(filled))));
        for span in &mut spans {
            span.style = span.style.bg(bg);
        }
    }
    Line::from(spans)
}

/// The prefix, sequence, status and kind columns, plus the what and group
/// columns of records that have them, and the detail text to show after.
fn record_spans<'a, 'r>(
    record: &'r RecordSummary,
    highlighted: bool,
    line_style: Style,
    theme: &Theme,
) -> (Vec<Span<'a>>, &'r str) {
    let prefix = " ";
    let kind_style = if highlighted {
        line_style
    } else {
        Style::default().fg(record_kind_color(&record.tx_kind, theme))
    };

    let mut fixed_spans = vec![
        Span::styled(prefix, line_style),
        Span::styled(format!("#{:<3}", record.seq), line_style),
        status_span(record.status, highlighted, theme),
        Span::styled(fit_column(&record.tx_kind, KIND_WIDTH), kind_style),
    ];
    // Records without columns show their one-line summary after the kind.
    let detail = if record.what.is_empty() {
        &record.info
    } else {
        let is_failure = matches!(record.what.as_str(), "error" | "failed");
        let what_style = if highlighted {
            line_style
        } else if is_failure {
            Style::default().fg(theme.colors.error)
        } else {
            Style::default().fg(theme.colors.foreground)
        };
        fixed_spans.push(Span::styled(
            format!(" {}", fit_column(&record.what, WHAT_WIDTH)),
            what_style,
        ));
        fixed_spans.push(Span::styled(
            format!(" {}", fit_column(&record.group, GROUP_WIDTH)),
            Style::default().fg(theme.colors.muted),
        ));
        &record.detail
    };
    (fixed_spans, detail)
}

/// The status column: a glyph in the status's color, bold when highlighted.
fn status_span<'a>(status: RecordStatus, highlighted: bool, theme: &Theme) -> Span<'a> {
    let (status_text, status_color) = match status {
        RecordStatus::Ok => (" ✓ ", theme.colors.success),
        RecordStatus::Error => (" ✗ ", theme.colors.error),
        RecordStatus::Pending => (" ◌ ", theme.colors.pending),
        RecordStatus::None => (" · ", theme.colors.muted),
    };
    let status_style = if highlighted {
        Style::default()
            .fg(status_color)
            .add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(status_color)
    };
    Span::styled(status_text, status_style)
}

/// Widths of the Record panel's kind, what and group columns.
const KIND_WIDTH: usize = 8;
const WHAT_WIDTH: usize = 11;
const GROUP_WIDTH: usize = 7;

/// `text` upper-cased and padded or cut to exactly `width` columns.
fn fit_column(text: &str, width: usize) -> String {
    let fitted = truncate_record_info(&text.to_uppercase(), width);
    let pad = width.saturating_sub(display_width(&fitted));
    format!("{fitted}{}", " ".repeat(pad))
}

/// Color for the kind column, grouped so tool activity stands out from
/// conversation and bookkeeping rows.
fn record_kind_color(kind: &str, theme: &Theme) -> ratatui::style::Color {
    match kind {
        "ToolCall" | "ToolRes" | "Action" | "Complete" => theme.colors.secondary,
        "Prompt" | "Reply" | "Trigger" => theme.colors.primary,
        "LLM" | "Spawn" => theme.colors.pending,
        _ => theme.colors.muted,
    }
}
