//! An info body: a read-only document of titled sections of facts. Every
//! panel that describes one thing draws it this way (POLICIES, HOST, a
//! channel's INFO, the RECORD and PROCESS views, a project's info box);
//! panels that list things draw [`super::list_row`] rows instead.
//!
//! ```text
//! boxd · aura-re36-3f9c              title     bold foreground
//! ● ready · up 2h 14m                status    the accent
//!
//! RESOURCES  1 near full             section   secondary, then a muted note
//!   Region    fra                    fact      muted label, value cut with …
//!   Disk      █████████░ 91%         bar       the accent, the rest faint
//!   ✓ read_file   on   per tool      marked    only the glyph colored
//!   `cargo test` commands            item      foreground, then a muted detail
//!
//! [p] pull changes  [s] sleep        actions   primary keys, muted labels
//! ```
//!
//! A value is [`Value::Plain`] unless it needs attention ([`Value::Flag`],
//! the accent), is an id, a hash or `none` ([`Value::Quiet`]), or says
//! what failed ([`Value::Error`]). Nothing else is colored: section titles
//! never take the accent.

use super::StatusDot;
use crate::themes::Theme;
use ratatui::{
    style::{Color, Modifier, Style},
    text::{Line, Span},
};
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

/// Columns of a fact's label, unless [`Info::label_width`] sets others.
const LABEL_WIDTH: usize = 10;
/// Cells of a use bar.
const BAR_WIDTH: usize = 10;
/// What a section's facts, items and prose are indented by.
const INDENT: &str = "  ";
/// The fewest columns an item's detail is drawn in; fewer drop it.
const MIN_DETAIL: usize = 4;

/// How a fact's value is drawn.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Value {
    /// `foreground`: most values.
    Plain,
    /// `muted`: ids, hashes and `none`.
    Quiet,
    /// The accent: a value that needs attention, as `yolo` or a full disk.
    Flag,
    /// `error`: what failed.
    Error,
}

impl Value {
    /// [`Self::Flag`] when `flag`, else [`Self::Plain`].
    pub const fn flag_if(flag: bool) -> Self {
        if flag {
            Self::Flag
        } else {
            Self::Plain
        }
    }

    fn style(self, theme: &Theme) -> Style {
        Style::default().fg(match self {
            Self::Plain => theme.colors.foreground,
            Self::Quiet => theme.colors.muted,
            Self::Flag => theme.colors.success,
            Self::Error => theme.colors.error,
        })
    }
}

/// The lines of an info body `width` columns wide, built top to bottom.
pub struct Info<'t> {
    width: usize,
    label_width: usize,
    wrap_values: bool,
    theme: &'t Theme,
    lines: Vec<Line<'static>>,
}

impl<'t> Info<'t> {
    pub const fn new(width: usize, theme: &'t Theme) -> Self {
        Self {
            width,
            label_width: LABEL_WIDTH,
            wrap_values: false,
            theme,
            lines: Vec::new(),
        }
    }

    /// Labels `columns` wide, for a document whose labels are longer.
    #[must_use]
    pub const fn label_width(mut self, columns: usize) -> Self {
        self.label_width = columns;
        self
    }

    /// Wrap long values under themselves instead of cutting them, for a
    /// view that shows the whole of what it describes.
    #[must_use]
    pub const fn wrap_values(mut self) -> Self {
        self.wrap_values = true;
        self
    }

    /// What the document describes, at its top: bold `foreground`.
    pub fn title(&mut self, text: &str) {
        let style = Style::default()
            .fg(self.theme.colors.foreground)
            .add_modifier(Modifier::BOLD);
        self.flush(text, style);
    }

    /// How the thing is doing, under its title: the accent.
    pub fn status(&mut self, text: &str) {
        self.flush(text, Style::default().fg(self.theme.colors.success));
    }

    /// A muted remark at the left edge, wrapped.
    pub fn note(&mut self, text: &str) {
        let style = self.muted();
        for line in wrap(text, self.width) {
            self.lines.push(Line::from(Span::styled(line, style)));
        }
    }

    /// A section's title, after a blank row unless it opens the document,
    /// then its muted `note`.
    pub fn section(&mut self, title: &str, note: Option<&str>) {
        if !self.lines.is_empty() {
            self.blank();
        }
        let title = truncate_line(title, self.width);
        let room = self.width.saturating_sub(title.width() + 2);
        let mut spans = vec![Span::styled(
            title,
            Style::default().fg(self.theme.colors.secondary),
        )];
        if let Some(note) = note.filter(|_| room > 0) {
            spans.push(Span::styled(
                format!("  {}", truncate_line(note, room)),
                self.muted(),
            ));
        }
        self.lines.push(Line::from(spans));
    }

    /// A section's title led by a status dot, after a blank row unless it
    /// opens the document, then its muted `note`: `● MERGED  a1b2c3d`.
    /// The dot is the one colored cell; the title keeps the `secondary`
    /// of every section title and is never bold.
    pub fn marked_section(&mut self, dot: StatusDot, title: &str, note: Option<&str>) {
        if !self.lines.is_empty() {
            self.blank();
        }
        let lead = dot.glyph().width() + 1;
        let title = truncate_line(title, self.width.saturating_sub(lead));
        let room = self.width.saturating_sub(lead + title.width() + 2);
        let mut spans = vec![
            dot.span(self.theme),
            Span::raw(" "),
            Span::styled(title, Style::default().fg(self.theme.colors.secondary)),
        ];
        if let Some(note) = note.filter(|note| room > 0 && !note.trim().is_empty()) {
            spans.push(Span::styled(
                format!("  {}", truncate_line(note, room)),
                self.muted(),
            ));
        }
        self.lines.push(Line::from(spans));
    }

    /// `  text`, indented like an item, in `foreground` and underlined: a
    /// line the pointer opens. Returns its index among the document's
    /// lines, for the caller to find it under the pointer.
    pub fn link(&mut self, text: &str) -> usize {
        let text = truncate_line(text, self.width.saturating_sub(INDENT.len()));
        self.lines.push(Line::from(vec![
            Span::raw(INDENT),
            Span::styled(text, self.plain().add_modifier(Modifier::UNDERLINED)),
        ]));
        self.lines.len() - 1
    }

    /// `  label     value`: the muted label in its column, then the value,
    /// cut to fit (or wrapped under itself, see [`Self::wrap_values`]). A
    /// fact without a value shows a muted `—`.
    pub fn fact(&mut self, label: &str, value: &str, kind: Value) {
        let (value, kind) = if value.trim().is_empty() {
            ("—", Value::Quiet)
        } else {
            (value, kind)
        };
        let lead = INDENT.len() + self.label_width;
        let room = self.width.saturating_sub(lead);
        let style = kind.style(self.theme);
        let label = self.label(label);
        if !self.wrap_values {
            self.lines.push(Line::from(vec![
                Span::raw(INDENT),
                label,
                Span::styled(truncate_line(value, room), style),
            ]));
            return;
        }
        let mut first = Some(label);
        for chunk in wrap(value, room) {
            let lead = match first.take() {
                Some(label) => vec![Span::raw(INDENT), label],
                None => vec![Span::raw(" ".repeat(lead))],
            };
            let mut spans = lead;
            spans.push(Span::styled(chunk, style));
            self.lines.push(Line::from(spans));
        }
    }

    /// A [`Self::fact`] about a use: a bar `percent` full before the value.
    pub fn bar(&mut self, label: &str, percent: u8, value: &str, kind: Value) {
        let filled = (usize::from(percent) * BAR_WIDTH)
            .div_ceil(100)
            .min(BAR_WIDTH);
        let room = self
            .width
            .saturating_sub(INDENT.len() + self.label_width + BAR_WIDTH + 1);
        let label = self.label(label);
        self.lines.push(Line::from(vec![
            Span::raw(INDENT),
            label,
            Span::styled(
                "█".repeat(filled),
                Style::default().fg(self.theme.colors.success),
            ),
            Span::styled(
                "░".repeat(BAR_WIDTH - filled),
                Style::default().fg(self.theme.colors.faint),
            ),
            Span::raw(" "),
            Span::styled(truncate_line(value, room), kind.style(self.theme)),
        ]));
    }

    /// `  text  detail`: one thing a section lists, then an optional muted
    /// detail, which is cut first and dropped when there is no room.
    pub fn item(&mut self, text: &str, detail: Option<&str>) {
        let room = self.width.saturating_sub(INDENT.len());
        let text = truncate_line(text, room);
        let left = room.saturating_sub(text.width() + 2);
        let mut spans = vec![Span::raw(INDENT), Span::styled(text, self.plain())];
        if let Some(detail) = detail.filter(|detail| !detail.is_empty() && left >= MIN_DETAIL) {
            spans.push(Span::styled(
                format!("  {}", truncate_line(detail, left)),
                self.muted(),
            ));
        }
        self.lines.push(Line::from(spans));
    }

    /// Each of `items` as an [`Self::item`], or `none`.
    pub fn items(&mut self, items: &[String]) {
        if items.is_empty() {
            self.none();
        }
        for item in items {
            self.item(item, None);
        }
    }

    /// `  none`, muted: a section with nothing in it.
    pub fn none(&mut self) {
        self.text("none", Value::Quiet);
    }

    /// Prose in a section, indented like its facts and wrapped. Blank
    /// lines in `text` are kept.
    pub fn text(&mut self, text: &str, kind: Value) {
        let room = self.width.saturating_sub(INDENT.len());
        let style = kind.style(self.theme);
        for paragraph in text.lines() {
            if paragraph.trim().is_empty() {
                self.blank();
                continue;
            }
            for line in wrap(paragraph, room) {
                self.lines.push(Line::from(vec![
                    Span::raw(INDENT),
                    Span::styled(line, style),
                ]));
            }
        }
    }

    /// `  ✓ name      detail`: an item marked by a glyph in `color`, the
    /// one colored cell, with a muted detail. Names are padded to
    /// `name_width` so the details line up.
    pub fn marked(
        &mut self,
        (glyph, color): (&str, Color),
        (name, name_width): (&str, usize),
        detail: &str,
    ) {
        let name = truncate_line(name, name_width);
        let pad = " ".repeat(name_width.saturating_sub(name.width()));
        let used = INDENT.len() + glyph.width() + 1 + name_width + 2;
        let detail = truncate_line(detail, self.width.saturating_sub(used));
        self.lines.push(Line::from(vec![
            Span::raw(INDENT),
            Span::styled(glyph.to_string(), Style::default().fg(color)),
            Span::raw(" "),
            Span::styled(format!("{name}{pad}"), self.plain()),
            Span::raw("  "),
            Span::styled(detail, self.muted()),
        ]));
    }

    /// `[p] pull changes  [s] sleep`: the keys that work the thing, after
    /// a blank row, each key `primary` and its label muted.
    pub fn actions(&mut self, actions: &[(char, &str)]) {
        if actions.is_empty() {
            return;
        }
        if !self.lines.is_empty() {
            self.blank();
        }
        let mut spans = Vec::new();
        for (index, (key, label)) in actions.iter().enumerate() {
            if index > 0 {
                spans.push(Span::raw("  "));
            }
            spans.push(Span::styled(
                format!("[{key}]"),
                Style::default().fg(self.theme.colors.primary),
            ));
            spans.push(Span::styled(format!(" {label}"), self.muted()));
        }
        self.lines.push(Line::from(spans));
    }

    /// Lines drawn elsewhere, indented like a section's facts; they were
    /// laid out [`INDENT`] narrower than the document.
    pub fn indented(&mut self, lines: impl IntoIterator<Item = Line<'static>>) {
        for mut line in lines {
            line.spans.insert(0, Span::raw(INDENT));
            self.lines.push(line);
        }
    }

    /// Columns left for lines passed to [`Self::indented`].
    pub const fn indented_width(&self) -> usize {
        self.width.saturating_sub(INDENT.len())
    }

    pub fn blank(&mut self) {
        self.lines.push(Line::default());
    }

    pub fn into_lines(self) -> Vec<Line<'static>> {
        self.lines
    }

    fn flush(&mut self, text: &str, style: Style) {
        let text = truncate_line(text, self.width);
        self.lines.push(Line::from(Span::styled(text, style)));
    }

    /// `label` padded to the label column, with a space kept after it.
    fn label(&self, label: &str) -> Span<'static> {
        let label = truncate_line(label, self.label_width.saturating_sub(1));
        let pad = " ".repeat(self.label_width.saturating_sub(label.width()));
        Span::styled(format!("{label}{pad}"), self.muted())
    }

    fn plain(&self) -> Style {
        Style::default().fg(self.theme.colors.foreground)
    }

    fn muted(&self) -> Style {
        Style::default().fg(self.theme.colors.muted)
    }
}

/// `text` on one line, cut to `width` columns and ending in `…` when cut.
/// Line breaks and other control characters turn into spaces, but unlike
/// [`super::truncate_record_info`] runs of spaces are kept, so text spaced
/// into columns stays in them.
pub fn truncate_line(text: &str, width: usize) -> String {
    let line: String = text
        .chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect();
    let line = line.trim_end();
    if line.width() <= width {
        return line.to_string();
    }
    if width == 0 {
        return String::new();
    }
    let mut cut = String::new();
    let mut used = 1;
    for c in line.chars() {
        let char_width = c.width().unwrap_or(0);
        if used + char_width > width {
            break;
        }
        used += char_width;
        cut.push(c);
    }
    cut.push('…');
    cut
}

/// `text` broken at spaces into lines at most `width` columns wide; a
/// word longer than a line is broken where it reaches the edge.
pub(super) fn wrap(text: &str, width: usize) -> Vec<String> {
    let width = width.max(1);
    let mut lines = Vec::new();
    let mut line = String::new();
    let mut used = 0;
    for word in text.split_whitespace() {
        if used > 0 && used + 1 + word.width() > width {
            lines.push(std::mem::take(&mut line));
            used = 0;
        }
        if used > 0 {
            line.push(' ');
            used += 1;
        }
        for c in word.chars() {
            let char_width = c.width().unwrap_or(0);
            if used > 0 && used + char_width > width {
                lines.push(std::mem::take(&mut line));
                used = 0;
            }
            line.push(c);
            used += char_width;
        }
    }
    if !line.is_empty() {
        lines.push(line);
    }
    lines
}

#[cfg(test)]
mod tests;
