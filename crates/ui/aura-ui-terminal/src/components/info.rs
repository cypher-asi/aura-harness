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

use crate::Theme;
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
pub(crate) enum Value {
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
    pub(crate) const fn flag_if(flag: bool) -> Self {
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
pub(crate) struct Info<'t> {
    width: usize,
    label_width: usize,
    wrap_values: bool,
    theme: &'t Theme,
    lines: Vec<Line<'static>>,
}

impl<'t> Info<'t> {
    pub(crate) const fn new(width: usize, theme: &'t Theme) -> Self {
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
    pub(crate) const fn label_width(mut self, columns: usize) -> Self {
        self.label_width = columns;
        self
    }

    /// Wrap long values under themselves instead of cutting them, for a
    /// view that shows the whole of what it describes.
    #[must_use]
    pub(crate) const fn wrap_values(mut self) -> Self {
        self.wrap_values = true;
        self
    }

    /// What the document describes, at its top: bold `foreground`.
    pub(crate) fn title(&mut self, text: &str) {
        let style = Style::default()
            .fg(self.theme.colors.foreground)
            .add_modifier(Modifier::BOLD);
        self.flush(text, style);
    }

    /// How the thing is doing, under its title: the accent.
    pub(crate) fn status(&mut self, text: &str) {
        self.flush(text, Style::default().fg(self.theme.colors.success));
    }

    /// A muted remark at the left edge, wrapped.
    pub(crate) fn note(&mut self, text: &str) {
        let style = self.muted();
        for line in wrap(text, self.width) {
            self.lines.push(Line::from(Span::styled(line, style)));
        }
    }

    /// A section's title, after a blank row unless it opens the document,
    /// then its muted `note`.
    pub(crate) fn section(&mut self, title: &str, note: Option<&str>) {
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

    /// `  label     value`: the muted label in its column, then the value,
    /// cut to fit (or wrapped under itself, see [`Self::wrap_values`]). A
    /// fact without a value shows a muted `—`.
    pub(crate) fn fact(&mut self, label: &str, value: &str, kind: Value) {
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
    pub(crate) fn bar(&mut self, label: &str, percent: u8, value: &str, kind: Value) {
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
    pub(crate) fn item(&mut self, text: &str, detail: Option<&str>) {
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
    pub(crate) fn items(&mut self, items: &[String]) {
        if items.is_empty() {
            self.none();
        }
        for item in items {
            self.item(item, None);
        }
    }

    /// `  none`, muted: a section with nothing in it.
    pub(crate) fn none(&mut self) {
        self.text("none", Value::Quiet);
    }

    /// Prose in a section, indented like its facts and wrapped. Blank
    /// lines in `text` are kept.
    pub(crate) fn text(&mut self, text: &str, kind: Value) {
        let room = self.width.saturating_sub(INDENT.len());
        let style = kind.style(self.theme);
        for paragraph in text.lines() {
            if paragraph.trim().is_empty() {
                self.blank();
                continue;
            }
            for line in wrap(paragraph, room) {
                self.lines
                    .push(Line::from(vec![Span::raw(INDENT), Span::styled(line, style)]));
            }
        }
    }

    /// `  ✓ name      detail`: an item marked by a glyph in `color`, the
    /// one colored cell, with a muted detail. Names are padded to
    /// `name_width` so the details line up.
    pub(crate) fn marked(
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
    pub(crate) fn actions(&mut self, actions: &[(char, &str)]) {
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
    pub(crate) fn indented(&mut self, lines: impl IntoIterator<Item = Line<'static>>) {
        for mut line in lines {
            line.spans.insert(0, Span::raw(INDENT));
            self.lines.push(line);
        }
    }

    /// Columns left for lines passed to [`Self::indented`].
    pub(crate) const fn indented_width(&self) -> usize {
        self.width.saturating_sub(INDENT.len())
    }

    pub(crate) fn blank(&mut self) {
        self.lines.push(Line::default());
    }

    pub(crate) fn into_lines(self) -> Vec<Line<'static>> {
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
pub(crate) fn truncate_line(text: &str, width: usize) -> String {
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
fn wrap(text: &str, width: usize) -> Vec<String> {
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
mod tests {
    use super::*;

    fn text(line: &Line<'_>) -> String {
        line.spans.iter().map(|span| span.content.as_ref()).collect()
    }

    fn span<'a>(line: &'a Line<'_>, content: &str) -> &'a Span<'a> {
        line.spans
            .iter()
            .find(|span| span.content.trim() == content)
            .unwrap_or_else(|| panic!("no {content:?} in {:?}", text(line)))
    }

    #[test]
    fn a_section_title_is_gray_regular_and_flush_left() {
        let theme = Theme::cyber();
        let mut info = Info::new(40, &theme);
        info.section("STATUS", Some("2/12"));
        let lines = info.into_lines();
        assert_eq!(lines.len(), 1, "no blank row above the first section");
        assert_eq!(text(&lines[0]), "STATUS  2/12");
        let title = span(&lines[0], "STATUS");
        assert_eq!(title.style.fg, Some(theme.colors.secondary));
        assert!(!title.style.add_modifier.contains(Modifier::BOLD));
        assert_eq!(span(&lines[0], "2/12").style.fg, Some(theme.colors.muted));
    }

    #[test]
    fn every_later_section_follows_one_blank_row() {
        let theme = Theme::cyber();
        let mut info = Info::new(40, &theme);
        info.title("boxd");
        info.section("MACHINE", None);
        info.fact("Region", "fra", Value::Plain);
        info.section("COST", None);
        let rows: Vec<String> = info.into_lines().iter().map(text).collect();
        assert_eq!(rows, ["boxd", "", "MACHINE", "  Region    fra", "", "COST"]);
    }

    #[test]
    fn a_fact_pads_its_label_and_cuts_its_value() {
        let theme = Theme::cyber();
        let mut info = Info::new(20, &theme);
        info.fact("Region", "frankfurt-am-main", Value::Plain);
        info.fact("A long label", "x", Value::Plain);
        let lines = info.into_lines();
        assert_eq!(text(&lines[0]), "  Region    frankfu…");
        assert_eq!(text(&lines[1]), "  A long l… x", "a space after the label");
        assert_eq!(span(&lines[0], "Region").style.fg, Some(theme.colors.muted));
    }

    #[test]
    fn a_wide_label_column_and_wrapped_values_keep_the_column() {
        let theme = Theme::cyber();
        let mut info = Info::new(24, &theme).label_width(14).wrap_values();
        info.fact("Context hash", &"ab".repeat(8), Value::Quiet);
        info.fact("Agent id", "", Value::Plain);
        let lines = info.into_lines();
        let rows: Vec<String> = lines.iter().map(text).collect();
        assert_eq!(
            rows,
            [
                "  Context hash  abababab",
                "                abababab",
                "  Agent id      —",
            ]
        );
        assert_eq!(span(&lines[2], "—").style.fg, Some(theme.colors.muted));
    }

    #[test]
    fn values_are_plain_unless_flagged_quiet_or_failed() {
        let theme = Theme::cyber();
        let mut info = Info::new(40, &theme);
        for (value, kind) in [
            ("ask", Value::Plain),
            ("yolo", Value::Flag),
            ("0123abcd", Value::Quiet),
            ("denied", Value::Error),
        ] {
            info.fact("x", value, kind);
        }
        let colors: Vec<_> = info
            .into_lines()
            .iter()
            .map(|line| line.spans.last().and_then(|span| span.style.fg))
            .collect();
        let c = &theme.colors;
        assert_eq!(
            colors,
            [
                Some(c.foreground),
                Some(c.success),
                Some(c.muted),
                Some(c.error)
            ]
        );
        assert_eq!(Value::flag_if(true), Value::Flag);
        assert_eq!(Value::flag_if(false), Value::Plain);
    }

    #[test]
    fn a_bar_fills_in_the_accent() {
        let theme = Theme::cyber();
        let mut info = Info::new(60, &theme);
        info.bar("Disk", 91, "91 GiB of 100 GiB", Value::Flag);
        let lines = info.into_lines();
        assert_eq!(text(&lines[0]), "  Disk      ██████████ 91 GiB of 100 GiB");
        assert_eq!(span(&lines[0], "██████████").style.fg, Some(theme.colors.success));
        let mut info = Info::new(60, &theme);
        info.bar("CPU", 37, "37%", Value::Plain);
        let lines = info.into_lines();
        assert_eq!(text(&lines[0]), "  CPU       ████░░░░░░ 37%");
        assert_eq!(span(&lines[0], "░░░░░░").style.fg, Some(theme.colors.faint));
    }

    #[test]
    fn a_marked_item_colors_only_its_glyph() {
        let theme = Theme::cyber();
        let mut info = Info::new(40, &theme);
        info.marked(("✓", theme.colors.success), ("read", 6), "on   rule");
        let lines = info.into_lines();
        assert_eq!(text(&lines[0]), "  ✓ read    on   rule");
        assert_eq!(span(&lines[0], "✓").style.fg, Some(theme.colors.success));
        assert_eq!(span(&lines[0], "read").style.fg, Some(theme.colors.foreground));
        assert_eq!(span(&lines[0], "on   rule").style.fg, Some(theme.colors.muted));
    }

    #[test]
    fn items_and_prose_are_indented_and_lists_say_none() {
        let theme = Theme::cyber();
        let mut info = Info::new(16, &theme);
        info.items(&[]);
        info.item("Zode", Some("~/code/zode"));
        info.text("SpawnAgent, ControlAgent, ReadAgent", Value::Plain);
        let rows: Vec<String> = info.into_lines().iter().map(text).collect();
        assert_eq!(
            rows,
            [
                "  none",
                "  Zode  ~/code/…",
                "  SpawnAgent,",
                "  ControlAgent,",
                "  ReadAgent",
            ]
        );
    }

    #[test]
    fn actions_follow_a_blank_row() {
        let theme = Theme::cyber();
        let mut info = Info::new(40, &theme);
        info.status("● ready");
        info.actions(&[('p', "pull changes"), ('s', "sleep")]);
        let lines = info.into_lines();
        assert_eq!(text(&lines[2]), "[p] pull changes  [s] sleep");
        assert_eq!(span(&lines[2], "[p]").style.fg, Some(theme.colors.primary));
        assert_eq!(span(&lines[2], "sleep").style.fg, Some(theme.colors.muted));
        assert_eq!(span(&lines[0], "● ready").style.fg, Some(theme.colors.success));
    }

    #[test]
    fn a_cut_line_keeps_its_spacing_and_loses_its_breaks() {
        assert_eq!(truncate_line("on   rule", 20), "on   rule");
        assert_eq!(truncate_line("one\ntwo  ", 20), "one two");
        assert_eq!(truncate_line("↑↓ scroll  ⇧←→ tabs", 12), "↑↓ scroll  …");
        assert_eq!(truncate_line("abc", 0), "");
    }

    #[test]
    fn words_longer_than_a_line_are_broken_at_its_edge() {
        assert_eq!(wrap("abcdefgh ij", 3), ["abc", "def", "gh", "ij"]);
        assert_eq!(wrap("", 3), Vec::<String>::new());
    }
}
