//! The conversation's rows that fall on screen, gathered from the line
//! cache and the live tail.

use super::live_tail::LiveTail;
use super::user_band::BandEdge;
use crate::renderer::chat_lines::{BlockLines, ChatLineCache};
use crate::{
    app::{ToggleRow, ToggleTarget},
    Theme,
};
use ratatui::text::Line;

/// The rows on screen, gathered from the top of the conversation down.
pub(super) struct VisibleRows {
    pub(super) lines: Vec<Line<'static>>,
    /// Rows of `lines` that are prose, with the columns of their indent
    pub(super) prose: Vec<(usize, u16)>,
    /// Rows of `lines` in a user's message, with where each falls in it
    pub(super) user_rows: Vec<(usize, BandEdge)>,
    /// Row of the conversation the next line pushed is
    line_index: usize,
    /// First row on screen
    start: usize,
    /// Most rows kept
    rows: usize,
}

/// The row below the screen, with its prose indent and its band edge.
pub(super) type NextRow = (Option<Line<'static>>, (Option<u16>, Option<BandEdge>));

impl VisibleRows {
    pub(super) fn new(start: usize, rows: usize) -> Self {
        Self {
            lines: Vec::with_capacity(rows),
            prose: Vec::new(),
            user_rows: Vec::new(),
            line_index: 0,
            start,
            rows,
        }
    }

    /// Keep `line` when it falls inside the chat viewport.
    fn push(&mut self, line: Line<'static>) {
        if self.line_index >= self.start && self.lines.len() < self.rows {
            self.lines.push(line);
        }
        self.line_index += 1;
    }

    /// Keep the rows of `cache`'s blocks that fall on screen, `gap` blank
    /// rows between two, lighting up the activity header the pointer is on.
    pub(super) fn push_blocks(
        &mut self,
        cache: &ChatLineCache,
        gap: usize,
        hovered: Option<ToggleTarget>,
        theme: &Theme,
    ) {
        for (i, block) in cache.blocks().enumerate() {
            if self.lines.len() >= self.rows {
                break;
            }
            if i > 0 {
                for _ in 0..gap {
                    self.push(Line::default());
                }
            }
            // A block above the screen only moves the ones below it down.
            if self.line_index + block.lines.len() <= self.start {
                self.line_index += block.lines.len();
                continue;
            }
            self.push_block(block, hovered, theme);
        }
    }

    fn push_block(
        &mut self,
        block: &BlockLines,
        hovered_activity: Option<ToggleTarget>,
        theme: &Theme,
    ) {
        let hovered = |offset: usize| {
            block
                .toggles
                .iter()
                .any(|toggle| toggle.line == offset && hovered_activity == Some(toggle.target))
        };
        for (offset, line) in block.lines.iter().enumerate() {
            if self.line_index >= self.start && self.lines.len() < self.rows {
                let mut line = line.clone();
                if hovered(offset) {
                    apply_activity_hover(&mut line, theme);
                }
                if let Some(Some(indent)) = block.prose.get(offset).filter(|_| line.width() > 0) {
                    self.prose.push((self.lines.len(), *indent));
                }
                if block.user {
                    let edge = BandEdge::at(offset, block.lines.len());
                    self.user_rows.push((self.lines.len(), edge));
                }
                self.lines.push(line);
            }
            self.line_index += 1;
        }
    }

    /// Keep the rows of `tail` that fall on screen, after `tail_gap` blank
    /// ones, lighting up the checklist or the working line (`tail_toggle`)
    /// when the pointer is on it.
    pub(super) fn push_tail(
        &mut self,
        tail: &LiveTail,
        (tail_gap, tail_toggle, hovered_activity): (usize, Option<ToggleRow>, Option<ToggleTarget>),
        theme: &Theme,
    ) {
        if tail_gap > 0 {
            self.push(Line::default());
        }
        let checklist_hovered = hovered_activity == Some(ToggleTarget::TodoChecklist);
        for (row, line) in tail.lines.iter().enumerate() {
            let mut line = line.clone();
            let status_hovered = tail.status_row == Some(row)
                && tail_toggle.is_some_and(|t| hovered_activity == Some(t.target));
            if status_hovered || (checklist_hovered && tail.is_checklist_label(row)) {
                apply_activity_hover(&mut line, theme);
            }
            self.push(line);
        }
    }

    /// Take out the row past the first `height`, the one below the screen,
    /// with its prose indent, at most `width`, and its band edge.
    pub(super) fn take_next(&mut self, height: usize, width: u16) -> NextRow {
        let next = (self.lines.len() > height)
            .then(|| self.lines.pop())
            .flatten();
        let next_prose = self
            .prose
            .last()
            .filter(|(row, _)| *row == height)
            .map(|&(_, indent)| indent.min(width));
        self.prose.retain(|(row, _)| *row < height);
        let next_user = self
            .user_rows
            .last()
            .filter(|(row, _)| *row == height)
            .map(|&(_, edge)| edge);
        self.user_rows.retain(|(row, _)| *row < height);
        (next, (next_prose, next_user))
    }
}

/// Hover style on an activity header. Status colors stay.
fn apply_activity_hover(line: &mut Line<'_>, theme: &Theme) {
    let status = [
        theme.colors.success,
        theme.colors.error,
        theme.colors.warning,
    ];
    for span in &mut line.spans {
        let keep_color = span.style.fg.is_some_and(|color| status.contains(&color));
        if !keep_color {
            span.style = span.style.fg(theme.colors.secondary);
        }
    }
}
