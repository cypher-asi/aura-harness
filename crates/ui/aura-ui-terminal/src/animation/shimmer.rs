//! A band of light that sweeps across a line of text, frame by frame, to
//! show the agent is still at work.

use ratatui::style::{Color, Style};
use ratatui::text::Span;

/// Characters on each side of the band's center that it lights.
const BAND_RADIUS: usize = 3;
/// Frames the band takes to move one character.
const FRAMES_PER_STEP: usize = 1;
/// Frames the text rests unlit after the band leaves it.
const REST_STEPS: usize = 8;

/// `text` split into spans whose color runs from `base` to `highlight` and
/// back where the band falls on frame `frame`. Every span takes `style`
/// with only its foreground changed.
///
/// When either color is not one RGB can stand for, the band is drawn as
/// characters in `highlight` over the rest in `base`.
#[must_use]
pub fn shimmer_spans(
    text: &str,
    frame: usize,
    style: Style,
    (base, highlight): (Color, Color),
) -> Vec<Span<'static>> {
    let chars: Vec<char> = text.chars().collect();
    if chars.is_empty() {
        return Vec::new();
    }
    // The band starts fully left of the text and leaves it fully on the
    // right, then rests before the next pass.
    let travel = chars.len() + 2 * BAND_RADIUS;
    let cycle = travel + REST_STEPS;
    // The band's center, counted in characters from `BAND_RADIUS` before
    // the text's first one.
    let step = (frame / FRAMES_PER_STEP) % cycle;
    let colors = rgb(base).zip(rgb(highlight));
    // Steps of light from the band's edge to its center.
    let levels = BAND_RADIUS + 1;

    let mut spans: Vec<Span<'static>> = Vec::new();
    let mut run = String::new();
    let mut run_color = None;
    for (i, ch) in chars.iter().enumerate() {
        let distance = (i + BAND_RADIUS).abs_diff(step);
        let color = if step >= travel || distance > BAND_RADIUS {
            base
        } else {
            // Full at the center, fading to the base color at the edges.
            let lit = levels - distance;
            match colors {
                Some((from, to)) => mix(from, to, (lit, levels)),
                None if 2 * lit >= levels => highlight,
                None => base,
            }
        };
        if run_color.is_some_and(|c| c != color) {
            spans.push(Span::styled(
                std::mem::take(&mut run),
                style.fg(run_color.unwrap_or(base)),
            ));
        }
        run_color = Some(color);
        run.push(*ch);
    }
    spans.push(Span::styled(run, style.fg(run_color.unwrap_or(base))));
    spans
}

/// `color` as red, green and blue, when it has a fixed value.
pub(crate) const fn rgb(color: Color) -> Option<(u8, u8, u8)> {
    match color {
        Color::Rgb(r, g, b) => Some((r, g, b)),
        Color::White => Some((255, 255, 255)),
        Color::Gray => Some((192, 192, 192)),
        Color::DarkGray => Some((128, 128, 128)),
        Color::Black => Some((0, 0, 0)),
        _ => None,
    }
}

/// The color `part / whole` of the way from `from` to `to`, `part` at most
/// `whole`.
pub(crate) fn mix(from: (u8, u8, u8), to: (u8, u8, u8), (part, whole): (usize, usize)) -> Color {
    let channel = |a: u8, b: u8| {
        let (a, b) = (usize::from(a), usize::from(b));
        let value = if b >= a {
            a + (b - a) * part / whole
        } else {
            a - (a - b) * part / whole
        };
        // Between `a` and `b`, so always a valid channel.
        u8::try_from(value).unwrap_or(u8::MAX)
    };
    Color::Rgb(
        channel(from.0, to.0),
        channel(from.1, to.1),
        channel(from.2, to.2),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    const BASE: Color = Color::Rgb(100, 100, 100);
    const LIGHT: Color = Color::Rgb(250, 250, 250);

    fn text(spans: &[Span<'_>]) -> String {
        spans.iter().map(|s| s.content.as_ref()).collect()
    }

    fn colors(spans: &[Span<'_>]) -> Vec<Color> {
        spans
            .iter()
            .flat_map(|s| {
                let fg = s.style.fg.unwrap_or(Color::Reset);
                std::iter::repeat_n(fg, s.content.chars().count())
            })
            .collect()
    }

    #[test]
    fn keeps_the_text_on_every_frame() {
        for frame in 0..40 {
            let spans = shimmer_spans("Thinking…", frame, Style::default(), (BASE, LIGHT));
            assert_eq!(text(&spans), "Thinking…");
        }
    }

    #[test]
    fn band_peaks_at_its_center_and_moves() {
        let at = |frame| {
            colors(&shimmer_spans(
                "abcdefgh",
                frame,
                Style::default(),
                (BASE, LIGHT),
            ))
        };
        // On this frame the band's center is on the first character.
        let first = at(BAND_RADIUS);
        assert_eq!(first[0], LIGHT);
        assert_eq!(first[BAND_RADIUS + 1], BASE);
        let next = at(BAND_RADIUS + 1);
        assert_eq!(next[1], LIGHT);
        assert_ne!(next[0], LIGHT);
    }

    #[test]
    fn rests_unlit_between_passes() {
        let len = "abc".chars().count();
        let resting = len + 2 * BAND_RADIUS;
        let spans = shimmer_spans("abc", resting, Style::default(), (BASE, LIGHT));
        assert!(colors(&spans).iter().all(|c| *c == BASE));
        assert_eq!(spans.len(), 1);
    }

    #[test]
    fn named_colors_without_rgb_still_light_the_band() {
        let spans = shimmer_spans(
            "abcdef",
            BAND_RADIUS + 2,
            Style::default(),
            (Color::Blue, Color::Cyan),
        );
        let colors = colors(&spans);
        assert_eq!(colors[2], Color::Cyan);
        assert_eq!(colors[5], Color::Blue);
    }

    #[test]
    fn empty_text_has_no_spans() {
        assert!(shimmer_spans("", 3, Style::default(), (BASE, LIGHT)).is_empty());
    }
}
