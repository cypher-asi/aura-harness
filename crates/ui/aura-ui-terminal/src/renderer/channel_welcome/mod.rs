//! An empty channel's chat: its `#slug` as a banner, over its
//! coordinator's welcome ([`ChannelWelcome`]).
//!
//! The banner takes the biggest form that fits the chat's text area,
//! after room for the welcome: [`Size::Large`] on one line, then
//! [`Size::Small`], then small lines broken after the slug's hyphens,
//! then the plain `#slug` in bold. It never draws past the area.

mod font;
#[cfg(test)]
mod tests;

use super::text::wrap_words;
use crate::{app::ChannelWelcome, Theme};
use font::Size;
use ratatui::{
    layout::Rect,
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::Paragraph,
    Frame,
};

/// Most lines a slug broken at its hyphens is drawn on; past that it
/// reads as the plain `#slug`.
const MAX_BANNER_LINES: usize = 3;

/// How the banner spells a slug.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Banner {
    /// Pixel art, a string per row, its lines a blank row apart
    Art(Vec<String>),
    /// The plain `#slug`, in bold, cut to fit
    Plain(String),
}

impl Banner {
    fn rows(&self) -> usize {
        match self {
            Self::Art(rows) => rows.len(),
            Self::Plain(_) => 1,
        }
    }
}

/// The banner of `slug` that fits `width` cells across and `height`
/// rows down.
fn fit_banner(slug: &str, width: usize, height: usize) -> Banner {
    if font::draws(slug) {
        for size in [Size::Large, Size::Small] {
            if size.width(slug) <= width && size.rows() <= height {
                if let Some(rows) = font::render(slug, size) {
                    return Banner::Art(rows);
                }
            }
        }
        if let Some(rows) = broken_at_hyphens(slug, width, height) {
            return Banner::Art(rows);
        }
    }
    Banner::Plain(plain(slug, width))
}

/// `slug` drawn small on two or more lines broken after its hyphens,
/// each line fitting `width`, all of them `height`. `None` when a part
/// is too wide or the lines too many.
fn broken_at_hyphens(slug: &str, width: usize, height: usize) -> Option<Vec<String>> {
    let mut lines: Vec<String> = Vec::new();
    for part in slug.split_inclusive('-') {
        match lines.last_mut() {
            Some(line) if Size::Small.width(&format!("{line}{part}")) <= width => {
                line.push_str(part);
            }
            _ if Size::Small.width(part) <= width => lines.push(part.to_string()),
            _ => return None,
        }
    }
    let tall = lines.len() * Size::Small.rows() + lines.len().saturating_sub(1);
    if lines.len() < 2 || lines.len() > MAX_BANNER_LINES || tall > height {
        return None;
    }
    let mut rows = Vec::with_capacity(tall);
    for (at, line) in lines.iter().enumerate() {
        if at > 0 {
            rows.push(String::new());
        }
        rows.extend(font::render(line, Size::Small)?);
    }
    Some(rows)
}

/// `#slug` cut to `width` characters, marked with `…` when cut.
fn plain(slug: &str, width: usize) -> String {
    let full = format!("#{slug}");
    if full.chars().count() <= width {
        return full;
    }
    let keep = width.saturating_sub(1);
    let cut: String = full.chars().take(keep).collect();
    if width == 0 {
        String::new()
    } else {
        format!("{cut}…")
    }
}

/// The welcome's lines at `width`: its paragraphs a blank row apart, then
/// the hint, with whether each is the hint.
fn welcome_lines(welcome: &ChannelWelcome, width: usize) -> Vec<(String, bool)> {
    let mut lines = Vec::new();
    for paragraph in &welcome.paragraphs {
        if !lines.is_empty() {
            lines.push((String::new(), false));
        }
        lines.extend(wrap_words(paragraph, width).into_iter().map(|l| (l, false)));
    }
    lines.push((String::new(), false));
    lines.extend(
        wrap_words(&welcome.hint, width)
            .into_iter()
            .map(|l| (l, true)),
    );
    lines
}

/// Draw `welcome` in `area`, the chat's text area: the banner in
/// `primary`, then a blank row, the welcome in `text` and its hint in
/// `muted`. What doesn't fit below is clipped.
pub(super) fn render_channel_welcome(
    frame: &mut Frame,
    area: Rect,
    welcome: &ChannelWelcome,
    theme: &Theme,
) {
    let width = usize::from(area.width);
    let words = welcome_lines(welcome, width);
    let room = usize::from(area.height).saturating_sub(words.len() + 1);
    let banner = fit_banner(&welcome.slug, width, room);
    let mut lines: Vec<Line> = Vec::with_capacity(banner.rows() + 1 + words.len());
    match banner {
        Banner::Art(rows) => {
            let art = Style::default().fg(theme.colors.primary);
            lines.extend(
                rows.into_iter()
                    .map(|row| Line::from(Span::styled(row, art))),
            );
        }
        Banner::Plain(text) => {
            let bold = Style::default()
                .fg(theme.colors.foreground)
                .add_modifier(Modifier::BOLD);
            lines.push(Line::from(Span::styled(text, bold)));
        }
    }
    lines.push(Line::default());
    let text = Style::default().fg(theme.colors.text);
    let hint = Style::default().fg(theme.colors.muted);
    lines.extend(
        words.into_iter().map(|(line, is_hint)| {
            Line::from(Span::styled(line, if is_hint { hint } else { text }))
        }),
    );
    frame.render_widget(Paragraph::new(lines), area);
}
