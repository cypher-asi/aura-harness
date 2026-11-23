//! Input rendering: the prompt and what is being typed, at the bottom of
//! the Chat panel, growing upward as the text wraps. The Chat body's
//! bottom border is the line between them.
//!
//! ```text
//! │ ...                                  │
//! ├──────────────────────────────────────┤
//! │ →  Add a follow-up                   │
//! └──────────────────────────────────────┘
//! ```
//!
//! Typed newlines (Shift+Enter) start a new row. While a turn runs, a
//! stop button sits at the right end of the first row and the text wraps
//! before it:
//!
//! ```text
//! │ →  Add a follow-up            ■ Stop │
//! ```

use super::text::display_width;
use crate::{
    app::{AppState, HoverTarget, PanelFocus},
    App, Theme,
};
use ratatui::{
    layout::Rect,
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Paragraph},
    Frame,
};
use unicode_width::UnicodeWidthChar;

/// Rows the box's border adds to the rows of text: only the bottom one,
/// since the Chat body's bottom border sits on top of the text.
pub(super) const BOX_ROWS: u16 = 1;
/// Blank columns inside the border on each side of the text.
const PAD: u16 = 1;
/// Prompt before a chat message.
const CHAT_PROMPT: &str = "→ ";
/// Most rows the input grows to; past this it scrolls with the cursor.
const MAX_INPUT_ROWS: u16 = 8;
/// What the empty input says before the first message.
const FIRST_PLACEHOLDER: &str = "Ask anything";
/// What the empty input says once the conversation has started.
const FOLLOW_UP_PLACEHOLDER: &str = "Add a follow-up";
/// Blank columns between the cursor and the placeholder, so the cursor
/// does not sit on top of the placeholder's first letter.
const PLACEHOLDER_GAP: u16 = 1;
/// Label of the button that stops the running turn.
const STOP_LABEL: &str = "■ Stop";
/// Blank columns between the typed text and the stop button.
const STOP_GAP: u16 = 1;
/// Fewest columns the stop button leaves for typing; a narrower box
/// goes without the button (Ctrl+C still stops the turn).
const MIN_TYPING_WITH_STOP: u16 = 8;

/// Where the input goes in a box spanning `area`'s columns.
struct Plan<'a> {
    prompt: &'a str,
    prompt_x: u16,
    typing_x: u16,
    /// Rows of typed text, each at most the typing width.
    rows: Vec<Vec<Span<'static>>>,
    /// Row and column of the cursor within `rows`.
    cursor: (usize, usize),
    /// Shown in place of the text while nothing is typed.
    placeholder: Option<&'static str>,
    /// Column of the stop button, shown on the first row while a turn
    /// runs and the box has room for it.
    stop_x: Option<u16>,
}

/// Lay out the input in a box spanning `area`'s columns.
fn plan<'a>(area: Rect, app: &'a App, theme: &Theme) -> Plan<'a> {
    let input = app.input();
    let before_cursor = input.get(..app.cursor_pos()).unwrap_or(input);
    let command = app.input_is_command();
    let (prompt, display_input, display_before) = match app.state() {
        _ if command => (
            app.shell_prompt(),
            input.to_string(),
            before_cursor.to_string(),
        ),
        AppState::LoginEmail => ("Email: ", input.to_string(), before_cursor.to_string()),
        AppState::LinkCode => (
            "Code: ",
            "•".repeat(input.chars().count()),
            "•".repeat(before_cursor.chars().count()),
        ),
        AppState::LoginPassword => (
            "Password: ",
            "•".repeat(input.chars().count()),
            "•".repeat(before_cursor.chars().count()),
        ),
        _ => (CHAT_PROMPT, input.to_string(), before_cursor.to_string()),
    };
    let prompt_width = u16::try_from(display_width(prompt)).unwrap_or(u16::MAX);
    let placeholder = (input.is_empty() && prompt == CHAT_PROMPT).then(|| {
        if app.messages().is_empty() {
            FIRST_PLACEHOLDER
        } else {
            FOLLOW_UP_PLACEHOLDER
        }
    });

    let prompt_x = area.x + 1 + PAD;
    let typing_x = prompt_x.saturating_add(prompt_width);
    let full_right = (area.x + area.width).saturating_sub(1 + PAD);
    let stop_x = stop_button_x(app, typing_x, full_right);
    // The text wraps before the button, so the layout's height (from
    // `input_rows`) and the drawn rows agree while it is shown.
    let right = stop_x.map_or(full_right, |x| x.saturating_sub(STOP_GAP));
    let width = usize::from(right.saturating_sub(typing_x).max(1));

    let spans = if command {
        super::command::command_spans(&display_input, theme)
    } else {
        prose_spans(&display_input, theme)
    };
    let starts = row_starts(&display_input, width);
    let mut rows = split_rows(super::command::owned(spans), &starts);
    let cursor = wrapped_cursor(
        &display_input,
        &starts,
        display_before.chars().count(),
        width,
    );
    while rows.len() <= cursor.0 {
        rows.push(Vec::new());
    }

    Plan {
        prompt,
        prompt_x,
        typing_x,
        rows,
        cursor,
        placeholder,
        stop_x,
    }
}

/// Column where the stop button starts, right-aligned before the box's
/// right padding, while the agent on screen is busy and the box leaves
/// [`MIN_TYPING_WITH_STOP`] columns to type in beside it.
fn stop_button_x(app: &App, typing_x: u16, right: u16) -> Option<u16> {
    if !app.is_busy() {
        return None;
    }
    let label = u16::try_from(display_width(STOP_LABEL)).ok()?;
    let x = right.checked_sub(label)?;
    let typing = x.checked_sub(STOP_GAP)?.checked_sub(typing_x)?;
    (typing >= MIN_TYPING_WITH_STOP).then_some(x)
}

/// Char indices where each row of `text` starts when wrapped at `width`;
/// the first is always 0. A newline ends its row, and the next row starts
/// just after it. A word that does not fit on the current row moves to
/// the next one whole, and is only broken mid-word when it is wider than
/// a full row. Every char, whitespace and newlines included, stays on some
/// row, so a char offset maps straight to a row and column.
fn row_starts(text: &str, width: usize) -> Vec<usize> {
    let chars: Vec<(char, usize)> = text.chars().map(|c| (c, c.width().unwrap_or(0))).collect();
    let mut starts = vec![0];
    let mut used = 0;
    let mut i = 0;
    while i < chars.len() {
        let (c, w) = chars[i];
        if c == '\n' {
            starts.push(i + 1);
            used = 0;
            i += 1;
            continue;
        }
        if c.is_whitespace() {
            if used + w > width && used > 0 {
                starts.push(i);
                used = 0;
            }
            used += w;
            i += 1;
            continue;
        }
        let end = i + chars[i..]
            .iter()
            .take_while(|(c, _)| !c.is_whitespace())
            .count();
        let word: usize = chars[i..end].iter().map(|(_, w)| w).sum();
        if used > 0 && used + word > width {
            starts.push(i);
            used = 0;
        }
        for (j, &(_, w)) in chars.iter().enumerate().take(end).skip(i) {
            if used + w > width && used > 0 {
                starts.push(j);
                used = 0;
            }
            used += w;
        }
        i = end;
    }
    starts
}

/// Cut `spans` into rows at the char offsets in `starts`, one row per
/// start even when the text ends in a newline. Newlines count as chars for
/// the offsets but are not drawn: ratatui would show them as a blank cell
/// at best.
fn split_rows(spans: Vec<Span<'static>>, starts: &[usize]) -> Vec<Vec<Span<'static>>> {
    let mut rows: Vec<Vec<Span<'static>>> = vec![Vec::new()];
    let mut next = starts.iter().skip(1).peekable();
    let mut index = 0;
    for span in spans {
        for c in span.content.chars() {
            if next.peek().is_some_and(|&&start| start == index) {
                next.next();
                rows.push(Vec::new());
            }
            index += 1;
            if c == '\n' {
                continue;
            }
            if let Some(row) = rows.last_mut() {
                match row.last_mut() {
                    Some(last) if last.style == span.style => last.content.to_mut().push(c),
                    _ => row.push(Span::styled(c.to_string(), span.style)),
                }
            }
        }
    }
    rows.resize_with(starts.len().max(rows.len()), Vec::new);
    rows
}

/// Row and column of the cursor `offset` chars into `text`, wrapped at
/// the row `starts` from [`row_starts`]. A full row puts the cursor at
/// the start of the next one, unless a newline ends the row there: then
/// the next row's start is where the cursor goes after the newline, so it
/// stays in the padding column past the row's end to tell the two apart.
fn wrapped_cursor(text: &str, starts: &[usize], offset: usize, width: usize) -> (usize, usize) {
    let row = starts.iter().rposition(|&s| s <= offset).unwrap_or(0);
    let start = starts.get(row).copied().unwrap_or(0);
    let col: usize = text
        .chars()
        .skip(start)
        .take(offset.saturating_sub(start))
        .map(|c| c.width().unwrap_or(0))
        .sum();
    let before_newline = text.chars().nth(offset) == Some('\n');
    if col >= width && !before_newline {
        (row + 1, 0)
    } else {
        (row, col)
    }
}

/// Rows of text the input wants in a box spanning `area`'s columns, at
/// most `max_rows` and never fewer than one. The box adds [`BOX_ROWS`].
pub(super) fn input_rows(area: Rect, app: &App, theme: &Theme, max_rows: u16) -> u16 {
    let rows = plan(area, app, theme).rows.len();
    u16::try_from(rows)
        .unwrap_or(u16::MAX)
        .min(MAX_INPUT_ROWS)
        .min(max_rows)
        .max(1)
}

/// Render the input box into `area`, whose height is the rows of text
/// it gets (see [`input_rows`]) and the bottom border's. The row above
/// `area` is the Chat body's bottom border, already drawn. Returns where
/// the stop button was drawn, if it was.
pub(super) fn render_input(
    frame: &mut Frame,
    area: Rect,
    app: &App,
    theme: &Theme,
) -> Option<Rect> {
    if area.height < BOX_ROWS + 1 || area.width < 2 * (1 + PAD) + 2 {
        return None;
    }
    let block = Block::default()
        .borders(Borders::LEFT | Borders::RIGHT | Borders::BOTTOM)
        .border_style(Style::default().fg(super::panels::border_color(
            app.focus() == PanelFocus::Chat,
            theme,
        )));
    let block = if app.editing_queued() {
        block.title_bottom(super::queue_panel::editing_title(theme.colors.warning))
    } else {
        block
    };
    let inner = block.inner(area);
    frame.render_widget(block, area);
    join_divider(frame, area);

    let plan = plan(area, app, theme);
    let right = inner.x + inner.width;
    let prompt_width = plan.typing_x - plan.prompt_x;
    frame.render_widget(
        Paragraph::new(Line::from(prompt_spans(
            plan.prompt,
            theme.colors.muted,
            theme,
        ))),
        Rect {
            x: plan.prompt_x,
            y: inner.y,
            width: prompt_width.min(right.saturating_sub(plan.prompt_x)),
            height: 1,
        },
    );

    let typing_right = plan
        .stop_x
        .map_or(right, |x| x.saturating_sub(STOP_GAP).min(right));
    let typing_width = typing_right.saturating_sub(plan.typing_x);
    let stop = plan
        .stop_x
        .map(|x| render_stop_button(frame, x, inner, app, theme));
    if let Some(placeholder) = plan.placeholder {
        // Start one column past the cursor so its blink lands on a blank
        // cell instead of covering the placeholder's first letter.
        frame.render_widget(
            Paragraph::new(Span::styled(
                placeholder,
                Style::default().fg(theme.colors.muted),
            )),
            Rect {
                x: plan.typing_x.saturating_add(PLACEHOLDER_GAP),
                y: inner.y,
                width: typing_width.saturating_sub(PLACEHOLDER_GAP),
                height: 1,
            },
        );
    }

    // Keep the cursor's row in view once the input outgrows its rows.
    let visible = usize::from(inner.height);
    let first = (plan.cursor.0 + 1).saturating_sub(visible);
    for (i, row) in plan.rows.into_iter().skip(first).take(visible).enumerate() {
        let y = inner.y + u16::try_from(i).unwrap_or(u16::MAX);
        frame.render_widget(
            Paragraph::new(Line::from(row)),
            Rect {
                x: plan.typing_x,
                y,
                width: typing_width,
                height: 1,
            },
        );
    }

    if !app.status().contains("Thinking") {
        let (row, col) = plan.cursor;
        let y = inner.y + u16::try_from(row - first).unwrap_or(0);
        let x = plan
            .typing_x
            .saturating_add(u16::try_from(col).unwrap_or(u16::MAX));
        frame.set_cursor_position((x, y));
    }
    stop
}

/// Draw the stop button at column `x` on the box's first row and return
/// its cells. It is in the error color, and lit while the pointer is on
/// it like the other buttons.
fn render_stop_button(frame: &mut Frame, x: u16, inner: Rect, app: &App, theme: &Theme) -> Rect {
    let width = u16::try_from(display_width(STOP_LABEL)).unwrap_or(u16::MAX);
    let cell = Rect {
        x,
        y: inner.y,
        width: width.min(inner.right().saturating_sub(x)),
        height: 1,
    };
    let style = Style::default()
        .fg(theme.colors.error)
        .add_modifier(Modifier::BOLD);
    let style = if app.hover() == Some(HoverTarget::StopTurn) {
        style.bg(theme.colors.surface_hover)
    } else {
        style
    };
    frame.render_widget(Paragraph::new(Span::styled(STOP_LABEL, style)), cell);
    cell
}

/// Turn the bottom corners of the border above `area` into joints, so
/// that border reads as a line across one panel. The cells keep their
/// style, so the line stays in whatever color the body above drew it.
pub(super) fn join_divider(frame: &mut Frame, area: Rect) {
    let Some(y) = area.y.checked_sub(1) else {
        return;
    };
    let buf = frame.buffer_mut();
    let right = area.right().saturating_sub(1);
    for (x, joint) in [(area.x, "├"), (right, "┤")] {
        if let Some(cell) = buf.cell_mut((x, y)) {
            cell.set_symbol(joint);
        }
    }
}

/// Typed text in the reply color, with `@path` mentions and `[image N]`
/// chips picked out so it is clear what will be attached.
pub(super) fn prose_spans<'a>(text: &'a str, theme: &Theme) -> Vec<Span<'a>> {
    let plain = Style::default().fg(theme.colors.text);
    let chip = Style::default()
        .fg(theme.colors.foreground)
        .add_modifier(Modifier::UNDERLINED);
    let mut spans = Vec::new();
    let mut start = 0;
    let mut i = 0;
    while i < text.len() {
        let rest = &text[i..];
        let at_word_start = i == 0 || text[..i].ends_with(char::is_whitespace);
        let token_len = if at_word_start && rest.starts_with('@') && rest.len() > 1 {
            rest.find(char::is_whitespace).unwrap_or(rest.len())
        } else {
            image_chip_len(rest)
        };
        if token_len == 0 {
            i += rest.chars().next().map_or(1, char::len_utf8);
            continue;
        }
        if start < i {
            spans.push(Span::styled(&text[start..i], plain));
        }
        spans.push(Span::styled(&text[i..i + token_len], chip));
        i += token_len;
        start = i;
    }
    if start < text.len() {
        spans.push(Span::styled(&text[start..], plain));
    }
    spans
}

/// Length of an `[image N]` chip at the start of `text`, or 0.
fn image_chip_len(text: &str) -> usize {
    let Some(rest) = text.strip_prefix("[image ") else {
        return 0;
    };
    let digits = rest.chars().take_while(char::is_ascii_digit).count();
    if digits > 0 && rest[digits..].starts_with(']') {
        "[image ".len() + digits + 1
    } else {
        0
    }
}

/// `[user@host dir]$ ` with the user in the success color and `@host` muted;
/// any other prompt is drawn in `color`.
pub(super) fn prompt_spans<'a>(
    prompt: &'a str,
    color: ratatui::style::Color,
    theme: &Theme,
) -> Vec<Span<'a>> {
    let plain = || vec![Span::styled(prompt, Style::default().fg(color))];
    let Some(body) = prompt.strip_prefix('[') else {
        return plain();
    };
    let Some(at) = body.find('@') else {
        return plain();
    };
    let host_end = body[at..].find(' ').map_or(body.len(), |i| at + i);
    vec![
        Span::styled("[", Style::default().fg(color)),
        Span::styled(&body[..at], Style::default().fg(theme.colors.success)),
        Span::styled(&body[at..host_end], Style::default().fg(theme.colors.muted)),
        Span::styled(&body[host_end..], Style::default().fg(color)),
    ]
}
#[cfg(test)]
mod wrap_tests {
    use super::{row_starts, split_rows, wrapped_cursor};
    use ratatui::text::Span;

    fn rows(text: &str, width: usize) -> Vec<String> {
        let starts = row_starts(text, width);
        split_rows(vec![Span::raw(text.to_string())], &starts)
            .iter()
            .map(|row| row.iter().map(|s| s.content.as_ref()).collect())
            .collect()
    }

    #[test]
    fn words_move_to_the_next_row_whole() {
        assert_eq!(rows("hello world foo", 8), vec!["hello ", "world ", "foo"]);
        assert_eq!(rows("ab cdefg", 5), vec!["ab ", "cdefg"]);
    }

    #[test]
    fn words_wider_than_a_row_are_broken() {
        assert_eq!(rows("ab abcdefghij", 5), vec!["ab ", "abcde", "fghij"]);
    }

    #[test]
    fn cursor_follows_a_word_to_the_next_row() {
        let text = "hello world";
        let starts = row_starts(text, 8);
        assert_eq!(wrapped_cursor(text, &starts, text.len(), 8), (1, 5));
        assert_eq!(wrapped_cursor(text, &starts, 6, 8), (1, 0));
        assert_eq!(wrapped_cursor(text, &starts, 3, 8), (0, 3));
    }

    #[test]
    fn cursor_after_a_full_row_starts_the_next() {
        let text = "abcde";
        let starts = row_starts(text, 5);
        assert_eq!(wrapped_cursor(text, &starts, 5, 5), (1, 0));
    }

    #[test]
    fn newlines_start_rows_and_are_not_drawn() {
        assert_eq!(rows("ab\ncd", 10), vec!["ab", "cd"]);
        assert_eq!(rows("ab\n", 10), vec!["ab", ""]);
        assert_eq!(rows("\n\nx", 10), vec!["", "", "x"]);
        assert_eq!(rows("abc de\nf", 4), vec!["abc ", "de", "f"]);
    }

    #[test]
    fn cursor_after_a_newline_is_at_the_next_row_start() {
        let text = "ab\ncd";
        let starts = row_starts(text, 10);
        assert_eq!(wrapped_cursor(text, &starts, 2, 10), (0, 2));
        assert_eq!(wrapped_cursor(text, &starts, 3, 10), (1, 0));
        assert_eq!(wrapped_cursor(text, &starts, 5, 10), (1, 2));

        let trailing = "ab\n";
        let starts = row_starts(trailing, 10);
        assert_eq!(starts.len(), 2);
        assert_eq!(wrapped_cursor(trailing, &starts, 3, 10), (1, 0));
    }

    #[test]
    fn cursor_before_a_newline_ending_a_full_row_stays_on_it() {
        let text = "abcde\nf";
        let starts = row_starts(text, 5);
        assert_eq!(wrapped_cursor(text, &starts, 5, 5), (0, 5));
        assert_eq!(wrapped_cursor(text, &starts, 6, 5), (1, 0));
    }
}
