//! Painting rows of cells: their looks, backgrounds, glyphs and prose.

use crate::geom::{rgba, Color, Point, Rectangle, Size};
use crate::grid::{self, Metrics, Palette};
use crate::scene::{fill, Layers};
use crate::shapes;
use crate::text::{Align, Face, Family, Label, Shaping, TextSystem};
use crate::theme;
use aura_ui_terminal::ProseRow;
use ratatui::buffer::Cell;
use ratatui::layout::Rect;
use ratatui::style::Modifier;
use std::ops::Range;
use unicode_width::UnicodeWidthStr;

/// How one cell is painted, after modifiers are applied.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(super) struct Look {
    pub(super) fg: Color,
    pub(super) bg: Color,
    pub(super) bold: bool,
    pub(super) italic: bool,
    pub(super) underline: Option<Color>,
    pub(super) strike: bool,
    pub(super) hidden: bool,
}

impl Look {
    pub(super) fn of(cell: &Cell, palette: Palette) -> Self {
        let m = cell.modifier;
        let mut fg = grid::color(cell.fg, palette.fg);
        let mut bg = grid::color(cell.bg, palette.bg);
        if m.contains(Modifier::REVERSED) {
            std::mem::swap(&mut fg, &mut bg);
        }
        if m.contains(Modifier::DIM) {
            fg = fg.mix(bg, 0.45);
        }
        let underline = m
            .contains(Modifier::UNDERLINED)
            .then(|| grid::color(cell.underline_color, fg));
        Self {
            fg,
            bg,
            bold: m.contains(Modifier::BOLD),
            italic: m.contains(Modifier::ITALIC),
            underline,
            strike: m.contains(Modifier::CROSSED_OUT),
            hidden: m.contains(Modifier::HIDDEN),
        }
    }

    pub(super) const fn face(&self, family: Family) -> Face {
        Face {
            family,
            bold: self.bold,
            italic: self.italic,
        }
    }
}

/// The prose on screen row `y`, if it holds any.
pub(super) fn prose_at(prose: &[ProseRow], y: usize) -> Option<&ProseRow> {
    prose.iter().find(|p| usize::from(p.y) == y)
}

/// The columns of a row of `len` cells that `prose` flows in, after its
/// indent.
pub(super) fn prose_columns(prose: &ProseRow, len: usize) -> Range<usize> {
    let start = usize::from(prose.x + prose.indent).min(len);
    start..usize::from(prose.x + prose.width).clamp(start, len)
}

/// The columns where screen row `y` of `len` cells is cut into pieces,
/// first and last included: each side of the borders of the panels it
/// crosses and the edges of a sliding `body`, except inside the prose
/// `flow`, which is one run.
pub(super) fn cut(
    cuts: &mut Vec<usize>,
    (panes, body): (&[Rect], Option<Rect>),
    y: u16,
    len: usize,
    flow: Option<&Range<usize>>,
) {
    cuts.clear();
    cuts.push(0);
    for pane in panes.iter().filter(|p| p.y <= y && y < p.bottom()) {
        let (left, right) = (usize::from(pane.x), usize::from(pane.right()));
        cuts.extend([left, left + 1, right.saturating_sub(1), right]);
    }
    if let Some(body) = body {
        cuts.extend([usize::from(body.x), usize::from(body.right())]);
    }
    cuts.push(len);
    cuts.retain(|&c| c <= len && !flow.is_some_and(|f| f.start < c && c < f.end));
    cuts.sort_unstable();
    cuts.dedup();
}

/// One printable ASCII character, which every monospace font has at the
/// cell's advance.
pub(super) fn is_plain(symbol: &str) -> bool {
    let bytes = symbol.as_bytes();
    bytes.len() == 1 && (0x20..0x7f).contains(&bytes[0])
}

/// One row of the screen as it is painted.
pub(super) struct Row<'a> {
    pub(super) cells: &'a [Cell],
    pub(super) looks: &'a [Look],
}

/// Paints rows, each with its top at 0.
pub(super) struct Brush<'a> {
    pub(super) text: &'a mut TextSystem,
    pub(super) metrics: Metrics,
    pub(super) palette: Palette,
    /// Left edge of the grid, in pixels
    pub(super) origin: f32,
}

impl Brush<'_> {
    pub(super) fn looks(&self, cells: &[Cell]) -> Vec<Look> {
        cells.iter().map(|c| Look::of(c, self.palette)).collect()
    }

    /// The `columns` of `row`, with the `prose` that flows inside them.
    pub(super) fn piece(
        &mut self,
        out: &mut Layers,
        row: &Row<'_>,
        columns: Range<usize>,
        prose: Option<&ProseRow>,
    ) {
        let flow = prose.map_or(columns.end..columns.end, |p| {
            prose_columns(p, row.cells.len())
        });
        for grid in [columns.start..flow.start, flow.end..columns.end] {
            self.backgrounds(out, row, grid.clone());
            self.cells(out, row, grid);
        }
        if let Some(prose) = prose {
            self.prose(out, row, flow, &prose.code);
        }
    }

    /// A prose row's `columns` after its indent, laid out in flow: runs
    /// of text in the proportional face, as wide as they measure; inline
    /// `code` a cell per column, in the monospace face, on the text's
    /// baseline and on a background that hugs it (see [`Self::chip`]).
    pub(super) fn prose(
        &mut self,
        out: &mut Layers,
        row: &Row<'_>,
        columns: Range<usize>,
        code: &[Range<u16>],
    ) {
        let (cells, looks) = (row.cells, row.looks);
        let sans = Face {
            family: Family::Sans,
            bold: false,
            italic: false,
        };
        let baseline = self.baseline("x", sans, theme::prose_size(self.metrics.font_size));
        let is_code =
            |i: usize| u16::try_from(i).is_ok_and(|i| code.iter().any(|r| r.contains(&i)));
        let mut x = self.x_at(columns.start);
        let mut col = columns.start;
        while col < columns.end {
            let look = looks[col];
            if cells[col].skip {
                col += 1;
                continue;
            }
            let code = is_code(col);
            let end = (col..columns.end)
                .find(|&i| looks[i] != look || cells[i].skip || is_code(i) != code)
                .unwrap_or(columns.end);
            let run: String = cells[col..end].iter().map(Cell::symbol).collect();
            let reversed = cells[col].modifier.contains(Modifier::REVERSED);
            let code_run = (code, end - col, reversed);
            let width = self.prose_run(out, &run, x, &look, code_run, baseline);
            x += width;
            col = end;
        }
    }

    /// One run of a prose row from `x`, `cells` wide if it is `code`:
    /// its background, its text and its decorations. How wide it is.
    pub(super) fn prose_run(
        &mut self,
        out: &mut Layers,
        run: &str,
        x: f32,
        look: &Look,
        (code, cells, reversed): (bool, usize, bool),
        baseline: f32,
    ) -> f32 {
        let (face, size, width) = if code {
            let size = self.metrics.font_size;
            (
                look.face(Family::Mono),
                size,
                cells as f32 * self.metrics.cell.width,
            )
        } else {
            let (face, size) = (
                look.face(Family::Sans),
                theme::prose_size(self.metrics.font_size),
            );
            (face, size, self.text.run_width(run, face, size))
        };
        let (x0, x1) = (x.round(), (x + width).round());
        // Selected code is highlighted across the row like the text
        // around it.
        if code && !reversed {
            self.chip(out, look.bg, (x0, x1), baseline);
        } else {
            self.background(out, look.bg, x0, x1);
        }
        let shown = run.trim_end();
        if !look.hidden && !shown.is_empty() {
            // Monospace glyphs sit higher in the row than the prose
            // face's: lowered onto its baseline.
            let y = if code {
                baseline - self.baseline(shown, face, size)
            } else {
                0.0
            };
            self.run(
                out,
                shown,
                Point::new(x, y),
                look,
                (face, size, Shaping::Advanced),
            );
        }
        self.decorate(out, look, x, x + width);
        width
    }

    /// A block over the cell in column `column`, the character `under`
    /// it cut out of the block in the cell's background color.
    pub(super) fn cursor(&mut self, out: &mut Layers, column: u16, under: Option<&Cell>) {
        let cell = self.metrics.cell;
        let x = (self.origin + f32::from(column) * cell.width).round();
        let next = (self.origin + f32::from(column + 1) * cell.width).round();
        fill(
            &mut out.under,
            x,
            0.0,
            next - x,
            cell.height,
            self.palette.fg,
        );
        let Some(under) = under else {
            return;
        };
        let look = Look::of(under, self.palette);
        let cut = Look {
            fg: look.bg,
            bg: self.palette.fg,
            underline: None,
            strike: false,
            ..look
        };
        if !under.skip {
            self.symbol(out, under.symbol(), x, &cut);
        }
    }

    /// Backgrounds of `columns`, one rectangle per run of the same color.
    pub(super) fn backgrounds(&self, out: &mut Layers, row: &Row<'_>, columns: Range<usize>) {
        let edge = |col: usize| self.x_at(col).round();
        let mut start = columns.start;
        while start < columns.end {
            let bg = row.looks[start].bg;
            let end = (start..columns.end)
                .find(|&i| row.looks[i].bg != bg)
                .unwrap_or(columns.end);
            self.background(out, bg, edge(start), edge(end));
            start = end;
        }
    }

    /// Foreground of `columns`: runs of plain ASCII share one shaped run;
    /// anything else is a built-in shape or a single centered glyph.
    pub(super) fn cells(&mut self, out: &mut Layers, row: &Row<'_>, columns: Range<usize>) {
        let (cells, looks) = (row.cells, row.looks);
        let mut col = columns.start;
        while col < columns.end {
            let look = looks[col];
            let symbol = cells[col].symbol();
            if cells[col].skip || look.hidden {
                col += 1;
                continue;
            }
            if is_plain(symbol) {
                let end = (col..columns.end)
                    .find(|&i| looks[i] != look || cells[i].skip || !is_plain(cells[i].symbol()))
                    .unwrap_or(columns.end);
                let run: String = cells[col..end].iter().map(Cell::symbol).collect();
                let x = self.x_at(col);
                let face = (
                    look.face(Family::Mono),
                    self.metrics.font_size,
                    Shaping::Basic,
                );
                self.run(out, &run, Point::new(x, 0.0), &look, face);
                self.decorate(out, &look, x, self.x_at(end));
                col = end;
                continue;
            }
            self.symbol(out, symbol, self.x_at(col), &look);
            col += 1;
        }
    }

    /// One symbol that is not plain ASCII, in the cells from `x`.
    pub(super) fn symbol(&mut self, out: &mut Layers, symbol: &str, x: f32, look: &Look) {
        let cell = self.metrics.cell;
        let width = symbol.width().max(1);
        let area = Rectangle::new(
            Point::new(x, 0.0),
            Size::new(cell.width * width as f32, cell.height),
        );
        if !look.hidden {
            let mut chars = symbol.chars();
            let built_in = match (chars.next(), chars.next()) {
                (Some(c), None) if width == 1 => {
                    let atlas = self.text.atlas_mut();
                    shapes::draw(&mut out.under, atlas, c, area, rgba(look.fg))
                }
                _ => false,
            };
            if !built_in && !symbol.trim().is_empty() {
                let face = (
                    look.face(Family::Mono),
                    self.metrics.font_size,
                    Shaping::Advanced,
                );
                let at = Point::new(area.center_x(), 0.0);
                self.write(out, symbol, at, look, face, Align::Center);
            }
        }
        self.decorate(out, look, area.x, area.x + area.width);
    }

    pub(super) fn x_at(&self, col: usize) -> f32 {
        self.origin + col as f32 * self.metrics.cell.width
    }

    /// Where `text` in `face` at `size` has its baseline on a row, in
    /// whole pixels down from the row's top, as [`Self::write`] puts it.
    pub(super) fn baseline(&mut self, text: &str, face: Face, size: f32) -> f32 {
        let line_height = self.metrics.cell.height;
        let run = self
            .text
            .shape(text, face, size, line_height, Shaping::Advanced);
        run.baseline.round()
    }

    /// The background of inline code from `x0` to `x1` on a row whose
    /// text sits on `baseline`: from a little above the code's tallest
    /// glyphs to a little below its descenders, inside the row, so the
    /// code of rows one under another stays apart and the row reads no
    /// taller than the plain ones.
    pub(super) fn chip(&self, out: &mut Layers, bg: Color, (x0, x1): (f32, f32), baseline: f32) {
        let (above, below) = theme::code_reach(self.metrics.font_size);
        let top = (baseline - above).round().max(0.0);
        let bottom = (baseline + below).round().min(self.metrics.cell.height);
        if bg != self.palette.bg && x1 > x0 && bottom > top {
            fill(&mut out.under, x0, top, x1 - x0, bottom - top, bg);
        }
    }

    pub(super) fn background(&self, out: &mut Layers, bg: Color, x0: f32, x1: f32) {
        if bg != self.palette.bg && x1 > x0 {
            fill(
                &mut out.under,
                x0,
                0.0,
                x1 - x0,
                self.metrics.cell.height,
                bg,
            );
        }
    }

    /// A run of text from `at`, unless it is only spaces.
    pub(super) fn run(
        &mut self,
        out: &mut Layers,
        run: &str,
        at: Point,
        look: &Look,
        face: (Face, f32, Shaping),
    ) {
        let text = run.trim_end();
        if !text.is_empty() {
            self.write(out, text, at, look, face, Align::Left);
        }
    }

    pub(super) fn write(
        &mut self,
        out: &mut Layers,
        content: &str,
        at: Point,
        look: &Look,
        (face, size, shaping): (Face, f32, Shaping),
        align: Align,
    ) {
        let label = Label {
            text: content,
            face,
            size,
            line_height: self.metrics.cell.height,
            shaping,
            align,
            middle: false,
            color: rgba(look.fg),
            on_row: true,
        };
        self.text.write(&mut out.over, &label, at);
    }

    pub(super) fn decorate(&self, out: &mut Layers, look: &Look, x0: f32, x1: f32) {
        let cell = self.metrics.cell;
        let t = (self.metrics.font_size / 14.0).round().max(1.0);
        if let Some(color) = look.underline {
            fill(&mut out.under, x0, cell.height - 2.0 * t, x1 - x0, t, color);
        }
        if look.strike {
            let mid = (cell.height / 2.0).round();
            fill(&mut out.under, x0, mid, x1 - x0, t, look.fg);
        }
    }
}
