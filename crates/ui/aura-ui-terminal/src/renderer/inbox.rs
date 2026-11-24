//! The Kernel panel's INBOX tab: the threads of mail between the agents
//! of the run on screen.
//!
//! ```text
//!  ● ▾ Review parser changes (3)   Lead ↔ scout  12:04
//!        Lead → scout   Please check the tokenizer…  12:01
//!      ↩ scout → Lead   Found two issues: …          12:03
//!    ▸ Benchmark run (1)           Lead → bench  11:50
//! ```
//!
//! A thread row is the unread dot in the accent color (or a space), the
//! fold arrow, the subject (bold while unread) and its muted message
//! count, with the participants and the time of its newest message muted
//! at the right. The messages of an unfolded thread follow it, indented,
//! as `from → to`, a preview and the time each was sent.

use super::panels::{cell_style, render_scrollbar, truncate_record_info, CellState, ScrollView};
use super::side_list::{name_style, render_empty, render_hint, split_hint, Tone, INBOX_HINT};
use super::text::display_width;
use crate::app::ThreadView;
use crate::app::{HoverTarget, InboxRow, PanelFocus};
use crate::events::MailView;
use crate::{App, Theme};
use ratatui::style::{Modifier, Style};
use ratatui::text::Span;
use ratatui::{layout::Rect, text::Line, widgets::Paragraph, Frame};

/// Columns before a thread's subject: ` ● ▾ `.
const THREAD_PREFIX: usize = 5;

/// Columns before a message's names: the thread prefix, two more of
/// indent, then the reply marker or two spaces.
const MESSAGE_INDENT: &str = "     ";

/// The fewest subject columns a thread row keeps before it drops its
/// participants.
const MIN_SUBJECT: usize = 8;

/// The fewest preview columns a message row draws; fewer drop it.
const MIN_PREVIEW: usize = 4;

/// Draw the inbox rows into `inner`, the body below the tab bar of the
/// panel bordered by `area`.
pub(super) fn render_inbox(
    frame: &mut Frame,
    area: Rect,
    inner: Rect,
    app: &mut App,
    theme: &Theme,
) {
    let is_focused = app.focus() == PanelFocus::Records;
    if app.inbox_rows().is_empty() {
        render_empty(frame, inner, "No messages between agents yet.", theme);
        return;
    }

    let (rows_area, hint) = split_hint(inner, is_focused);
    let visible = usize::from(rows_area.height);
    let scroll = app.sync_inbox_scroll(visible);
    let hovered = match app.hover() {
        Some(HoverTarget::InboxRow(index)) => Some(index),
        _ => None,
    };
    let selected = app.selected_inbox_row();
    let width = usize::from(rows_area.width);
    let lines: Vec<Line> = app
        .inbox_rows()
        .iter()
        .enumerate()
        .skip(scroll)
        .take(visible)
        .map(|(i, row)| {
            let state = CellState::of(is_focused && i == selected, hovered == Some(i));
            inbox_line(row, width, state, theme)
        })
        .collect();
    let view = ScrollView {
        total: app.inbox_rows().len(),
        visible,
        first: scroll,
    };
    app.set_inbox_rows_area(rows_area, scroll);
    frame.render_widget(Paragraph::new(lines), rows_area);
    render_scrollbar(frame, area, rows_area, view, is_focused, theme);
    if let Some(hint) = hint {
        render_hint(frame, hint, INBOX_HINT, theme);
    }
}

/// The styles of one row's pieces, on the row's fill.
struct RowStyles {
    fill: Style,
    text: Style,
    muted: Style,
    accent: Style,
}

impl RowStyles {
    /// The styles of a row in `state` whose main text is `text`: the
    /// selection turns every piece its text color, as members rows do.
    fn of(theme: &Theme, state: CellState, text: Style) -> Self {
        let cell = cell_style(theme, state, text);
        let fill = cell
            .bg
            .map_or_else(Style::default, |bg| Style::default().bg(bg));
        let on_row = |color| {
            if state == CellState::Selected {
                theme.colors.on_selection
            } else {
                color
            }
        };
        Self {
            fill,
            text: fill.patch(cell.text),
            muted: fill.fg(on_row(theme.colors.muted)),
            accent: fill.fg(on_row(theme.colors.success)),
        }
    }
}

fn inbox_line(row: &InboxRow, width: usize, state: CellState, theme: &Theme) -> Line<'static> {
    match row {
        InboxRow::Thread {
            subject,
            count,
            participants,
            last_ms,
            unread,
            expanded,
            ..
        } => {
            let mut text = name_style(Tone::Active, theme);
            if *unread {
                text = text.add_modifier(Modifier::BOLD);
            }
            let thread = ThreadRow {
                subject,
                count: *count,
                participants,
                time: clock(*last_ms),
                unread: *unread,
                expanded: *expanded,
            };
            thread_row_line(&thread, width, &RowStyles::of(theme, state, text))
        }
        InboxRow::Message {
            from_name,
            to_name,
            preview,
            sent_ms,
            reply,
            ..
        } => {
            let message = MessageRow {
                names: format!("{from_name} → {to_name}"),
                preview,
                time: clock(*sent_ms),
                reply: *reply,
            };
            let text = name_style(Tone::Active, theme);
            message_row_line(&message, width, &RowStyles::of(theme, state, text))
        }
    }
}

/// What a thread row shows.
struct ThreadRow<'a> {
    subject: &'a str,
    count: usize,
    participants: &'a str,
    time: String,
    unread: bool,
    expanded: bool,
}

/// ` ● ▾ subject (3)      Lead ↔ scout  12:04 `, `width` columns wide.
/// The subject is cut to fit; a row too narrow for the participants
/// beside at least [`MIN_SUBJECT`] subject columns keeps only the time.
fn thread_row_line(row: &ThreadRow<'_>, width: usize, styles: &RowStyles) -> Line<'static> {
    let dot = if row.unread { "●" } else { " " };
    let arrow = if row.expanded { '▾' } else { '▸' };
    let count = format!(" ({})", row.count);
    let fixed = THREAD_PREFIX + display_width(&count) + 2;
    let end = [
        format!("{}  {} ", row.participants, row.time),
        format!("{} ", row.time),
    ]
    .into_iter()
    .find(|end| width >= fixed + MIN_SUBJECT + display_width(end))
    .unwrap_or_default();
    let room = width.saturating_sub(fixed + display_width(&end)).max(1);
    let subject = truncate_record_info(row.subject, room);
    let used = THREAD_PREFIX + display_width(&subject) + display_width(&count);
    let padding = width.saturating_sub(used + display_width(&end));
    Line::from(vec![
        Span::styled(" ", styles.fill),
        Span::styled(dot, styles.accent),
        Span::styled(format!(" {arrow} "), styles.muted),
        Span::styled(subject, styles.text),
        Span::styled(count, styles.muted),
        Span::styled(" ".repeat(padding), styles.fill),
        Span::styled(end, styles.muted),
    ])
}

/// What a message row of an unfolded thread shows.
struct MessageRow<'a> {
    names: String,
    preview: &'a str,
    time: String,
    reply: bool,
}

/// `     ↩ scout → Lead   Found two issues: …   12:03 `, `width` columns
/// wide. The names are cut first to leave the time; the preview takes
/// what is left, and is dropped below [`MIN_PREVIEW`] columns.
fn message_row_line(row: &MessageRow<'_>, width: usize, styles: &RowStyles) -> Line<'static> {
    let marker = if row.reply { "↩ " } else { "  " };
    let indent = display_width(MESSAGE_INDENT) + display_width(marker);
    let end = format!("  {} ", row.time);
    let names_room = width.saturating_sub(indent + display_width(&end)).max(1);
    let names = truncate_record_info(&row.names, names_room);
    let rest = width.saturating_sub(indent + display_width(&names) + display_width(&end));
    let preview = rest
        .checked_sub(3)
        .filter(|room| *room >= MIN_PREVIEW)
        .map(|room| truncate_record_info(row.preview, room))
        .filter(|preview| !preview.is_empty())
        .map(|preview| format!("   {preview}"))
        .unwrap_or_default();
    let padding = rest.saturating_sub(display_width(&preview));
    Line::from(vec![
        Span::styled(MESSAGE_INDENT, styles.fill),
        Span::styled(marker, styles.muted),
        Span::styled(names, styles.muted),
        Span::styled(preview, styles.text),
        Span::styled(" ".repeat(padding), styles.fill),
        Span::styled(end, styles.muted),
    ])
}

/// Every line of the thread view, and the source line of each message's
/// header with the message's id.
///
/// ```text
/// ▸ Lead → scout · 12:01   Please check the tokenizer…
/// ▾ scout → Lead · 12:03 · reply
///   Found two issues:
/// ```
///
/// A folded message is its header and a preview on one line cut to
/// `width`; an unfolded one is its header and its text indented two
/// columns. One blank line parts messages, the layout
/// `App::open_thread` scrolls by.
pub(super) fn thread_lines(
    app: &App,
    view: &ThreadView,
    width: u16,
    theme: &Theme,
) -> (Vec<Line<'static>>, Vec<(usize, u64)>) {
    let width = usize::from(width);
    let mut lines = Vec::new();
    let mut headers = Vec::new();
    for (index, mail) in app.thread_messages(&view.thread_id).into_iter().enumerate() {
        if index > 0 {
            lines.push(Line::default());
        }
        headers.push((lines.len(), mail.id));
        let unfolded = view.unfolded.contains(&mail.id);
        let selected = view.selected == Some(mail.id);
        lines.push(message_header(mail, unfolded, selected, width, theme));
        if unfolded {
            let body = Style::default().fg(theme.colors.foreground);
            lines.extend(mail.text.lines().map(|line| {
                Line::from(Span::styled(
                    format!("  {}", line.replace('\t', "    ")),
                    body,
                ))
            }));
        }
    }
    (lines, headers)
}

/// `▾ from → to · HH:MM · reply`, highlighted when `selected`, and a
/// muted preview after it when folded, all cut to `width`.
fn message_header(
    mail: &MailView,
    unfolded: bool,
    selected: bool,
    width: usize,
    theme: &Theme,
) -> Line<'static> {
    let arrow = if unfolded { '▾' } else { '▸' };
    let reply = if mail.reply { " · reply" } else { "" };
    let header = format!(
        "{arrow} {} → {} · {}{reply}",
        mail.from_name,
        mail.to_name,
        clock(mail.sent_ms)
    );
    let header = truncate_record_info(&header, width);
    let style = if selected {
        Style::default()
            .fg(theme.colors.on_selection)
            .bg(theme.colors.selection)
            .add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(theme.colors.text)
    };
    let room = width.saturating_sub(display_width(&header) + 3);
    let mut spans = vec![Span::styled(header, style)];
    if !unfolded && room > 0 {
        let preview = truncate_record_info(&mail.text, room);
        if !preview.is_empty() {
            spans.push(Span::raw("   "));
            spans.push(Span::styled(
                preview,
                Style::default().fg(theme.colors.muted),
            ));
        }
    }
    Line::from(spans)
}

/// `HH:MM` of `ms`, Unix milliseconds, in local time.
fn clock(ms: u64) -> String {
    i64::try_from(ms)
        .ok()
        .and_then(chrono::DateTime::from_timestamp_millis)
        .map(|utc| {
            utc.with_timezone(&chrono::Local)
                .format("%H:%M")
                .to_string()
        })
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use crate::events::{AgentSummary, MailView, UiCommand};
    use crate::{App, Theme};
    use crate::{PaneTab, RecordsTab};
    use crossterm::event::{
        KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind,
    };
    use ratatui::{backend::TestBackend, Terminal};

    fn render_rows(app: &mut App) -> Vec<String> {
        let mut terminal = Terminal::new(TestBackend::new(240, 40)).unwrap();
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

    /// Left-click the first cell of `text` on screen.
    fn click(app: &mut App, rows: &[String], text: &str) {
        let (column, row) = rows
            .iter()
            .enumerate()
            .find_map(|(y, row)| row.find(text).map(|x| (row[..x].chars().count(), y)))
            .unwrap_or_else(|| panic!("{text} not on screen"));
        app.handle_mouse(MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: u16::try_from(column).unwrap(),
            row: u16::try_from(row).unwrap(),
            modifiers: KeyModifiers::NONE,
        });
    }

    fn mail(id: u64, from: &str, to: &str, text: &str) -> MailView {
        MailView {
            id,
            thread_id: "t1".into(),
            subject: "Tokenizer review".into(),
            from_id: from.to_lowercase(),
            from_name: from.into(),
            to_id: to.to_lowercase(),
            to_name: to.into(),
            reply: id > 1,
            text: text.into(),
            sent_ms: 1_700_000_000_000 + id,
        }
    }

    #[test]
    fn the_inbox_bar_counts_unread_threads() {
        let mut app = app_with_two_threads();
        app.select_pane_tab(PaneTab::Records(RecordsTab::Inbox));
        let screen = render_rows(&mut app).join("\n");
        assert!(screen.contains("INBOX"), "{screen}");
        assert!(screen.contains("2 NEW │"), "{screen}");
        assert!(screen.contains("⏎ open"), "the focused tab shows its keys");
    }

    #[test]
    fn thread_rows_show_their_dot_arrow_subject_count_and_participants() {
        let mut app = app_with_two_threads();
        app.select_pane_tab(PaneTab::Records(RecordsTab::Inbox));
        let rows = render_rows(&mut app);
        let thread = rows
            .iter()
            .find(|row| row.contains("Tokenizer review"))
            .unwrap();
        for part in ["● ▸ Tokenizer review (2)", "Lead ↔ Scout"] {
            assert!(thread.contains(part), "{part} in {thread}");
        }
        let other = rows.iter().find(|row| row.contains("Benchmark")).unwrap();
        assert!(
            other.contains("(1)") && other.contains("Lead → Bench"),
            "{other}"
        );
    }

    #[test]
    fn unfolding_a_thread_lists_its_messages_under_it() {
        let mut app = app_with_two_threads();
        app.select_pane_tab(PaneTab::Records(RecordsTab::Inbox));
        app.handle_key(KeyEvent::from(KeyCode::Right));
        let rows = render_rows(&mut app);
        let screen = rows.join("\n");
        assert!(screen.contains("▾ Tokenizer review (2)"), "{screen}");
        let first = rows
            .iter()
            .find(|row| row.contains("Lead → Scout   Please check"))
            .unwrap_or_else(|| panic!("{screen}"));
        assert!(first.contains("       Lead → Scout"), "indented: {first}");
        let reply = rows
            .iter()
            .find(|row| row.contains("Scout → Lead"))
            .unwrap_or_else(|| panic!("{screen}"));
        assert!(
            reply.contains("↩ Scout → Lead   Found two issues:"),
            "{reply}"
        );
    }

    #[test]
    fn an_empty_inbox_says_so() {
        let mut app = App::new();
        app.select_pane_tab(PaneTab::Records(RecordsTab::Inbox));
        let screen = render_rows(&mut app).join("\n");
        assert!(
            screen.contains("No messages between agents yet."),
            "{screen}"
        );
        assert!(screen.contains("0 THREADS │"), "{screen}");
    }

    /// Lead on screen with thread t1 (two messages, the second a reply)
    /// and thread t2 (one message to Bench), both unread.
    fn app_with_two_threads() -> App {
        let mut app = App::new();
        app.process_command(UiCommand::SetAgents(vec![AgentSummary {
            id: "lead".into(),
            name: "Lead".into(),
            ..AgentSummary::default()
        }]));
        app.process_command(UiCommand::SetActiveAgent("lead".into()));
        for message in [
            mail(1, "Lead", "Scout", "Please check the tokenizer"),
            mail(2, "Scout", "Lead", "Found two issues:\n1. spaces"),
            MailView {
                thread_id: "t2".into(),
                subject: "Benchmark run".into(),
                // Older than t1, so t1 lists first and is highlighted.
                sent_ms: 1_600_000_000_000,
                ..mail(3, "Lead", "Bench", "Run the suite")
            },
        ] {
            app.process_command(UiCommand::AddMail(message));
        }
        app
    }

    #[test]
    fn thread_view_folds_older_messages_and_unfolds_the_newest() {
        let mut app = App::new();
        app.process_command(UiCommand::SetAgents(vec![AgentSummary {
            id: "lead".into(),
            name: "Lead".into(),
            ..AgentSummary::default()
        }]));
        app.process_command(UiCommand::SetActiveAgent("lead".into()));
        app.process_command(UiCommand::AddMail(mail(
            1,
            "Lead",
            "Scout",
            "Please check the tokenizer",
        )));
        app.process_command(UiCommand::AddMail(mail(
            2,
            "Scout",
            "Lead",
            "Found two issues:\n1. spaces",
        )));
        app.select_pane_tab(PaneTab::Records(RecordsTab::Inbox));
        let rows = render_rows(&mut app);
        click(&mut app, &rows, "Tokenizer review");

        let rows = render_rows(&mut app);
        let screen = rows.join("\n");
        assert!(screen.contains("THREAD Tokenizer review"), "{screen}");
        assert!(screen.contains("2 MESSAGES"), "{screen}");
        assert!(
            rows.iter().any(|row| row.contains("▸ Lead → Scout · ")
                && row.contains("Please check the tokenizer")),
            "{screen}"
        );
        assert!(screen.contains("▾ Scout → Lead · "), "{screen}");
        assert!(screen.contains(" · reply"), "{screen}");
        assert!(screen.contains("  Found two issues:"), "{screen}");
        assert!(screen.contains("  1. spaces"), "{screen}");
        assert!(screen.contains("n/p message"), "{screen}");

        // The drawn headers map clicks: the first header unfolds.
        click(&mut app, &rows, "▸ Lead");
        let screen = render_rows(&mut app).join("\n");
        assert!(screen.contains("▾ Lead → Scout"), "{screen}");
        assert!(screen.contains("  Please check the tokenizer"), "{screen}");
    }
}
