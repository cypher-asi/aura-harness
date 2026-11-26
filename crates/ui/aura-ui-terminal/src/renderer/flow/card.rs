//! The GRAPH tab's cards: border, title, head and rows.

use super::{Canvas, Standing};
use crate::app::flow::layout::CellRect;
use crate::components::truncate_record_info;
use crate::events::{FlowKind, TraceNodeStatus};
use crate::renderer::graph::status_glyph;
use crate::renderer::text::display_width;
use crate::Theme;
use ratatui::style::{Color, Modifier, Style};

impl Canvas<'_> {
    fn border(&self, standing: Standing) -> Style {
        match standing {
            Standing::Selected => Style::default().fg(self.theme.colors.primary),
            Standing::Linked => Style::default().fg(self.theme.colors.secondary),
            Standing::Other => self.muted(),
        }
    }

    fn selection(&self) -> Style {
        Style::default()
            .bg(self.theme.colors.selection)
            .fg(self.theme.colors.on_selection)
            .add_modifier(Modifier::BOLD)
    }

    pub(super) fn card(&mut self, card: usize, standing: Standing) {
        let rect = self.layout.card_rect(card);
        let (xs, ys) = self.shown();
        let on_screen = rect.right() > xs.start
            && rect.x < xs.end
            && rect.bottom() > ys.start
            && rect.y < ys.end;
        if !on_screen {
            return;
        }
        let border = self.border(standing);
        self.frame_card(rect, border);
        self.title(card, border);
        self.report(card);

        let selected = self.flow.selected_place().filter(|(at, _)| *at == card);
        self.head(card, selected == Some((card, None)));
        self.rows(card, selected);
    }

    /// The box of a card at `rect`, its borders in `border`, blank inside.
    fn frame_card(&mut self, rect: CellRect, border: Style) {
        let (x, y, w) = (rect.x, rect.y, rect.width);
        let bottom = rect.bottom() - 1;
        for line in y..=bottom {
            let (left, fill, right) = if line == y {
                ("┌", "─", "┐")
            } else if line == bottom {
                ("└", "─", "┘")
            } else {
                ("│", " ", "│")
            };
            self.put(x, line, left, border);
            for column in x + 1..x + w - 1 {
                self.put(
                    column,
                    line,
                    fill,
                    if fill == " " {
                        Style::default()
                    } else {
                        border
                    },
                );
            }
            self.put(x + w - 1, line, right, border);
        }
    }

    /// A card's head, lit when `selected`, over its tokens and time spent.
    fn head(&mut self, card: usize, selected: bool) {
        let rect = self.layout.card_rect(card);
        let (x, y, w) = (rect.x, rect.y, rect.width);
        let node = &self.flow.nodes()[self.layout.cards[card].head];
        let head = if selected {
            self.selection()
        } else {
            let style = Style::default().fg(self.theme.colors.foreground);
            match node.kind {
                FlowKind::Prompt | FlowKind::Reply => style.add_modifier(Modifier::BOLD),
                _ => style,
            }
        };
        self.line_fill(card, 1, head);
        self.text(x + 2, y + 1, &node.label, w - 4, head);
        if let Some((tokens, spent)) = self.flow.card_stats(card) {
            let style = Style::default().fg(self.theme.colors.secondary);
            let spent = truncate_record_info(&spent, w - 4);
            let spent_width = display_width(&spent);
            self.text(x + w - 2 - spent_width, y + 2, &spent, spent_width, style);
            let room = (w - 4).saturating_sub(spent_width + 1);
            self.text(x + 2, y + 2, &tokens, room, style);
        }
    }

    /// A card's rows, the one in `selected` lit; the last line of a card
    /// with more rows than lines says how many more.
    fn rows(&mut self, card: usize, selected: Option<(usize, Option<usize>)>) {
        let rect = self.layout.card_rect(card);
        let (x, y, w) = (rect.x, rect.y, rect.width);
        let placed = &self.layout.cards[card];
        let shown = placed.shown_rows();
        for row in 0..placed.lines() {
            let line = placed.first_row_line() + row;
            if row == shown {
                // The last line of a card with more rows than lines.
                match self.flow.last_line_row(card) {
                    Some(hidden) => self.row(card, hidden, line, true),
                    None => {
                        let more = self.flow.more_label(card);
                        self.text(x + 2, y + line, &more, w - 4, self.muted());
                    }
                }
                continue;
            }
            let is_selected = selected == Some((card, Some(row)));
            self.row(card, row, line, is_selected);
        }
    }

    /// `┌ ▾ ✓ model ─────── 1.2k/300 · 4s ┐`: fold marker (on cards that
    /// fold), status, kind, and the card's facts against the right corner.
    fn title(&mut self, card: usize, border: Style) {
        let rect = self.layout.card_rect(card);
        let placed = &self.layout.cards[card];
        let node = &self.flow.nodes()[placed.head];
        let (glyph, glyph_color) = status_glyph(node.status, self.theme);
        let mut parts: Vec<(&str, Style)> = Vec::new();
        if placed.folds() {
            let marker = if placed.folded { "▸" } else { "▾" };
            parts.push((marker, Style::default().fg(self.theme.colors.secondary)));
        }
        parts.push((glyph, Style::default().fg(glyph_color)));
        parts.push((
            self.flow.card_kind_label(card),
            Style::default()
                .fg(kind_color(node.kind, self.theme))
                .add_modifier(Modifier::BOLD),
        ));
        let end = rect.right() - 1;
        let mut at = rect.x + 1;
        self.put(at, rect.y, " ", border);
        at += 1;
        for (text, style) in parts {
            let width = display_width(text);
            if at + width + 1 >= end {
                break;
            }
            self.text(at, rect.y, text, width, style);
            self.put(at + width, rect.y, " ", border);
            at += width + 1;
        }
        let room = end.saturating_sub(at + 1);
        let facts = truncate_record_info(&self.flow.card_facts(card), room.saturating_sub(2));
        if room >= 5 && !facts.is_empty() {
            let width = display_width(&facts);
            let start = end - width - 2;
            self.put(start, rect.y, " ", self.muted());
            self.text(start + 1, rect.y, &facts, width, self.muted());
            self.put(start + 1 + width, rect.y, " ", self.muted());
        }
    }

    /// `└─ ↩ reported to Agent ─┘`: on the bottom border of the card an
    /// agent reported back from, to whom and how it went.
    fn report(&mut self, card: usize) {
        let Some((label, status)) = self.flow.card_report(card) else {
            return;
        };
        let rect = self.layout.card_rect(card);
        let color = if status == TraceNodeStatus::Error {
            self.theme.colors.error
        } else {
            self.theme.colors.success
        };
        let room = rect.width.saturating_sub(6);
        let label = truncate_record_info(&label, room);
        let width = display_width(&label);
        if width == 0 {
            return;
        }
        let y = rect.bottom() - 1;
        let start = rect.right() - 3 - width;
        self.put(start - 1, y, " ", self.muted());
        self.text(start, y, &label, width, Style::default().fg(color));
        self.put(start + width, y, " ", self.muted());
    }

    /// Paint line `line` of card `card`, inside its borders, in `style`.
    fn line_fill(&mut self, card: usize, line: usize, style: Style) {
        let rect = self.layout.card_rect(card);
        for x in rect.x + 1..rect.right() - 1 {
            self.put(x, rect.y + line, " ", style);
        }
    }

    /// `✓ read_file src/lib.rs          12ms`: a row's status, label, and
    /// its detail against the right border.
    fn row(&mut self, card: usize, row: usize, line: usize, selected: bool) {
        let rect = self.layout.card_rect(card);
        let node = &self.flow.nodes()[self.layout.cards[card].rows[row]];
        let (glyph, glyph_color) = match node.kind {
            FlowKind::Reply => ("↳", self.theme.colors.secondary),
            _ => status_glyph(node.status, self.theme),
        };
        let base = if selected {
            self.selection()
        } else {
            Style::default().fg(match node.kind {
                FlowKind::Event => self.theme.colors.muted,
                FlowKind::Spawn => self.theme.colors.secondary,
                _ => self.theme.colors.foreground,
            })
        };
        if selected {
            self.line_fill(card, line, base);
        }
        let y = rect.y + line;
        let room = rect.width - 4;
        let glyph_style = if selected { base } else { base.fg(glyph_color) };
        self.text(rect.x + 2, y, glyph, 1, glyph_style);
        let detail = truncate_record_info(&node.detail, room / 3);
        let detail_width = display_width(&detail);
        let label_room = room - 2 - detail_width - usize::from(detail_width > 0);
        let label = truncate_record_info(&node.label, label_room);
        self.text(rect.x + 4, y, &label, label_room, base);
        if detail_width > 0 {
            let detail_style = if selected { base } else { self.muted() };
            self.text(
                rect.right() - 2 - detail_width,
                y,
                &detail,
                detail_width,
                detail_style,
            );
        }
    }
}

fn kind_color(kind: FlowKind, theme: &Theme) -> Color {
    match kind {
        FlowKind::Prompt => theme.colors.primary,
        FlowKind::Model | FlowKind::Spawn | FlowKind::Message | FlowKind::Result => {
            theme.colors.secondary
        }
        FlowKind::Gate | FlowKind::Judge | FlowKind::Title | FlowKind::Event => theme.colors.muted,
        FlowKind::Tool | FlowKind::Reply | FlowKind::Process => theme.colors.foreground,
    }
}
