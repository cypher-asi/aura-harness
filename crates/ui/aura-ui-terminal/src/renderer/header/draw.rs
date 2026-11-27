//! Drawing a header bar: its frame, tabs, info cell and buttons.

use super::{
    close_cell, ExpandButton, PanelHeader, ScopeButton, EXPAND_GLYPH, FRAMED_HEIGHT, RESTORE_GLYPH,
};
use crate::app::SideScope;
use crate::components::truncate_record_info;
use crate::renderer::text::display_width;
use crate::Theme;
use ratatui::{
    buffer::Buffer,
    layout::Rect,
    style::{Modifier, Style},
    Frame,
};

/// Draw `header` into `bar`, the area `split` gave it.
pub(in crate::renderer) fn render_panel_header<T: Copy + PartialEq>(
    frame: &mut Frame,
    bar: Rect,
    header: &PanelHeader<'_, T>,
    theme: &Theme,
) {
    if bar.width < 2 || bar.height == 0 {
        return;
    }
    let frame_style =
        Style::default().fg(super::super::chrome::border_color(header.focused, theme));
    let buf = frame.buffer_mut();
    if header.hosted {
        draw_hosted_frame(buf, bar, frame_style);
        return;
    }
    let parts = BarParts::new(bar, header);
    draw_frame(buf, &parts, frame_style);
    draw_tabs(buf, &parts, header, theme);
    draw_info_cell(buf, &parts, header, theme);
    draw_buttons(buf, &parts, theme);
}

/// Where the parts of a bar the grid draws itself fall.
struct BarParts {
    bar: Rect,
    framed: bool,
    /// The row with the labels
    middle: u16,
    /// The bar's last column
    last: u16,
    /// The column the tabs stop at
    limit: u16,
    dividers: Vec<u16>,
    close: Option<(bool, Rect)>,
    scope: Option<(ScopeButton, Rect)>,
    expand: Option<(ExpandButton, Rect)>,
}

impl BarParts {
    fn new<T: Copy>(bar: Rect, header: &PanelHeader<'_, T>) -> Self {
        let framed = bar.height >= FRAMED_HEIGHT;
        let middle = if framed { bar.y + 1 } else { bar.y };
        let last = bar.width - 1;
        let close = header.close.zip(close_cell(bar));
        let scope = header.scope.and_then(|button| Some((button, button.cell?)));
        let expand = header
            .expand
            .and_then(|button| Some((button, button.cell?)));
        let icon_tab = header.icon_tab.and_then(|icon| Some((icon, icon.cell?)));
        // A tabbed bar's buttons each sit after a divider of their own, and
        // its tabs stop at the first of those.
        let button_dividers: Vec<u16> = if header.layout.cells.is_empty() {
            Vec::new()
        } else {
            [
                icon_tab.map(|(_, cell)| cell),
                scope.map(|(_, cell)| cell),
                expand.map(|(_, cell)| cell),
            ]
            .into_iter()
            .flatten()
            .map(|cell| cell.x.saturating_sub(bar.x).saturating_sub(1))
            .filter(|&column| column > 0 && column < last)
            .collect()
        };
        let limit = button_dividers.iter().copied().min().unwrap_or(last);
        let dividers: Vec<u16> = header
            .layout
            .cells
            .iter()
            .map(|cell| cell.start + cell.width)
            .filter(|&column| column < limit)
            .chain(button_dividers.iter().copied())
            .collect();
        Self {
            bar,
            framed,
            middle,
            last,
            limit,
            dividers,
            close,
            scope,
            expand,
        }
    }
}

/// The frame's top and bottom edges, when framed, and its sides and
/// dividers on the labels' row.
fn draw_frame(buf: &mut Buffer, parts: &BarParts, frame_style: Style) {
    let BarParts {
        bar,
        last,
        ref dividers,
        ..
    } = *parts;
    if parts.framed {
        let edge = |left: &str, join: &str, right: &str| -> String {
            (0..bar.width)
                .map(|column| match column {
                    0 => left,
                    c if c == last => right,
                    c if dividers.contains(&c) => join,
                    _ => "─",
                })
                .collect()
        };
        buf.set_string(bar.x, bar.y, edge("┌", "┬", "┐"), frame_style);
        buf.set_string(bar.x, bar.y + 2, edge("└", "┴", "┘"), frame_style);
    }
    for column in [0, last].into_iter().chain(dividers.iter().copied()) {
        buf.set_string(bar.x + column, parts.middle, "│", frame_style);
    }
}

/// The tabs, cut at the first button's divider, then the icon tab and the
/// scope button.
fn draw_tabs<T: Copy + PartialEq>(
    buf: &mut Buffer,
    parts: &BarParts,
    header: &PanelHeader<'_, T>,
    theme: &Theme,
) {
    let limit = parts.limit;
    for cell in &header.layout.cells {
        if cell.start >= limit {
            break;
        }
        let width = cell.width.min(limit - cell.start);
        let style = tab_style(
            header.active == Some(cell.tab),
            header.hovered == Some(cell.tab),
            header.focused,
            theme,
        );
        let area = Rect::new(parts.bar.x + cell.start, parts.middle, width, 1);
        draw_centered(buf, area, cell.label, style);
    }
    draw_icon_tab(buf, header, theme);
    if let Some((button, cell)) = parts.scope {
        let style = tab_style(button.on, button.hovered, header.focused, theme);
        draw_centered(buf, cell, SideScope::BUTTON, style);
    }
}

/// The tab drawn as its icon, lit as the labelled tabs are.
fn draw_icon_tab<T: Copy + PartialEq>(
    buf: &mut Buffer,
    header: &PanelHeader<'_, T>,
    theme: &Theme,
) {
    let Some((icon, cell)) = header.icon_tab.and_then(|icon| Some((icon, icon.cell?))) else {
        return;
    };
    let style = tab_style(
        header.active == Some(icon.tab),
        header.hovered == Some(icon.tab),
        header.focused,
        theme,
    );
    buf.set_style(cell, style);
    // Both of the icon's columns centered, the blank one included.
    let width = u16::try_from(display_width(icon.icon)).unwrap_or(u16::MAX);
    let x = cell.x + cell.width.saturating_sub(width) / 2;
    buf.set_string(x, cell.y, icon.icon, style);
}

/// The name of `header`'s icon tab while the pointer is over it, on the
/// row under `bar` with its right end under the icon's: shaded as a
/// hovered button, over whatever the panel's body drew there, so it is
/// drawn after the body. A bar the front end draws shows its own.
pub(in crate::renderer) fn render_icon_tooltip<T: Copy + PartialEq>(
    frame: &mut Frame,
    bar: Rect,
    header: &PanelHeader<'_, T>,
    theme: &Theme,
) {
    let Some(icon) = header
        .icon_tab
        .filter(|icon| header.hovered == Some(icon.tab))
    else {
        return;
    };
    let (Some(cell), Some(name)) = (icon.cell, icon.tooltip) else {
        return;
    };
    let buf = frame.buffer_mut();
    let row = bar.bottom();
    if header.hosted || row >= buf.area.bottom() {
        return;
    }
    let text = format!(" {name} ");
    let width = u16::try_from(display_width(&text))
        .unwrap_or(u16::MAX)
        .min(cell.right().saturating_sub(bar.x));
    let tip = Rect::new(cell.right().saturating_sub(width), row, width, 1);
    let style = Style::default()
        .bg(theme.colors.surface_hover)
        .fg(theme.colors.foreground);
    draw_centered(buf, tip, &text, style);
}

/// The info cell, less the columns of the buttons beside it.
fn draw_info_cell<T>(
    buf: &mut Buffer,
    parts: &BarParts,
    header: &PanelHeader<'_, T>,
    theme: &Theme,
) {
    if let Some((start, width)) = header.layout.info {
        let buttons = u16::from(parts.close.is_some()) + u16::from(parts.expand.is_some());
        let width = width.min(parts.last - start).saturating_sub(2 * buttons);
        let area = Rect::new(parts.bar.x + start, parts.middle, width, 1);
        draw_info(buf, area, header, theme);
    }
}

/// The close and expand buttons.
fn draw_buttons(buf: &mut Buffer, parts: &BarParts, theme: &Theme) {
    let colors = &theme.colors;
    let button_style = |hovered| {
        if hovered {
            Style::default()
                .bg(colors.surface_hover)
                .fg(colors.foreground)
        } else {
            Style::default().fg(colors.muted)
        }
    };
    if let Some((hovered, cell)) = parts.close {
        draw_centered(buf, cell, "✕", button_style(hovered));
    }
    if let Some((button, cell)) = parts.expand {
        let style = button_style(button.hovered);
        let glyph = if button.expanded {
            RESTORE_GLYPH
        } else {
            EXPAND_GLYPH
        };
        buf.set_style(cell, style);
        // After the space of a padded cell; a tight cell is the glyph.
        buf.set_string(cell.x + u16::from(cell.width > 1), cell.y, glyph, style);
    }
}

/// A tab cell's style: the shown tab is bold, filled with the selection
/// color while its panel has focus; a tab under the pointer is shaded.
pub(in crate::renderer) fn tab_style(
    active: bool,
    hovered: bool,
    focused: bool,
    theme: &Theme,
) -> Style {
    let colors = &theme.colors;
    if active {
        let style = Style::default()
            .fg(colors.foreground)
            .add_modifier(Modifier::BOLD);
        if focused {
            style.bg(colors.selection).fg(colors.on_selection)
        } else {
            style
        }
    } else if hovered {
        Style::default()
            .bg(colors.surface_hover)
            .fg(colors.secondary)
    } else {
        Style::default().fg(colors.muted)
    }
}

/// The frame of a bar the front end draws: its top and sides around empty
/// rows, open at the bottom for the body's top border to close.
fn draw_hosted_frame(buf: &mut Buffer, bar: Rect, style: Style) {
    let inner = usize::from(bar.width.saturating_sub(2));
    let mut rows = bar.rows();
    if bar.height > 1 {
        if let Some(top) = rows.next() {
            buf.set_string(top.x, top.y, format!("┌{}┐", "─".repeat(inner)), style);
        }
    }
    for row in rows {
        buf.set_string(row.x, row.y, format!("│{}│", " ".repeat(inner)), style);
    }
}

/// `text` centered in the one-row `area`, which is filled with `style`.
fn draw_centered(buf: &mut Buffer, area: Rect, text: &str, style: Style) {
    buf.set_style(area, style);
    let text = truncate_record_info(text, usize::from(area.width));
    let width = u16::try_from(display_width(&text)).unwrap_or(u16::MAX);
    let x = area.x + area.width.saturating_sub(width) / 2;
    buf.set_string(x, area.y, text, style);
}

/// The info cell: the title on the left and the info on the right when
/// there is a title, otherwise the info centered, or on the right beside
/// tabs. The info shortens, then drops, before the title is cut.
fn draw_info<T>(buf: &mut Buffer, area: Rect, header: &PanelHeader<'_, T>, theme: &Theme) {
    let colors = &theme.colors;
    let room = usize::from(area.width);
    let Some(title) = header.title else {
        if let Some(info) = &header.info {
            let style = Style::default().fg(info.color);
            if header.layout.cells.is_empty() {
                let text = [&info.text, &info.short]
                    .into_iter()
                    .find(|text| display_width(text) + 2 <= room)
                    .unwrap_or(&info.short);
                draw_centered(buf, area, text, style);
            } else if let Some(text) = [&info.text, &info.short]
                .into_iter()
                .find(|text| display_width(text) + 2 <= room)
            {
                // Where an untabbed bar has it, a space in from the border.
                let width = u16::try_from(display_width(text) + 1).unwrap_or(u16::MAX);
                buf.set_string(area.x + area.width - width, area.y, text, style);
            }
        }
        return;
    };
    let title = truncate_record_info(title, room.saturating_sub(2));
    let title_style = Style::default()
        .fg(if header.focused {
            colors.primary
        } else {
            colors.foreground
        })
        .add_modifier(Modifier::BOLD);
    buf.set_string(area.x + 1, area.y, &title, title_style);
    let left = display_width(&title) + 1;
    let Some(info) = &header.info else {
        return;
    };
    let fits = |text: &str| left + 1 + display_width(text) < room;
    let Some(text) = [&info.text, &info.short]
        .into_iter()
        .find(|text| fits(text))
    else {
        return;
    };
    let width = u16::try_from(display_width(text) + 1).unwrap_or(u16::MAX);
    buf.set_string(
        area.x + area.width - width,
        area.y,
        text,
        Style::default().fg(info.color),
    );
}
