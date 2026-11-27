//! `@` picker, drawn over the bottom of the Chat panel like the slash
//! menu so it sits directly above the input line: a channel's members,
//! then files and folders.

use crate::app::mentions::MentionCandidate;
use crate::components::{list_item, popup_block};
use crate::{App, Theme};
use ratatui::{
    layout::Rect,
    style::Style,
    text::{Line, Span},
    widgets::{Clear, Paragraph},
    Frame,
};

/// Most matches listed at once; the list scrolls past this.
const MAX_ROWS: usize = 8;

/// Render the `@` picker across the full width of `chat`, bottom-aligned
/// with it. Draws nothing while the picker is closed.
pub(super) fn render_mention_menu(frame: &mut Frame, chat: Rect, app: &App, theme: &Theme) {
    let matches = app.mention_matches();
    if matches.is_empty() || chat.height < 3 {
        return;
    }
    let height = u16::try_from(matches.len().min(MAX_ROWS))
        .unwrap_or(u16::MAX)
        .saturating_add(2)
        .min(chat.height);
    let area = Rect {
        y: chat.bottom() - height,
        height,
        ..chat
    };
    let rows = usize::from(height - 2);
    let cursor = app.mention_cursor();
    let scroll = (cursor + 1).saturating_sub(rows);
    let width = usize::from(area.width.saturating_sub(2));

    let lines: Vec<Line<'_>> = matches
        .iter()
        .enumerate()
        .skip(scroll)
        .take(rows)
        .map(|(idx, candidate)| candidate_line(candidate, idx == cursor, width, theme))
        .collect();

    let title = if matches.iter().any(|candidate| candidate.member.is_some()) {
        "MEMBERS · FILES"
    } else {
        "FILES"
    };
    let hint = "↑/↓ select  Tab complete  Enter insert  Esc close";
    let block = popup_block(Some(title), Some(hint), theme);
    frame.render_widget(Clear, area);
    frame.render_widget(Paragraph::new(lines).block(block), area);
}

/// One match, `width` columns: `@path`, and a member's name and role.
fn candidate_line<'a>(
    candidate: &MentionCandidate,
    selected: bool,
    width: usize,
    theme: &Theme,
) -> Line<'a> {
    let color = if candidate.is_dir {
        theme.colors.secondary
    } else {
        theme.colors.foreground
    };
    let mut spans = vec![Span::styled(
        format!("@{}", candidate.path),
        Style::default().fg(color),
    )];
    if let Some(member) = &candidate.member {
        spans.push(Span::raw(format!("  {}", member.name)));
        spans.push(Span::styled(
            format!("  {}", member.role),
            Style::default().fg(theme.colors.muted),
        ));
    }
    list_item(spans, selected, width, theme)
}
