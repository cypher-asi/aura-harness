//! Lines rendered from markdown, with where the links a click opens fell.

use super::inline::Linked;
use super::wrap::wrap_linked;
use crate::app::LinkHit;
use crate::renderer::text::Measure;
use ratatui::text::{Line, Span};

/// Lines rendered from markdown and the rows of its links, `line`
/// counted in `lines`, `x` in display columns from the line's start.
#[derive(Debug, Default)]
pub(crate) struct Rendered {
    pub(crate) lines: Vec<Line<'static>>,
    pub(crate) links: Vec<LinkHit>,
}

impl Rendered {
    /// Wrap `linked` after these lines, as [`super::wrap_spans`] does,
    /// its links numbered after the ones before.
    pub(super) fn wrap(
        &mut self,
        mut linked: Linked,
        width: usize,
        prefixes: (&[Span<'static>], &[Span<'static>]),
        measure: Measure,
    ) {
        let urls = std::mem::take(&mut linked.urls);
        let (lines, pieces) = wrap_linked(linked, width, prefixes, measure);
        let (rows, numbers) = (self.lines.len(), LinkHit::numbers(&self.links));
        self.links.extend(pieces.into_iter().filter_map(|piece| {
            Some(LinkHit {
                line: rows + piece.line,
                x: piece.columns.start,
                width: piece.columns.end - piece.columns.start,
                link: numbers + piece.link,
                url: urls.get(piece.link)?.clone(),
            })
        }));
        self.lines.extend(lines);
    }

    /// Put `inner`'s lines after these, each behind `bar`.
    pub(super) fn quote(&mut self, inner: Self, bar: &Span<'static>) {
        let shift = (
            self.lines.len(),
            u16::try_from(bar.width()).unwrap_or(u16::MAX),
            LinkHit::numbers(&self.links),
        );
        self.links
            .extend(inner.links.into_iter().map(|hit| hit.moved(shift)));
        for mut quoted in inner.lines {
            quoted.spans.insert(0, bar.clone());
            self.lines.push(quoted);
        }
    }
}
