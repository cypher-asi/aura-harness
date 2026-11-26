//! Cards on the canvas: their frames, heads, rows and reports.

use super::{fit, Colors, Pen, GREEK_BELOW};
use crate::geom::Color;
use crate::scene::fill;
use crate::text::Align;
use aura_ui::events::{FlowKind, TraceNodeStatus};
use aura_ui_terminal::flow_layout::CellRect;
use std::collections::HashSet;
use unicode_width::UnicodeWidthStr;

/// How a card stands to the selection.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Standing {
    Selected,
    /// A wire joins it to the selected card
    Linked,
    Other,
}

impl Pen<'_, '_> {
    pub(super) fn card(&mut self, card: usize, selected: Option<usize>, linked: &HashSet<usize>) {
        let rect = self.layout.card_rect(card);
        let standing = if selected == Some(card) {
            Standing::Selected
        } else if linked.contains(&card) {
            Standing::Linked
        } else {
            Standing::Other
        };
        let colors = self.colors();
        let flow = self.canvas.flow;
        let placed = &self.layout.cards[card];

        // From the middle of the side borders' cells, and from the top of
        // the title row to the middle of the bottom border's.
        let left = self.x(rect.x as f32 + 0.5).round();
        let right = self.x(rect.right() as f32 - 0.5).round();
        let (top, bottom) = (self.card_top(rect), self.card_bottom(rect));
        let edge = self.line();
        self.card_frame(standing, (left, right), (top, bottom), edge);

        self.title(card, rect);
        self.report(card, rect);

        let selected_place = flow.selected_place().filter(|(at, _)| *at == card);
        let head_selected = selected_place == Some((card, None));
        let line_top = |line: usize| rect.y as f32 + line as f32;
        if head_selected {
            self.highlight(left + edge, right - edge, line_top(1));
        }
        self.card_head(card, rect, head_selected);

        let shown = placed.shown_rows();
        for row in 0..placed.lines() {
            let line = placed.first_row_line() + row;
            if row == shown {
                match flow.last_line_row(card) {
                    Some(hidden) => self.row(card, hidden, line, true, (left + edge, right - edge)),
                    None => {
                        let more = flow.more_label(card);
                        self.write(
                            &more,
                            (rect.x as f32 + 2.0, line_top(line)),
                            colors.muted,
                            false,
                            Align::Left,
                        );
                    }
                }
                continue;
            }
            let is_selected = selected_place == Some((card, Some(row)));
            self.row(card, row, line, is_selected, (left + edge, right - edge));
        }
    }

    /// The background and the border of a card, its border colored by
    /// where it stands to the selection.
    fn card_frame(
        &mut self,
        standing: Standing,
        (left, right): (f32, f32),
        (top, bottom): (f32, f32),
        edge: f32,
    ) {
        let colors = self.colors();
        let border = match standing {
            Standing::Selected => colors.primary,
            Standing::Linked => colors.secondary.mix(colors.bg, 0.3),
            Standing::Other => colors.muted.mix(colors.bg, 0.45),
        };
        let out = &mut self.out.under;
        fill(out, left, top, right - left, bottom - top, colors.bg);
        fill(out, left, top, right - left, edge, border);
        fill(out, left, bottom - edge, right - left, edge, border);
        fill(out, left, top, edge, bottom - top, border);
        fill(out, right - edge, top, edge, bottom - top, border);
    }

    /// The head of a card: its node's label, and the card's tokens and
    /// spend under it.
    fn card_head(&mut self, card: usize, rect: CellRect, head_selected: bool) {
        let colors = self.colors();
        let flow = self.canvas.flow;
        let node = &flow.nodes()[self.layout.cards[card].head];
        let line_top = |line: usize| rect.y as f32 + line as f32;
        let head_color = if head_selected {
            colors.on_selection
        } else {
            colors.fg
        };
        let bold = head_selected || matches!(node.kind, FlowKind::Prompt | FlowKind::Reply);
        let label = fit(&node.label, rect.width - 4);
        self.write(
            &label,
            (rect.x as f32 + 2.0, line_top(1)),
            head_color,
            bold,
            Align::Left,
        );
        if let Some((tokens, spent)) = flow.card_stats(card) {
            let spent = fit(&spent, rect.width - 4);
            let room = (rect.width - 4).saturating_sub(spent.width() + 1);
            let tokens = fit(&tokens, room);
            let y = line_top(2);
            let color = colors.secondary;
            self.write(&tokens, (rect.x as f32 + 2.0, y), color, false, Align::Left);
            let right = rect.right() as f32 - 2.0;
            self.write(&spent, (right, y), color, false, Align::Right);
        }
    }

    /// `↩ reported to Agent`: over the bottom edge of the card at `rect`
    /// an agent reported back from, to whom and how it went.
    fn report(&mut self, card: usize, rect: CellRect) {
        let Some((label, status)) = self.canvas.flow.card_report(card) else {
            return;
        };
        let colors = self.colors();
        let label = fit(&label, rect.width.saturating_sub(6));
        if label.is_empty() {
            return;
        }
        let color = if status == TraceNodeStatus::Error {
            colors.error
        } else {
            colors.success
        };
        let right = rect.right() as f32 - 3.0;
        let left = right - label.width() as f32 - 1.0;
        let y = rect.bottom() as f32 - 1.0;
        let (top, bottom) = (self.y(y).round(), self.y(y + 1.0).round());
        let (x0, x1) = (self.x(left).round(), self.x(right + 1.0).round());
        fill(
            &mut self.out.under,
            x0,
            top,
            x1 - x0,
            bottom - top,
            colors.bg,
        );
        self.write(&label, (right, y), color, false, Align::Right);
    }

    /// The selection's fill across a card's line from canvas row `y`.
    fn highlight(&mut self, left: f32, right: f32, y: f32) {
        let (top, bottom) = (self.y(y).round(), self.y(y + 1.0).round());
        let color = self.colors().selection;
        fill(
            &mut self.out.under,
            left,
            top,
            right - left,
            bottom - top,
            color,
        );
    }

    /// The pixel row of the top edge of a card at `rect`.
    fn card_top(&self, rect: CellRect) -> f32 {
        self.y(rect.y as f32).round()
    }

    /// The pixel row of the bottom edge of a card at `rect`.
    pub(super) fn card_bottom(&self, rect: CellRect) -> f32 {
        self.y(rect.bottom() as f32 - 0.5).round()
    }

    /// `▾ ✓ model ······ 1.2k/300 · 4s`: fold marker (on cards that
    /// fold), status, kind, and the card's facts against the right edge,
    /// on the title row of the card at `rect`.
    fn title(&mut self, card: usize, rect: CellRect) {
        let colors = self.colors();
        let flow = self.canvas.flow;
        let placed = &self.layout.cards[card];
        let node = &flow.nodes()[placed.head];
        let (x1, y) = (rect.right() as f32, rect.y as f32);
        let mut at = rect.x as f32 + 2.0;
        if placed.folds() {
            if flow.fold_hovered(card) {
                self.fold_button(at, y);
            }
            let marker = if placed.folded { "▸" } else { "▾" };
            self.write(marker, (at, y), colors.secondary, false, Align::Left);
            at += 2.0;
        }
        let (glyph, color) = status_glyph(node.status, colors);
        self.write(glyph, (at, y), color, false, Align::Left);
        at += 2.0;
        let kind = flow.card_kind_label(card);
        self.write(
            kind,
            (at, y),
            kind_color(node.kind, colors),
            true,
            Align::Left,
        );
        at += kind.width() as f32 + 1.0;
        let room = (x1 - 2.0 - at - 1.0).max(0.0);
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        let facts = fit(&flow.card_facts(card), room as usize);
        if !facts.is_empty() {
            self.write(&facts, (x1 - 2.0, y), colors.muted, false, Align::Right);
        }
    }

    /// The square under a fold marker at canvas cell `(x, y)` while the
    /// pointer is over it, across the cells a click on it folds from.
    fn fold_button(&mut self, x: f32, y: f32) {
        if self.zoom < GREEK_BELOW {
            return;
        }
        let colors = self.colors();
        let (left, right) = (self.x(x - 0.6).round(), self.x(x + 1.6).round());
        let (top, bottom) = (self.y(y + 0.05).round(), self.y(y + 0.95).round());
        let face = colors.muted.mix(colors.bg, 0.7);
        fill(
            &mut self.out.under,
            left,
            top,
            right - left,
            bottom - top,
            face,
        );
    }

    /// `✓ read_file src/lib.rs          12ms`: a row's status, label, and
    /// its detail against the right edge, on line `line` of its card.
    fn row(&mut self, card: usize, row: usize, line: usize, selected: bool, fill: (f32, f32)) {
        let colors = self.colors();
        let rect = self.layout.card_rect(card);
        let node = &self.canvas.flow.nodes()[self.layout.cards[card].rows[row]];
        let y = (rect.y + line) as f32;
        if selected {
            self.highlight(fill.0, fill.1, y);
        }
        let text = if selected {
            colors.on_selection
        } else {
            match node.kind {
                FlowKind::Event => colors.muted,
                FlowKind::Spawn => colors.secondary,
                _ => colors.fg,
            }
        };
        let (glyph, glyph_color) = match node.kind {
            FlowKind::Reply => ("↳", colors.secondary),
            _ => status_glyph(node.status, colors),
        };
        let glyph_color = if selected { text } else { glyph_color };
        let x = rect.x as f32;
        self.write(glyph, (x + 2.0, y), glyph_color, false, Align::Left);
        let room = rect.width - 4;
        let detail = fit(&node.detail, room / 3);
        let detail_width = detail.width();
        let label_room = room - 2 - detail_width - usize::from(detail_width > 0);
        let label = fit(&node.label, label_room);
        self.write(&label, (x + 4.0, y), text, selected, Align::Left);
        if detail_width > 0 {
            let color = if selected { text } else { colors.muted };
            self.write(
                &detail,
                (rect.right() as f32 - 2.0, y),
                color,
                false,
                Align::Right,
            );
        }
    }
}

const fn status_glyph(status: TraceNodeStatus, colors: Colors) -> (&'static str, Color) {
    match status {
        TraceNodeStatus::Pending => ("●", colors.pending),
        TraceNodeStatus::Ok => ("✓", colors.success),
        TraceNodeStatus::Warn => ("!", colors.warning),
        TraceNodeStatus::Error => ("✗", colors.error),
    }
}

const fn kind_color(kind: FlowKind, colors: Colors) -> Color {
    match kind {
        FlowKind::Prompt => colors.primary,
        FlowKind::Model | FlowKind::Spawn | FlowKind::Message | FlowKind::Result => {
            colors.secondary
        }
        FlowKind::Gate | FlowKind::Judge | FlowKind::Title | FlowKind::Event => colors.muted,
        FlowKind::Tool | FlowKind::Reply | FlowKind::Process => colors.fg,
    }
}
