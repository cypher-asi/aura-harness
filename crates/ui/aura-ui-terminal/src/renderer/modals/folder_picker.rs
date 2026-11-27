//! The new-project folder browser: the folder being browsed, its
//! subfolders, and the new folder's name while one is being named.

use super::super::text::display_width;
use super::with_notice;
use crate::app::folder_picker::FolderRow;
use crate::components::{caret_line, list_item, ListScroll, Modal, ModalArea, WIDE_WIDTH};
use crate::{App, Theme};
use ratatui::{
    style::Style,
    text::{Line, Span},
    Frame,
};
use std::path::Path;

/// Most rows of folders shown. The list keeps this height, or the room
/// the screen has, so the box keeps still as folders are opened.
const FOLDER_ROWS: usize = 16;

/// Draw the browser, if it is open, its rows scrolled to keep the
/// highlighted one in view.
pub(super) fn render(frame: &mut Frame, app: &mut App, theme: &Theme) -> Option<ModalArea> {
    let screen = frame.area();
    let modal = Modal::new("SELECT PROJECT FOLDER", theme).width(WIDE_WIDTH);
    let modal = with_notice(modal, app, theme);
    let width = modal.body_width(screen);
    let naming = app.folder_picker()?.new_folder().is_some();
    let fixed = 1 + if naming { 2 } else { 0 };
    let visible = modal
        .body_room(screen)
        .saturating_sub(fixed)
        .clamp(1, FOLDER_ROWS);
    let first = app.sync_folder_picker_scroll(visible);
    let picker = app.folder_picker()?;
    let mut lines = vec![Line::from(Span::styled(
        truncate_start(&tilde_path(picker.dir()), width),
        Style::default().fg(theme.colors.secondary),
    ))];
    for (index, row) in picker.rows().iter().enumerate().skip(first).take(visible) {
        lines.push(folder_row(row, index == picker.selected(), width, theme));
    }
    lines.resize(1 + visible, Line::default());
    if let Some(name) = picker.new_folder() {
        let lead = vec![Span::styled(
            "New folder: ",
            Style::default().fg(theme.colors.primary),
        )];
        let text = Style::default().fg(theme.colors.foreground);
        lines.push(Line::default());
        lines.push(caret_line(lead, name, name.len(), width, text, theme));
    }
    let modal = modal.scroll(ListScroll {
        top: 1,
        visible,
        total: picker.rows().len(),
        first,
    });
    Some(modal.render(frame.buffer_mut(), screen, lines))
}

/// One row of the browser: its glyph and label.
fn folder_row<'a>(row: &'a FolderRow, selected: bool, width: usize, theme: &Theme) -> Line<'a> {
    let muted = Style::default().fg(theme.colors.muted);
    let text = Style::default().fg(theme.colors.foreground);
    let spans = match row {
        FolderRow::UseThis => vec![
            Span::styled("✓ ", Style::default().fg(theme.colors.success)),
            Span::styled("Use this folder", Style::default().fg(theme.colors.success)),
        ],
        FolderRow::Parent => vec![Span::raw("  "), Span::styled("..", text)],
        FolderRow::Folder(name) => {
            vec![Span::styled("▰ ", muted), Span::styled(name.as_str(), text)]
        }
        FolderRow::More(hidden) => vec![Span::styled(format!("  … {hidden} more"), muted)],
    };
    list_item(spans, selected, width, theme)
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
