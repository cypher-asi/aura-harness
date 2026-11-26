//! A sandbox file in the detail view, with its vim cursor and footer.

use super::super::chrome::ScrollView;
use super::highlight::is_pseudo_language;
use super::source::{file_source, fit, fully_shown, last_page_start, Highlighted, Source};
use super::wrap::sanitize;
use super::{draw_frame, Drawn};
use crate::app::file_view::FileView;
use crate::app::vim::Vim;
use crate::Theme;
use ratatui::{
    layout::Rect,
    style::Style,
    text::{Line, Span},
    Frame,
};

/// Draw `file` with its cursor in view, moving `scroll` to follow it.
pub(super) fn draw_file(
    frame: &mut Frame,
    (area, rows, hosted): (Rect, Rect, bool),
    file: &mut FileView,
    scroll: &mut usize,
    theme: &Theme,
) -> Drawn {
    let syntax = file.syntax();
    let mut source = file_source(&file.body, file.extension(), file.first_line, theme);
    let vim = &mut file.vim;
    let width = usize::from(rows.width).max(1);
    let visible = usize::from(rows.height);
    let total = source.len();
    let max_scroll = last_page_start(&source, width, visible);
    let align = vim.take_align();
    let first = fit(&source, align, vim.line(), *scroll, width, visible).min(max_scroll);
    *scroll = first;
    if let Source::Code { lines, .. } = &source {
        let last = first + fully_shown(&source, first, width, visible) - 1;
        vim.keep_within(first, last, lines);
    }
    let vim = &*vim;
    if let Source::Code {
        language,
        lines,
        highlighted,
        ..
    } = &mut source
    {
        if !is_pseudo_language(language) {
            // Each source line takes at least one row, so at most
            // `visible` lines starting at `first` reach the screen.
            let range = first..first.saturating_add(visible);
            let spans = file.highlight.spans(syntax, lines, range, sanitize, theme);
            *highlighted = Highlighted {
                first,
                lines: spans,
            };
        }
    }
    let (lines, shown) = source.visible(first, width, visible, theme, Some(vim));
    let is_code = matches!(source, Source::Code { .. });
    let footer = file_footer(vim, is_code, total, file.first_line, theme);
    let view = ScrollView {
        total,
        visible: shown,
        first,
    };
    draw_frame(frame, (area, rows, hosted), footer, lines, view, theme);
    Drawn {
        max_scroll,
        shown,
        top_row: source.rows_before(first, width),
        view,
        thread_headers: Vec::new(),
    }
}

/// The footer under a file: the vim prompt or message, else the cursor
/// position and pending keys.
fn file_footer(
    vim: &Vim,
    is_code: bool,
    total: usize,
    first_line: usize,
    theme: &Theme,
) -> Line<'static> {
    let muted = Style::default().fg(theme.colors.muted);
    let mut footer = Vec::new();
    if let Some(prompt) = vim.prompt() {
        footer.push(Span::styled(
            format!(" {}", prompt.display()),
            Style::default().fg(theme.colors.text),
        ));
        footer.push(Span::styled(
            "█ ",
            Style::default().fg(theme.colors.primary),
        ));
    } else if let Some(message) = vim.message() {
        footer.push(Span::styled(
            format!(" {message} "),
            Style::default().fg(theme.colors.warning),
        ));
    } else {
        if is_code {
            footer.push(Span::styled(
                format!(
                    " Ln {}/{}, Col {} ",
                    vim.line() + first_line,
                    total + first_line - 1,
                    vim.col() + 1
                ),
                muted,
            ));
        }
        let keys = vim.pending_keys();
        if !keys.is_empty() {
            footer.push(Span::styled(
                format!("{keys} "),
                Style::default().fg(theme.colors.text),
            ));
        }
    }
    Line::from(footer)
}
