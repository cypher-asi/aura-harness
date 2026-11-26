//! The new-project folder browser.

use super::super::text::{centered_rect, display_width};
use crate::{
    app::{
        folder_picker::{FolderPicker, FolderRow},
        AppState,
    },
    App, Theme,
};
use ratatui::{
    style::Style,
    text::{Line, Span},
    widgets::{Block, Borders, Clear, Paragraph},
    Frame,
};
use std::path::Path;

/// Render the new-project folder browser: the directory being browsed,
/// its subfolders, and the keys that drive it.
pub(in crate::renderer) fn render_folder_picker(frame: &mut Frame, app: &mut App, theme: &Theme) {
    if app.state() != AppState::SelectingFolder {
        return;
    }
    let area = frame.area();
    let modal_width = 70.min(area.width.saturating_sub(4));
    let modal_height = 24.min(area.height.saturating_sub(2));
    let modal_area = centered_rect(modal_width, modal_height, area);
    let inner_width = usize::from(modal_width.saturating_sub(2));
    // Path line, blank line above the footer, and two footer lines.
    let fixed_rows = 4 + usize::from(
        app.folder_picker()
            .is_some_and(|p| p.new_folder().is_some()),
    );
    let visible = usize::from(modal_height.saturating_sub(2)).saturating_sub(fixed_rows);
    let scroll = app.sync_folder_picker_scroll(visible);
    let Some(picker) = app.folder_picker() else {
        return;
    };

    let path = tilde_path(picker.dir());
    let mut lines = vec![Line::from(Span::styled(
        truncate_start(&path, inner_width),
        Style::default().fg(theme.colors.secondary),
    ))];

    for (index, row) in picker.rows().iter().enumerate().skip(scroll).take(visible) {
        lines.push(folder_row_line(row, index == picker.selected(), theme));
    }
    let shown = lines.len();
    lines.extend(std::iter::repeat_n(
        Line::from(""),
        (visible + 1).saturating_sub(shown),
    ));
    push_footer(&mut lines, picker, theme);

    frame.render_widget(Clear, modal_area);
    let block = Block::default()
        .title(" Select project folder ")
        .borders(Borders::ALL)
        .border_style(Style::default().fg(theme.colors.muted));
    frame.render_widget(Paragraph::new(lines).block(block), modal_area);
}

/// One row of the browser: the cursor marker, the row's glyph and label.
fn folder_row_line<'a>(row: &'a FolderRow, selected: bool, theme: &Theme) -> Line<'a> {
    let muted = Style::default().fg(theme.colors.muted);
    let marker = Span::styled(
        if selected { "› " } else { "  " },
        Style::default().fg(theme.colors.primary),
    );
    let label_style = |color| {
        Style::default().fg(if selected {
            theme.colors.primary
        } else {
            color
        })
    };
    match row {
        FolderRow::UseThis => Line::from(vec![
            marker,
            Span::styled("✓ ", Style::default().fg(theme.colors.success)),
            Span::styled("Use this folder", label_style(theme.colors.success)),
        ]),
        FolderRow::Parent => Line::from(vec![
            marker,
            Span::styled("  ", muted),
            Span::styled("..", label_style(theme.colors.foreground)),
        ]),
        FolderRow::Folder(name) => Line::from(vec![
            marker,
            Span::styled("▰ ", muted),
            Span::styled(name.as_str(), label_style(theme.colors.foreground)),
        ]),
        FolderRow::More(hidden) => Line::from(vec![
            marker,
            Span::styled(format!("  … {hidden} more"), muted),
        ]),
    }
}

/// The new folder being named, if any, and the key hints under the rows.
fn push_footer<'a>(lines: &mut Vec<Line<'a>>, picker: &'a FolderPicker, theme: &Theme) {
    let muted = Style::default().fg(theme.colors.muted);
    if let Some(name) = picker.new_folder() {
        lines.push(Line::from(vec![
            Span::styled("New folder: ", Style::default().fg(theme.colors.primary)),
            Span::styled(name, Style::default().fg(theme.colors.foreground)),
            Span::styled("█", Style::default().fg(theme.colors.primary)),
        ]));
    }
    lines.push(Line::from(""));
    if picker.new_folder().is_some() {
        lines.push(Line::from(Span::styled("Enter create  Esc stop", muted)));
        lines.push(Line::from(""));
    } else {
        let hidden = if picker.show_hidden() { "hide" } else { "show" };
        lines.push(Line::from(Span::styled(
            "↑/↓ move  Enter/→ open  ←/Backspace up  Space pick highlighted",
            muted,
        )));
        lines.push(Line::from(Span::styled(
            format!("n new folder  ~ home  . {hidden} hidden  Esc cancel"),
            muted,
        )));
    }
}

/// `path` with the home directory shown as `~`.
fn tilde_path(path: &Path) -> String {
    match dirs::home_dir().and_then(|home| path.strip_prefix(&home).ok().map(Path::to_path_buf)) {
        Some(rest) if rest.as_os_str().is_empty() => "~".to_string(),
        Some(rest) => format!("~/{}", rest.display()),
        None => path.display().to_string(),
    }
}

/// `text` cut from the left with `…` so it fits `width` columns.
fn truncate_start(text: &str, width: usize) -> String {
    if display_width(text) <= width {
        return text.to_string();
    }
    let mut kept: Vec<char> = Vec::new();
    let mut used = 1;
    for c in text.chars().rev() {
        let w = display_width(c.encode_utf8(&mut [0; 4]));
        if used + w > width {
            break;
        }
        used += w;
        kept.push(c);
    }
    std::iter::once('…').chain(kept.into_iter().rev()).collect()
}
