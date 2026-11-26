//! One row of a list panel: an icon column, a name cut to fit with an
//! optional badge and detail, and a time or code at the right end, on
//! the row's selected, hovered or plain colors.

use super::{AgentButton, AgentRowEnd};
use crate::animation::AURA_WIDTH;
use crate::themes::Theme;
use ratatui::{
    style::{Color, Style},
    text::{Line, Span},
};
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

fn display_width(text: &str) -> usize {
    UnicodeWidthStr::width(text)
}

/// What sits in the icon column of a projects-list row.
pub struct StatusIcon {
    glyph: String,
    color: Color,
}

impl StatusIcon {
    pub fn new(glyph: &str, color: Color) -> Self {
        Self {
            glyph: glyph.to_string(),
            color,
        }
    }
}

/// ` icon name      time `, filled with the row's background (if it has
/// one) from one panel border to the other. The icon column is wide enough
/// for the aura ring, so names line up whatever icon a row shows. `depth`
/// indents the row two columns per level.
pub fn list_row<'a>(
    width: usize,
    depth: usize,
    icon: StatusIcon,
    name: &str,
    cell: CellStyle,
    time: Option<&str>,
) -> Line<'a> {
    list_row_with_detail(width, depth, icon, (name, None), cell, time)
}

/// [`list_row`] with an optional `detail` drawn muted after the name. The
/// detail is cut first when the row is too narrow, and dropped when fewer
/// than four columns are left for it.
pub fn list_row_with_detail<'a>(
    width: usize,
    depth: usize,
    icon: StatusIcon,
    (name, detail): (&str, Option<&str>),
    cell: CellStyle,
    time: Option<&str>,
) -> Line<'a> {
    list_row_ending(
        width,
        depth,
        icon,
        (name, detail),
        cell,
        &AgentRowEnd::time(time),
    )
}

/// [`list_row_with_detail`] ending in `end` instead of a time, as roster
/// rows end in their abbreviation.
pub fn list_row_ending<'a>(
    width: usize,
    depth: usize,
    icon: StatusIcon,
    (name, detail): (&str, Option<&str>),
    cell: CellStyle,
    end: &AgentRowEnd<'_>,
) -> Line<'a> {
    let fill = cell
        .bg
        .map_or_else(Style::default, |bg| Style::default().bg(bg));
    let indent = Span::styled(format!(" {}", "  ".repeat(depth)), fill);
    let label = RowLabel {
        name,
        badge: None,
        detail,
    };
    indented_row(width, indent, icon, label, cell, end, &[])
}

/// [`list_row_with_detail`] led by `fold`, a fold arrow or a blank, drawn
/// in its own color before the icon, as the benchmark timeline leads its
/// rows: ` ▾ ✓  name  detail   time `.
pub fn fold_row<'a>(
    width: usize,
    (fold, color): (&str, Color),
    icon: StatusIcon,
    (name, detail): (&str, Option<&str>),
    cell: CellStyle,
    time: Option<&str>,
) -> Line<'a> {
    let fill = cell
        .bg
        .map_or_else(Style::default, |bg| Style::default().bg(bg));
    let lead = Span::styled(format!(" {fold} "), fill.fg(cell.icon.unwrap_or(color)));
    let label = RowLabel {
        name,
        badge: None,
        detail,
    };
    indented_row(
        width,
        lead,
        icon,
        label,
        cell,
        &AgentRowEnd::time(time),
        &[],
    )
}

/// [`list_row`] for an agent in a project's tree: indented by `guides`,
/// the lines from the agent that spawned it, drawn in their color. A
/// spawn with no icon has its elbow run on through the icon column. The
/// `badge`, if any, is drawn right after the name (see [`RowBadge`]). The
/// `buttons`, each with its color, are drawn left of `end`, the time,
/// where `agent_button_offset` says they are.
pub fn tree_row<'a>(
    width: usize,
    (guides, color): (&str, Color),
    icon: StatusIcon,
    (name, badge): (&str, Option<RowBadge<'_>>),
    mut cell: CellStyle,
    end: &AgentRowEnd<'_>,
    buttons: &[(AgentButton, Color)],
) -> Line<'a> {
    let fill = cell
        .bg
        .map_or_else(Style::default, |bg| Style::default().bg(bg));
    let icon = if icon.glyph.is_empty() && !guides.is_empty() {
        cell.icon = None;
        StatusIcon::new(&"─".repeat(AURA_WIDTH), color)
    } else {
        icon
    };
    let indent = Span::styled(format!(" {guides}"), fill.fg(color));
    let label = RowLabel {
        name,
        badge,
        detail: None,
    };
    indented_row(width, indent, icon, label, cell, end, buttons)
}

/// What an agent row says after its name when the agent is in plan mode.
pub const PLAN_BADGE: &str = " (P)";

/// The fewest name columns a row keeps beside its badge; narrower rows
/// drop the badge instead.
const MIN_NAME_BESIDE_BADGE: usize = 2;

/// A short marker drawn right after a row's name in its own color, which
/// it keeps on selected and hovered rows. The name is cut to make room for
/// it, so it stays visible after a long name.
#[derive(Debug, Clone, Copy)]
pub struct RowBadge<'a> {
    pub text: &'a str,
    pub color: Color,
}

/// The text of a projects-list row: its name, then an optional badge and
/// an optional muted detail.
#[derive(Debug, Clone, Copy)]
struct RowLabel<'a> {
    name: &'a str,
    badge: Option<RowBadge<'a>>,
    detail: Option<&'a str>,
}

fn indented_row<'a>(
    width: usize,
    indent: Span<'a>,
    icon: StatusIcon,
    label: RowLabel<'_>,
    cell: CellStyle,
    end: &AgentRowEnd<'_>,
    buttons: &[(AgentButton, Color)],
) -> Line<'a> {
    let fill = cell
        .bg
        .map_or_else(Style::default, |bg| Style::default().bg(bg));
    let icon_width = display_width(&icon.glyph);
    let icon_pad = AURA_WIDTH.saturating_sub(icon_width) + 1;
    // Buttons sit just left of the time and code, laid out from the same
    // pieces as `agent_button_offset`. Rows without buttons keep the
    // same time and code, so neither moves when the pointer comes.
    let time = end.time_cell(!buttons.is_empty());
    let code = end.code_cell();
    let buttons: Vec<Span<'a>> = buttons
        .iter()
        .map(|&(button, color)| Span::styled(button.label(), fill.fg(color)))
        .collect();
    let button_width: usize = buttons.iter().map(Span::width).sum();
    let text_width = width.saturating_sub(
        indent.width()
            + icon_width
            + icon_pad
            + button_width
            + display_width(&time)
            + display_width(&code),
    );
    // The badge's columns come out of the name's, so a long name is cut
    // and the badge still shows; a row too narrow for both drops it. A row
    // narrower than its fixed parts has no name columns at all.
    let badge = label
        .badge
        .filter(|badge| text_width >= display_width(badge.text) + MIN_NAME_BESIDE_BADGE);
    let badge_width = badge.map_or(0, |badge| display_width(badge.text));
    let name_width = text_width.saturating_sub(badge_width);
    let name = truncate_agent_name(label.name, name_width);
    let rest = name_width.saturating_sub(display_width(&name));
    let detail = label
        .detail
        .filter(|detail| !detail.is_empty() && rest >= 4)
        .map(|detail| format!("  {}", truncate_record_info(detail, rest - 2)))
        .unwrap_or_default();
    let padding = rest.saturating_sub(display_width(&detail));
    let icon_color = cell.icon.unwrap_or(icon.color);
    let mut spans = vec![
        indent,
        Span::styled(icon.glyph, fill.fg(icon_color)),
        Span::styled(" ".repeat(icon_pad), fill),
        Span::styled(name, fill.patch(cell.text)),
    ];
    if let Some(badge) = badge {
        spans.push(Span::styled(badge.text.to_string(), fill.fg(badge.color)));
    }
    spans.extend([
        Span::styled(detail, fill.fg(cell.time)),
        Span::styled(" ".repeat(padding), fill),
    ]);
    spans.extend(buttons);
    spans.push(Span::styled(time, fill.fg(cell.time)));
    // The code keeps the time's color on the row's fill, so it stays on
    // selected and hovered rows. No span at all without a code column.
    if !code.is_empty() {
        spans.push(Span::styled(code, fill.fg(cell.time)));
    }
    Line::from(spans)
}

/// How a list cell is drawn, from strongest to weakest.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CellState {
    /// The keyboard cursor while the panel is focused, otherwise the agent
    /// shown in the chat.
    Selected,
    /// Under the pointer.
    Hovered,
    Normal,
}

impl CellState {
    pub const fn of(selected: bool, hovered: bool) -> Self {
        if selected {
            Self::Selected
        } else if hovered {
            Self::Hovered
        } else {
            Self::Normal
        }
    }
}

/// Colors for one list row. `icon` overrides the icon's own color.
#[derive(Debug, Clone, Copy)]
pub struct CellStyle {
    pub bg: Option<Color>,
    pub text: Style,
    icon: Option<Color>,
    pub time: Color,
}

/// Rows sit on the panel background, turn a subtle gray under the pointer,
/// and turn the selection color with dark text when selected. `text` is the row's name style when nothing else applies.
pub fn cell_style(theme: &Theme, state: CellState, text: Style) -> CellStyle {
    let colors = &theme.colors;
    let plain = |text: Style| CellStyle {
        bg: None,
        text,
        icon: None,
        time: colors.muted,
    };
    match state {
        CellState::Selected => CellStyle {
            bg: Some(colors.selection),
            text: text.fg(colors.on_selection),
            icon: Some(colors.on_selection),
            time: colors.on_selection,
        },
        CellState::Hovered => CellStyle {
            bg: Some(colors.surface_hover),
            ..plain(text)
        },
        CellState::Normal => plain(text),
    }
}

/// `now`, `5m`, `3h`, `2d`, `6w`: how long before `now_ms` `then_ms` was.
pub fn format_ago(then_ms: u64, now_ms: u64) -> String {
    const MINUTE: u64 = 60;
    const HOUR: u64 = 60 * MINUTE;
    const DAY: u64 = 24 * HOUR;
    const WEEK: u64 = 7 * DAY;
    let secs = now_ms.saturating_sub(then_ms) / 1000;
    match secs {
        s if s < MINUTE => "now".to_string(),
        s if s < HOUR => format!("{}m", s / MINUTE),
        s if s < DAY => format!("{}h", s / HOUR),
        s if s < WEEK => format!("{}d", s / DAY),
        s => format!("{}w", s / WEEK),
    }
}

/// The agent's name cut to `max_width` columns, ending in `…` when cut.
/// No room at all means no name, not even the ellipsis, so a row narrower
/// than its fixed parts is never wider than the row.
fn truncate_agent_name(name: &str, max_width: usize) -> String {
    if max_width == 0 {
        return String::new();
    }
    if display_width(name) <= max_width {
        return name.to_string();
    }
    let mut truncated = String::new();
    let mut width = 0;
    for c in name.chars() {
        let char_width = c.width().unwrap_or(1);
        if width + char_width >= max_width {
            break;
        }
        truncated.push(c);
        width += char_width;
    }
    format!("{truncated}…")
}

/// Fit `info` into `max_width` display columns on a single line.
///
/// Control characters (including newlines) are folded into spaces so a
/// row can never spill onto a second line, and truncation is measured in
/// display width so wide (CJK/emoji) characters never split mid-glyph.
pub fn truncate_record_info(info: &str, max_width: usize) -> String {
    let mut single_line = String::with_capacity(info.len());
    let mut last_was_space = true;
    for c in info.chars() {
        if c.is_whitespace() || c.is_control() {
            if !last_was_space {
                single_line.push(' ');
                last_was_space = true;
            }
        } else {
            single_line.push(c);
            last_was_space = false;
        }
    }
    let single_line = single_line.trim_end();

    if max_width == 0 || single_line.is_empty() {
        return String::new();
    }
    if display_width(single_line) <= max_width {
        return single_line.to_string();
    }

    let budget = max_width - 1;
    let mut truncated = String::new();
    let mut width = 0;
    for c in single_line.chars() {
        let char_width = c.width().unwrap_or(0);
        if width + char_width > budget {
            break;
        }
        truncated.push(c);
        width += char_width;
    }
    format!("{}…", truncated.trim_end())
}
