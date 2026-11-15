//! ANSI-styled command output turned into colored runs of text.
//!
//! Only SGR sequences (colors, bold, dim, reset) change the style; every
//! other escape sequence (cursor movement, OSC titles and links) is
//! dropped, as are carriage returns that redraw a line.

use crate::theme;
use iced::Color;

/// A run of text with one style.
#[derive(Debug, Clone, PartialEq)]
pub struct Run {
    pub text: String,
    pub color: Option<Color>,
    pub bold: bool,
    pub dim: bool,
}

#[derive(Debug, Clone, Copy, Default, PartialEq)]
struct Style {
    color: Option<Color>,
    bold: bool,
    dim: bool,
}

/// Split `input` into styled runs. Runs keep their newlines.
#[must_use]
pub fn parse(input: &str) -> Vec<Run> {
    let mut runs = Vec::new();
    let mut style = Style::default();
    let mut text = String::new();
    let mut chars = input.chars().peekable();

    let flush = |text: &mut String, style: Style, runs: &mut Vec<Run>| {
        if !text.is_empty() {
            runs.push(Run {
                text: std::mem::take(text),
                color: style.color,
                bold: style.bold,
                dim: style.dim,
            });
        }
    };

    while let Some(c) = chars.next() {
        match c {
            '\x1b' => match chars.peek() {
                Some('[') => {
                    chars.next();
                    let mut params = String::new();
                    let mut end = None;
                    for c in chars.by_ref() {
                        if ('\x40'..='\x7e').contains(&c) {
                            end = Some(c);
                            break;
                        }
                        params.push(c);
                    }
                    if end == Some('m') {
                        flush(&mut text, style, &mut runs);
                        apply_sgr(&mut style, &params);
                    }
                }
                Some(']') => {
                    // OSC: runs to BEL or ST (ESC \).
                    chars.next();
                    while let Some(c) = chars.next() {
                        if c == '\x07' {
                            break;
                        }
                        if c == '\x1b' && chars.peek() == Some(&'\\') {
                            chars.next();
                            break;
                        }
                    }
                }
                Some(_) => {
                    chars.next();
                }
                None => {}
            },
            '\r' => {
                if chars.peek() != Some(&'\n') {
                    // A bare CR redraws the line (progress bars): keep the last draw.
                    if let Some(start) = text.rfind('\n') {
                        text.truncate(start + 1);
                    } else {
                        text.clear();
                    }
                }
            }
            '\t' => text.push_str("    "),
            c if c.is_control() && c != '\n' => {}
            c => text.push(c),
        }
    }
    flush(&mut text, style, &mut runs);
    runs
}

/// Text with every escape sequence removed.
#[must_use]
pub fn strip(input: &str) -> String {
    parse(input).into_iter().map(|r| r.text).collect()
}

fn apply_sgr(style: &mut Style, params: &str) {
    let codes: Vec<u16> = if params.is_empty() {
        vec![0]
    } else {
        params.split(';').map(|p| p.parse().unwrap_or(0)).collect()
    };
    let mut i = 0;
    while i < codes.len() {
        match codes[i] {
            0 => *style = Style::default(),
            1 => style.bold = true,
            2 => style.dim = true,
            22 => {
                style.bold = false;
                style.dim = false;
            }
            30..=37 => style.color = Some(basic(codes[i] - 30, false)),
            90..=97 => style.color = Some(basic(codes[i] - 90, true)),
            39 => style.color = None,
            38 => match codes.get(i + 1) {
                Some(5) => {
                    if let Some(&n) = codes.get(i + 2) {
                        style.color = Some(indexed(n));
                    }
                    i += 2;
                }
                Some(2) => {
                    if let (Some(&r), Some(&g), Some(&b)) =
                        (codes.get(i + 2), codes.get(i + 3), codes.get(i + 4))
                    {
                        style.color = Some(Color::from_rgb8(
                            r.min(255) as u8,
                            g.min(255) as u8,
                            b.min(255) as u8,
                        ));
                    }
                    i += 4;
                }
                _ => {}
            },
            // Background colors and their extended forms are not drawn.
            48 => match codes.get(i + 1) {
                Some(5) => i += 2,
                Some(2) => i += 4,
                _ => {}
            },
            _ => {}
        }
        i += 1;
    }
}

/// The eight base colors mapped onto the AURA palette.
fn basic(n: u16, bright: bool) -> Color {
    let color = match n {
        0 => theme::FAINT,
        1 => theme::RED,
        2 => theme::CYAN,
        3 => Color::from_rgb8(240, 200, 80),
        4 => theme::BLUE,
        5 => theme::PURPLE,
        6 => Color::from_rgb8(80, 220, 240),
        _ => theme::TEXT,
    };
    if bright {
        Color {
            r: (color.r * 1.15).min(1.0),
            g: (color.g * 1.15).min(1.0),
            b: (color.b * 1.15).min(1.0),
            a: 1.0,
        }
    } else {
        color
    }
}

/// xterm 256-color table.
fn indexed(n: u16) -> Color {
    match n {
        0..=7 => basic(n, false),
        8..=15 => basic(n - 8, true),
        16..=231 => {
            let n = n - 16;
            let level = |v: u16| if v == 0 { 0 } else { channel(55 + v * 40) };
            Color::from_rgb8(level(n / 36), level((n / 6) % 6), level(n % 6))
        }
        232..=255 => {
            let v = channel(8 + (n - 232) * 10);
            Color::from_rgb8(v, v, v)
        }
        _ => theme::TEXT,
    }
}

fn channel(v: u16) -> u8 {
    u8::try_from(v).unwrap_or(u8::MAX)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plain_text_is_one_run() {
        let runs = parse("hello\nworld");
        assert_eq!(runs.len(), 1);
        assert_eq!(runs[0].text, "hello\nworld");
        assert_eq!(runs[0].color, None);
    }

    #[test]
    fn sgr_colors_split_runs() {
        let runs = parse("a\x1b[31mred\x1b[0mb");
        let texts: Vec<_> = runs.iter().map(|r| r.text.as_str()).collect();
        assert_eq!(texts, ["a", "red", "b"]);
        assert_eq!(runs[1].color, Some(theme::RED));
        assert_eq!(runs[2].color, None);
    }

    #[test]
    fn bold_and_truecolor() {
        let runs = parse("\x1b[1;38;2;10;20;30mx");
        assert!(runs[0].bold);
        assert_eq!(runs[0].color, Some(Color::from_rgb8(10, 20, 30)));
    }

    #[test]
    fn cursor_and_osc_sequences_are_dropped() {
        assert_eq!(strip("\x1b[2K\x1b]0;title\x07ok\x1b[1A"), "ok");
    }

    #[test]
    fn carriage_return_keeps_last_redraw() {
        assert_eq!(strip("10%\r50%\r100%\ndone\r\n"), "100%\ndone\n");
    }
}
