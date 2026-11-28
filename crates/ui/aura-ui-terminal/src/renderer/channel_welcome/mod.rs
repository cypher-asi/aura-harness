//! What an open channel's chat shows while it has no messages yet: the
//! channel's slug as a block-letter banner, then a welcome from its
//! coordinator, under the coordinator's label as a post is:
//!
//! ```text
//!   ████    ██    ██  ██████      ████
//! ██    ██  ██    ██  ██    ██  ██    ██
//! ████████  ██    ██  ██████    ████████
//! ██    ██  ██    ██  ██  ██    ██    ██
//! ██    ██    ████    ██    ██  ██    ██
//!
//! CO2
//! Welcome to #aura. Agentic harness for swarm super intelligence.
//!
//! Tell me what you need and I'll hand it to an agent, or @mention a
//! member to ask them directly.
//! ```
//!
//! It is drawn by the UI and never posted: nothing reaches the session,
//! so the coordinator is not woken, no model is called, and nothing has
//! to remember that a channel was welcomed. The first post or message
//! makes the chat non-empty and the normal conversation takes over.
//!
//! The banner never leaves the area it is given. The welcome's text is
//! laid out first and the banner gets the rows left over; it takes the
//! first of these that fits both ways (see [`fit_banner`]):
//!
//! 1. [`Size::Large`] letters on one line,
//! 2. [`Size::Small`] letters on one line,
//! 3. small letters broken after the slug's hyphens, on at most
//!    [`MAX_BANNER_LINES`] lines,
//! 4. `#slug` as plain bold text, cut with `…` to the width,
//!
//! and with no row left for it there is no banner at all.

mod font;
#[cfg(test)]
mod tests;

use super::chat_panel::{pack_prose, prose_columns};
use super::text::{wrap_words, Measure};
use crate::app::ProseRow;
use crate::events::ChannelSummary;
use crate::{App, Theme};
use font::Size;
use ratatui::{
    buffer::Buffer,
    layout::Rect,
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Paragraph, Widget},
    Frame,
};

/// How the welcome says to get started.
pub(super) const GET_STARTED: &str = "Tell me what you need and I'll hand it to an agent, \
                                       or @mention a member to ask them directly.";

/// The welcome's label when the coordinator is not among the members.
const FALLBACK_AUTHOR: &str = "Coordinator";

/// Most lines a slug broken after its hyphens is drawn on.
const MAX_BANNER_LINES: usize = 3;

/// The coordinator's welcome: who says it, and what, a paragraph each.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Welcome {
    /// The coordinator's code, else its name
    author: String,
    /// What the channel is for, then how to get started
    paragraphs: Vec<String>,
}

/// The welcome `channel`'s coordinator gives.
fn welcome(channel: &ChannelSummary) -> Welcome {
    Welcome {
        author: coordinator_label(channel),
        paragraphs: vec![greeting(channel), GET_STARTED.to_string()],
    }
}

/// The coordinator's label: its code, else its name, as a post's author
/// label reads.
fn coordinator_label(channel: &ChannelSummary) -> String {
    let coordinator = channel
        .members
        .iter()
        .find(|member| member.id == channel.coordinator_id);
    let code = coordinator.and_then(|member| member.code.as_deref());
    let name = coordinator.map(|member| member.name.as_str());
    code.filter(|code| !code.is_empty())
        .or_else(|| name.filter(|name| !name.is_empty()))
        .unwrap_or(FALLBACK_AUTHOR)
        .to_string()
}

/// `Welcome to #slug.`, then what the channel is for when it says.
fn greeting(channel: &ChannelSummary) -> String {
    let hello = format!("Welcome to #{}.", channel.slug);
    match purpose(channel) {
        Some(about) => format!("{hello} {}", sentence(&about)),
        None => hello,
    }
}

/// What the channel is for, on one line: its description, else its
/// title unless that only repeats the slug.
fn purpose(channel: &ChannelSummary) -> Option<String> {
    let flat = |text: &str| text.split_whitespace().collect::<Vec<_>>().join(" ");
    let description = flat(&channel.description);
    if !description.is_empty() {
        return Some(description);
    }
    let title = flat(&channel.title);
    (!title.is_empty() && title != channel.slug).then_some(title)
}

/// `text` ending as a sentence does.
fn sentence(text: &str) -> String {
    if text.ends_with(['.', '!', '?']) {
        text.to_string()
    } else {
        format!("{text}.")
    }
}

/// The slug as the banner draws it.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Banner {
    /// Block letters at `size`, row by row
    Art { size: Size, rows: Vec<String> },
    /// `#slug`, cut to the width
    Plain(String),
}

/// The banner for `slug` in `width` columns and at most `rows` rows: the
/// largest block letters that fit, else `#slug` as plain text. `None`
/// with no room at all, or no slug.
fn fit_banner(slug: &str, width: usize, rows: usize) -> Option<Banner> {
    if slug.is_empty() || width == 0 || rows == 0 {
        return None;
    }
    fit_art(slug, width, rows).or_else(|| Some(Banner::Plain(plain(slug, width))))
}

/// Block letters for `slug` that fit `width` by `rows`: on one line at
/// the largest size that fits, else small and broken after hyphens.
/// `None` when none fit, or the font lacks one of its characters.
fn fit_art(slug: &str, width: usize, rows: usize) -> Option<Banner> {
    for size in Size::ALL {
        if font::text_width(slug, size)? <= width && size.rows() <= rows {
            let drawn = font::draw(slug, size)?;
            return Some(Banner::Art { size, rows: drawn });
        }
    }
    wrapped_art(slug, width, rows)
}

/// `slug` in small letters broken after its hyphens, a blank row between
/// its lines, when that fits `width` by `rows` on at most
/// [`MAX_BANNER_LINES`] lines.
fn wrapped_art(slug: &str, width: usize, rows: usize) -> Option<Banner> {
    let size = Size::Small;
    let lines = hyphen_lines(slug, width, size)?;
    if !(2..=MAX_BANNER_LINES).contains(&lines.len()) {
        return None;
    }
    let height = lines.len() * size.rows() + lines.len() - 1;
    if height > rows {
        return None;
    }
    let mut drawn = Vec::with_capacity(height);
    for (index, line) in lines.iter().enumerate() {
        if index > 0 {
            drawn.push(String::new());
        }
        drawn.extend(font::draw(line, size)?);
    }
    Some(Banner::Art { size, rows: drawn })
}

/// `slug` broken after its hyphens into lines that each fit `width` at
/// `size`, as many parts to a line as fit. `None` when a part is too wide
/// on its own.
fn hyphen_lines(slug: &str, width: usize, size: Size) -> Option<Vec<String>> {
    let mut lines: Vec<String> = Vec::new();
    for part in slug.split_inclusive('-') {
        if let Some(last) = lines.last_mut() {
            let joined = format!("{last}{part}");
            if font::text_width(&joined, size)? <= width {
                *last = joined;
                continue;
            }
        }
        if font::text_width(part, size)? > width {
            return None;
        }
        lines.push(part.to_string());
    }
    Some(lines)
}

/// `#slug`, cut with `…` to `width` columns. The slug is ASCII, so a
/// character is a column.
fn plain(slug: &str, width: usize) -> String {
    let text = format!("#{slug}");
    if text.chars().count() <= width {
        return text;
    }
    let kept: String = text.chars().take(width.saturating_sub(1)).collect();
    format!("{kept}…")
}

/// Draw the welcome of the channel the chat shows into `area`, from its
/// top: the banner, a blank row, then the coordinator's label and words.
/// The words are prose, so a front end with a proportional face sets
/// them as it sets posts; the banner stays on the grid. Nothing outside
/// a coordinated channel.
pub(in crate::renderer) fn render_channel_welcome(
    frame: &mut Frame,
    area: Rect,
    app: &mut App,
    theme: &Theme,
) {
    let Some(channel) = app.coordinated_channel() else {
        return;
    };
    let slug = channel.slug.clone();
    let text = text_lines(&welcome(channel), usize::from(area.width), theme);
    let room = usize::from(area.height).saturating_sub(text.len() + 1);
    let mut lines = fit_banner(&slug, usize::from(area.width), room)
        .map(|banner| banner_lines(banner, theme))
        .unwrap_or_default();
    if !lines.is_empty() {
        lines.push(Line::default());
    }
    let text_start = lines.len();
    lines.extend(text);
    let measure = Measure::prose(app.prose_measure(), theme);
    let prose = draw_lines(
        frame.buffer_mut(),
        area,
        (&lines, text_start),
        measure,
        theme,
    );
    app.set_prose_rows(prose);
}

/// The banner's rows, in bold `primary` as a heading.
fn banner_lines(banner: Banner, theme: &Theme) -> Vec<Line<'static>> {
    let style = Style::default()
        .fg(theme.colors.primary)
        .add_modifier(Modifier::BOLD);
    let rows = match banner {
        Banner::Art { rows, .. } => rows,
        Banner::Plain(text) => vec![text],
    };
    rows.into_iter()
        .map(|row| Line::from(Span::styled(row, style)))
        .collect()
}

/// The welcome's lines in `width` columns: the label in bold `primary`,
/// as a post's author label, then its paragraphs in `text`, wrapped, a
/// blank row between each two.
fn text_lines(welcome: &Welcome, width: usize, theme: &Theme) -> Vec<Line<'static>> {
    if width == 0 {
        return Vec::new();
    }
    let label = Style::default()
        .fg(theme.colors.primary)
        .add_modifier(Modifier::BOLD);
    let body = Style::default().fg(theme.colors.text);
    let mut lines = vec![Line::from(Span::styled(welcome.author.clone(), label))];
    for (index, paragraph) in welcome.paragraphs.iter().enumerate() {
        if index > 0 {
            lines.push(Line::default());
        }
        let wrapped = wrap_words(paragraph, width);
        lines.extend(
            wrapped
                .into_iter()
                .map(|row| Line::from(Span::styled(row, body))),
        );
    }
    lines
}

/// Draw `lines` from the top of `area`, as many as fit, and return the
/// prose rows: those from `text_start` on that hold text.
fn draw_lines(
    buffer: &mut Buffer,
    area: Rect,
    (lines, text_start): (&[Line<'static>], usize),
    measure: Measure,
    theme: &Theme,
) -> Vec<ProseRow> {
    let mut prose = Vec::new();
    for (offset, line) in lines.iter().take(usize::from(area.height)).enumerate() {
        let Ok(dy) = u16::try_from(offset) else {
            break;
        };
        let row = Rect {
            y: area.y + dy,
            height: 1,
            ..area
        };
        Paragraph::new(line.clone()).render(row, buffer);
        if offset < text_start || line.width() == 0 {
            continue;
        }
        let (width, code) = if measure.is_grid() {
            prose_columns(line, row, theme)
        } else {
            pack_prose(buffer, line, (row, 0), measure, theme)
        };
        prose.push(ProseRow::new(buffer, (row.x, row.y), (width, 0), code));
    }
    prose
}
