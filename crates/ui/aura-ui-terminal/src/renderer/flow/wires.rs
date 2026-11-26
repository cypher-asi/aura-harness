//! The wires between the GRAPH tab's cards, drawn with box characters
//! that join where wires meet.

use super::Canvas;
use crate::app::flow::layout::{FlowLayout, Wire, WireKind};
use crate::events::TraceNodeStatus;
use ratatui::style::Style;

impl Canvas<'_> {
    /// A wire's color: the selection's when it joins the selected card;
    /// for a wire giving an agent its work, how the work went once it
    /// came back; else its kind's.
    pub(super) fn wire_style(&self, wire: &Wire, hot: bool) -> Style {
        if hot {
            return Style::default().fg(self.theme.colors.primary);
        }
        match self.flow.wire_status(wire) {
            Some(TraceNodeStatus::Ok) => return Style::default().fg(self.theme.colors.success),
            Some(TraceNodeStatus::Error) => return Style::default().fg(self.theme.colors.error),
            _ => {}
        }
        Style::default().fg(match wire.kind {
            WireKind::Next | WireKind::Spine => self.theme.colors.muted,
            WireKind::Spawn | WireKind::Link => self.theme.colors.secondary,
            WireKind::Return => self.theme.colors.success,
            WireKind::Message => self.theme.colors.warning,
        })
    }

    /// One wire: out of its port, along to the gutter before the block it
    /// goes to, up or down there, and into its card's side (over the
    /// block's spine, for a subcard).
    pub(super) fn wire(&mut self, wire: Wire, style: Style) {
        let (from_x, from_y) = self.layout.port_out(wire.from, wire.row);
        let (to_x, to_y) = self.layout.port_in(wire.to);
        let gutter = FlowLayout::column_x(self.layout.cards[wire.to].column);
        if gutter < from_x + 3 {
            return;
        }
        if from_y == to_y {
            self.horizontal(from_x + 1..to_x, from_y, style);
        } else {
            let turn = (gutter - 2 - wire.track).max(from_x + 1);
            self.horizontal(from_x + 1..turn, from_y, style);
            let (top, bottom) = (from_y.min(to_y), from_y.max(to_y));
            for y in top + 1..bottom {
                self.join(turn, y, UP | DOWN, style);
            }
            let (leave, arrive) = if to_y > from_y {
                (LEFT | DOWN, UP | RIGHT)
            } else {
                (LEFT | UP, DOWN | RIGHT)
            };
            self.join(turn, from_y, leave, style);
            self.join(turn, to_y, arrive, style);
            self.horizontal(turn + 1..to_x, to_y, style);
        }
    }

    /// A spine wire: down from the main card's bottom border, then across
    /// into the subcard's side. The wires of one block share the spine,
    /// so it branches `├` at every subcard but the last.
    pub(super) fn spine(&mut self, wire: Wire, style: Style) {
        let main = self.layout.card_rect(wire.from);
        let (to_x, to_y) = self.layout.port_in(wire.to);
        let x = self.layout.spine_x(wire.from);
        self.join(x, main.bottom() - 1, DOWN, style);
        for y in main.bottom()..to_y {
            self.join(x, y, UP | DOWN, style);
        }
        self.join(x, to_y, UP | RIGHT, style);
        self.horizontal(x + 1..to_x, to_y, style);
        self.join(to_x, to_y, LEFT, style);
    }

    fn horizontal(&mut self, xs: std::ops::Range<usize>, y: usize, style: Style) {
        for x in xs {
            self.join(x, y, LEFT | RIGHT, style);
        }
    }

    /// A piece of wire with `ends` at `(x, y)`, joined to any wire already
    /// there: `─` over `│` makes `┼`, over `┌` makes `┬`.
    pub(super) fn join(&mut self, x: usize, y: usize, ends: u8, style: Style) {
        let ends = ends | ends_of(self.symbol(x, y));
        self.put(x, y, glyph_of(ends), style);
    }
}

const UP: u8 = 1;
const DOWN: u8 = 2;
pub(super) const LEFT: u8 = 4;
pub(super) const RIGHT: u8 = 8;

/// Box glyphs by the ends they join.
const GLYPHS: [(&str, u8); 11] = [
    ("─", LEFT | RIGHT),
    ("│", UP | DOWN),
    ("├", UP | DOWN | RIGHT),
    ("┤", UP | DOWN | LEFT),
    ("┬", DOWN | LEFT | RIGHT),
    ("┴", UP | LEFT | RIGHT),
    ("┼", UP | DOWN | LEFT | RIGHT),
    ("┌", DOWN | RIGHT),
    ("┐", DOWN | LEFT),
    ("└", UP | RIGHT),
    ("┘", UP | LEFT),
];

fn ends_of(symbol: &str) -> u8 {
    GLYPHS
        .iter()
        .find(|(glyph, _)| *glyph == symbol)
        .map_or(0, |(_, ends)| *ends)
}

fn glyph_of(ends: u8) -> &'static str {
    GLYPHS
        .iter()
        .find(|(_, glyph_ends)| *glyph_ends == ends)
        .map_or(
            if ends & (UP | DOWN) == 0 {
                "─"
            } else {
                "│"
            },
            |(glyph, _)| glyph,
        )
}
