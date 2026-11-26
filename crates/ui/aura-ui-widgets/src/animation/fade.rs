//! Fading content in from the background, for what comes on screen after
//! a wait (a log, a chat), so it doesn't appear with a jolt.
//!
//! A fade only moves colours: symbols are never touched, so anything that
//! read the cells' text before the fade (prose rows, copy) stays valid.

use super::{mix, rgb};
use crate::themes::Theme;
use ratatui::{buffer::Buffer, layout::Rect, style::Color, text::Line};

/// `color` drawn `part / whole` of the way from `from` to it. Colours with
/// no fixed value are left as they are.
fn toward(from: (u8, u8, u8), color: Color, (part, whole): (usize, usize)) -> Color {
    rgb(color).map_or(color, |to| mix(from, to, (part.min(whole), whole.max(1))))
}

/// `lines` drawn `progress` (`part / whole`) of the way from the
/// background to their colours.
pub fn fade_in(lines: &mut [Line<'static>], progress: (usize, usize), theme: &Theme) {
    let Some(from) = rgb(theme.colors.background) else {
        return;
    };
    for line in lines {
        for span in &mut line.spans {
            span.style.fg = span.style.fg.map(|c| toward(from, c, progress));
            span.style.bg = span.style.bg.map(|c| toward(from, c, progress));
        }
    }
}

/// The cells of `area` in `buf` drawn `progress` (`part / whole`) of the
/// way from the background to their colours, for content already drawn.
/// Cells outside the buffer are skipped.
pub fn fade_area(buf: &mut Buffer, area: Rect, progress: (usize, usize), theme: &Theme) {
    let Some(from) = rgb(theme.colors.background) else {
        return;
    };
    let area = area.intersection(buf.area);
    for y in area.top()..area.bottom() {
        for x in area.left()..area.right() {
            if let Some(cell) = buf.cell_mut((x, y)) {
                cell.fg = toward(from, cell.fg, progress);
                cell.bg = toward(from, cell.bg, progress);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::style::Style;
    use ratatui::text::Span;

    #[test]
    fn a_fade_runs_from_the_background_to_the_colours() {
        let theme = Theme::default();
        let line = || {
            vec![Line::from(Span::styled(
                "x",
                Style::default().fg(theme.colors.text),
            ))]
        };
        let fg = |lines: &[Line<'static>]| lines[0].spans[0].style.fg;
        let mut start = line();
        fade_in(&mut start, (0, 180), &theme);
        let mut end = line();
        fade_in(&mut end, (180, 180), &theme);
        if rgb(theme.colors.background).is_some() && rgb(theme.colors.text).is_some() {
            let as_rgb = |c: Option<Color>| c.and_then(rgb);
            assert_eq!(as_rgb(fg(&start)), rgb(theme.colors.background));
            assert_eq!(as_rgb(fg(&end)), rgb(theme.colors.text));
        }
    }

    #[test]
    fn fading_an_area_moves_its_colours_and_keeps_its_text() {
        let mut theme = Theme::default();
        theme.colors.background = Color::Rgb(0, 0, 0);
        let text = Color::Rgb(200, 100, 50);
        let area = Rect::new(0, 0, 3, 1);
        let drawn = || {
            let mut buf = Buffer::empty(area);
            buf.set_string(0, 0, "abc", Style::default().fg(text));
            buf
        };

        let mut start = drawn();
        fade_area(&mut start, area, (0, 180), &theme);
        let mut half = drawn();
        fade_area(&mut half, area, (90, 180), &theme);
        let mut end = drawn();
        fade_area(&mut end, area, (180, 180), &theme);

        assert_eq!(start[(1, 0)].fg, Color::Rgb(0, 0, 0));
        assert_eq!(half[(1, 0)].fg, Color::Rgb(100, 50, 25));
        assert_eq!(end[(1, 0)].fg, text);
        assert_eq!(start[(1, 0)].symbol(), "b", "only colours change");
        assert_eq!(start[(1, 0)].bg, Color::Reset, "no fixed colour, untouched");
    }

    #[test]
    fn fading_past_the_buffer_skips_what_is_outside() {
        let theme = Theme::default();
        let mut buf = Buffer::empty(Rect::new(0, 0, 2, 1));
        fade_area(&mut buf, Rect::new(1, 0, 10, 5), (0, 180), &theme);
        assert_eq!(buf[(0, 0)].symbol(), " ");
    }
}
