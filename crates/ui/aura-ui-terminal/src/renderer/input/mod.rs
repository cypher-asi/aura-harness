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
//! Typed newlines (Shift+Enter) start a new row. Past its most rows the
//! box follows the cursor, and the wheel over it scrolls the text until
//! the next edit or cursor move. While a turn runs, a
//! stop button sits at the right end of the first row and the text wraps
//! before it:
//!
//! ```text
//! │ →  Add a follow-up            ■ Stop │
//! ```

mod spans;
#[cfg(test)]
mod wrap_tests;

pub(super) use spans::{chat_spans, prompt_spans, prose_spans};

use super::text::display_width;
use crate::{
    app::{AppState, HoverTarget, InputRows, PanelFocus},
    input_wrap::{row_starts, wrapped_cursor},
    App, Theme,
};
use ratatui::{
    layout::Rect,
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Paragraph},
    Frame,
};

/// Rows the box's border adds to the rows of text: only the bottom one,
/// since the Chat body's bottom border sits on top of the text.
pub(super) const BOX_ROWS: u16 = 1;
/// Blank columns inside the border on each side of the text.
const PAD: u16 = 1;
/// Prompt before a chat message.
const CHAT_PROMPT: &str = "→ ";
/// Most rows the input grows to; past this it scrolls with the cursor,
/// or with the wheel until the draft or the cursor changes.
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
    /// Columns the typed text wraps at.
    width: usize,
    /// Rows of typed text, each at most the typing width.
    rows: Vec<Vec<Span<'static>>>,
    /// Row and column of the cursor within `rows`.
    cursor: (usize, usize),
    /// Shown in place of the text while nothing is typed.
    placeholder: Option<String>,
    /// Column of the stop button, shown on the first row while a turn
    /// runs and the box has room for it.
    stop_x: Option<u16>,
}

/// Lay out the input in a box spanning `area`'s columns.
fn plan<'a>(area: Rect, app: &'a App, theme: &Theme) -> Plan<'a> {
    let shape = shape(area, app);
    let spans = if shape.command {
        super::command::command_spans(&shape.display, theme)
    } else if shape.prompt == CHAT_PROMPT {
        chat_spans(&shape.display, theme)
    } else {
        prose_spans(&shape.display, theme)
    };
    let mut rows = split_rows(super::command::owned(spans), &shape.starts);
    rows.resize_with(shape.rows(), Vec::new);

    Plan {
        prompt: shape.prompt,
        prompt_x: shape.prompt_x,
        typing_x: shape.typing_x,
        width: shape.width,
        rows,
        cursor: shape.cursor,
        placeholder: shape.placeholder,
        stop_x: shape.stop_x,
    }
}

/// Where the input goes in a box spanning `area`'s columns and where its
/// text wraps, without the styled rows [`plan`] cuts from it: what the
/// layout needs to size the box.
struct Shape<'a> {
    prompt: &'a str,
    prompt_x: u16,
    typing_x: u16,
    width: usize,
    /// The text as drawn: typed, or masked for a secret
    display: String,
    /// Whether the text is a shell command
    command: bool,
    /// Char offsets where each row of `display` starts
    starts: Vec<usize>,
    cursor: (usize, usize),
    placeholder: Option<String>,
    stop_x: Option<u16>,
}

impl Shape<'_> {
    /// Rows of text: one per wrapped row, and one more when the cursor
    /// sits past a full last row.
    fn rows(&self) -> usize {
        self.starts.len().max(self.cursor.0 + 1)
    }
}

/// See [`Shape`].
fn shape(area: Rect, app: &App) -> Shape<'_> {
    let input = app.input();
    let command = app.input_is_command();
    let (prompt, display_input, display_before) = prompt_and_display(app, command);
    let prompt_width = u16::try_from(display_width(prompt)).unwrap_or(u16::MAX);
    let placeholder = placeholder(app, input, prompt);

    let prompt_x = area.x + 1 + PAD;
    let typing_x = prompt_x.saturating_add(prompt_width);
    let full_right = (area.x + area.width).saturating_sub(1 + PAD);
    let stop_x = stop_button_x(app, typing_x, full_right);
    // The text wraps before the button, so the layout's height (from
    // `input_rows`) and the drawn rows agree while it is shown.
    let right = stop_x.map_or(full_right, |x| x.saturating_sub(STOP_GAP));
    let width = usize::from(right.saturating_sub(typing_x).max(1));

    let starts = row_starts(&display_input, width);
    let cursor = wrapped_cursor(&display_input, &starts, display_before, width);

    Shape {
        prompt,
        prompt_x,
        typing_x,
        width,
        display: display_input,
        command,
        starts,
        cursor,
        placeholder,
        stop_x,
    }
}

/// The prompt, the text as drawn (masked for a secret) and how many of
/// its chars sit before the cursor.
fn prompt_and_display(app: &App, command: bool) -> (&str, String, usize) {
    let input = app.input();
    let before_cursor = input.get(..app.cursor_pos()).unwrap_or(input);
    match app.state() {
        _ if command => (
            app.shell_prompt(),
            input.to_string(),
            before_cursor.chars().count(),
        ),
        AppState::LoginEmail => ("Email: ", input.to_string(), before_cursor.chars().count()),
        AppState::LinkCode => (
            "Code: ",
            "•".repeat(input.chars().count()),
            before_cursor.chars().count(),
        ),
        AppState::LoginPassword => (
            "Password: ",
            "•".repeat(input.chars().count()),
            before_cursor.chars().count(),
        ),
        _ => (
            CHAT_PROMPT,
            input.to_string(),
            before_cursor.chars().count(),
        ),
    }
}

/// What the empty chat input says in place of the text.
fn placeholder(app: &App, input: &str, prompt: &str) -> Option<String> {
    (input.is_empty() && prompt == CHAT_PROMPT).then(|| {
        // The channel app says who a message goes to; an app showing
        // nothing says what to pick first.
        if let Some(text) = app.app_placeholder() {
            text
        } else if app.messages().is_empty() {
            FIRST_PLACEHOLDER.to_string()
        } else {
            FOLLOW_UP_PLACEHOLDER.to_string()
        }
    })
}

/// Column where the stop button starts, right-aligned before the box's
/// right padding, while the agent on screen is busy and the box leaves
/// [`MIN_TYPING_WITH_STOP`] columns to type in beside it.
fn stop_button_x(app: &App, typing_x: u16, right: u16) -> Option<u16> {
    // With no agent on screen there is no turn of its to stop.
    if !app.is_busy() || app.no_agent_on_screen() {
        return None;
    }
    let label = u16::try_from(display_width(STOP_LABEL)).ok()?;
    let x = right.checked_sub(label)?;
    let typing = x.checked_sub(STOP_GAP)?.checked_sub(typing_x)?;
    (typing >= MIN_TYPING_WITH_STOP).then_some(x)
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

/// Rows of text the input wants in a box spanning `area`'s columns, at
/// most `max_rows` and never fewer than one. The box adds [`BOX_ROWS`].
pub(super) fn input_rows(area: Rect, app: &App, max_rows: u16) -> u16 {
    let rows = shape(area, app).rows();
    u16::try_from(rows)
        .unwrap_or(u16::MAX)
        .min(MAX_INPUT_ROWS)
        .min(max_rows)
        .max(1)
}

/// What [`render_input`] drew, for the app to hit-test and scroll by.
#[derive(Debug, Default)]
pub(super) struct InputDrawn {
    /// Cells of the stop button, if it was drawn
    pub(super) stop: Option<Rect>,
    /// Columns the text wraps at, if the box was drawn
    pub(super) wrap_width: Option<usize>,
    /// Rows of text and which of them were shown, if the box was drawn
    pub(super) rows: Option<InputRows>,
}

/// Render the input box into `area`, whose height is the rows of text
/// it gets (see [`input_rows`]) and the bottom border's. The row above
/// `area` is the Chat body's bottom border, already drawn.
pub(super) fn render_input(frame: &mut Frame, area: Rect, app: &App, theme: &Theme) -> InputDrawn {
    if area.height < BOX_ROWS + 1 || area.width < 2 * (1 + PAD) + 2 {
        return InputDrawn::default();
    }
    let inner = render_box(frame, area, app, theme);

    let plan = plan(area, app, theme);
    let wrap_width = plan.width;
    let right = inner.x + inner.width;
    render_prompt(frame, &plan, inner, right, theme);

    let typing_right = plan
        .stop_x
        .map_or(right, |x| x.saturating_sub(STOP_GAP).min(right));
    let text = TextArea {
        x: plan.typing_x,
        y: inner.y,
        width: typing_right.saturating_sub(plan.typing_x),
    };
    let stop = plan
        .stop_x
        .map(|x| render_stop_button(frame, x, inner, app, theme));
    if let Some(placeholder) = plan.placeholder {
        render_placeholder(frame, placeholder, &text, theme);
    }

    let rows = shown_rows(app, plan.cursor, plan.rows.len(), inner);
    render_rows(frame, plan.rows, &rows, &text);
    place_cursor(frame, app, plan.cursor, &rows, &text);
    InputDrawn {
        stop,
        wrap_width: Some(wrap_width),
        rows: Some(rows),
    }
}

/// Where the rows of typed text go: their column, first row and width.
struct TextArea {
    x: u16,
    y: u16,
    width: u16,
}

/// Draw the box's side and bottom borders, join them to the divider
/// above, and return the cells inside.
fn render_box(frame: &mut Frame, area: Rect, app: &App, theme: &Theme) -> Rect {
    let block = Block::default()
        .borders(Borders::LEFT | Borders::RIGHT | Borders::BOTTOM)
        .border_style(Style::default().fg(super::chrome::border_color(
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
    inner
}

fn render_prompt(frame: &mut Frame, plan: &Plan<'_>, inner: Rect, right: u16, theme: &Theme) {
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
}

fn render_placeholder(frame: &mut Frame, placeholder: String, text: &TextArea, theme: &Theme) {
    // Start one column past the cursor so its blink lands on a blank
    // cell instead of covering the placeholder's first letter.
    frame.render_widget(
        Paragraph::new(Span::styled(
            placeholder,
            Style::default().fg(theme.colors.muted),
        )),
        Rect {
            x: text.x.saturating_add(PLACEHOLDER_GAP),
            y: text.y,
            width: text.width.saturating_sub(PLACEHOLDER_GAP),
            height: 1,
        },
    );
}

/// Which of the `total` rows of text are shown in `inner`.
fn shown_rows(app: &App, cursor: (usize, usize), total: usize, inner: Rect) -> InputRows {
    // Once the input outgrows its rows, show where the wheel scrolled it
    // to, or else keep the cursor's row in view.
    let visible = usize::from(inner.height);
    let first = app.input_scroll_top().map_or_else(
        || (cursor.0 + 1).saturating_sub(visible),
        |top| top.min(total.saturating_sub(visible)),
    );
    InputRows {
        top: first,
        total,
        visible,
    }
}

fn render_rows(
    frame: &mut Frame,
    rows: Vec<Vec<Span<'static>>>,
    shown: &InputRows,
    text: &TextArea,
) {
    let (first, visible) = (shown.top, shown.visible);
    for (i, row) in rows.into_iter().skip(first).take(visible).enumerate() {
        let y = text.y + u16::try_from(i).unwrap_or(u16::MAX);
        frame.render_widget(
            Paragraph::new(Line::from(row)),
            Rect {
                x: text.x,
                y,
                width: text.width,
                height: 1,
            },
        );
    }
}

fn place_cursor(
    frame: &mut Frame,
    app: &App,
    cursor: (usize, usize),
    shown: &InputRows,
    text: &TextArea,
) {
    // A cursor scrolled out of view stays hidden rather than being drawn
    // on a row it is not on.
    let (first, visible) = (shown.top, shown.visible);
    let (row, col) = cursor;
    let cursor_shown = (first..first + visible).contains(&row);
    if cursor_shown && !app.status().contains("Thinking") {
        let y = text.y + u16::try_from(row - first).unwrap_or(0);
        let x = text
            .x
            .saturating_add(u16::try_from(col).unwrap_or(u16::MAX));
        frame.set_cursor_position((x, y));
    }
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
