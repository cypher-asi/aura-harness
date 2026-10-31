//! ANSI escape sequences in captured terminal output: turn SGR color and
//! style codes into styled spans, or strip every escape sequence.

use ratatui::{
    style::{Color, Modifier, Style},
    text::Span,
};

/// Columns between tab stops.
const TAB_WIDTH: usize = 8;

/// One piece of terminal output.
enum Piece<'a> {
    /// A printable character, `\n` or `\t`.
    Char(char),
    /// Parameters of an SGR (`ESC [ ... m`) sequence.
    Sgr(&'a str),
}

/// Walk `text`, yielding characters and SGR parameters and skipping every
/// other escape sequence and control character.
fn walk<'a>(text: &'a str, mut on_piece: impl FnMut(Piece<'a>)) {
    let mut chars = text.char_indices().peekable();
    while let Some((_, c)) = chars.next() {
        match c {
            '\x1b' => match chars.next() {
                Some((start, '[')) => {
                    let params_start = start + 1;
                    let mut end = None;
                    for (i, c) in chars.by_ref() {
                        if ('\x40'..='\x7e').contains(&c) {
                            end = Some((i, c));
                            break;
                        }
                    }
                    if let Some((i, 'm')) = end {
                        on_piece(Piece::Sgr(&text[params_start..i]));
                    }
                }
                Some((_, ']' | 'P' | 'X' | '^' | '_')) => {
                    while let Some((_, c)) = chars.next() {
                        if c == '\x07' {
                            break;
                        }
                        if c == '\x1b' && chars.peek().is_some_and(|(_, c)| *c == '\\') {
                            chars.next();
                            break;
                        }
                    }
                }
                // `ESC ( B` and friends: intermediates, then one final byte.
                Some((_, '\x20'..='\x2f')) => {
                    for (_, c) in chars.by_ref() {
                        if !('\x20'..='\x2f').contains(&c) {
                            break;
                        }
                    }
                }
                _ => {}
            },
            '\n' | '\t' => on_piece(Piece::Char(c)),
            c if c.is_control() => {}
            c => on_piece(Piece::Char(c)),
        }
    }
}

/// `text` without ANSI escape sequences or control characters other than
/// newlines and tabs.
#[must_use]
pub fn strip_ansi(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    walk(text, |piece| {
        if let Piece::Char(c) = piece {
            out.push(c);
        }
    });
    out
}

/// Styled spans for one line of terminal output. SGR codes are applied on
/// top of `base`, and tabs are expanded to spaces.
#[must_use]
pub fn line_spans(line: &str, base: Style) -> Vec<Span<'static>> {
    let mut spans = Vec::new();
    let mut style = base;
    let mut run = String::new();
    let mut column = 0;
    walk(line, |piece| match piece {
        Piece::Char('\n') => {}
        Piece::Char('\t') => {
            let pad = TAB_WIDTH - column % TAB_WIDTH;
            run.push_str(&" ".repeat(pad));
            column += pad;
        }
        Piece::Char(c) => {
            run.push(c);
            column += unicode_width::UnicodeWidthChar::width(c).unwrap_or(0);
        }
        Piece::Sgr(params) => {
            let next = apply_sgr(style, base, params);
            if next != style && !run.is_empty() {
                spans.push(Span::styled(std::mem::take(&mut run), style));
            }
            style = next;
        }
    });
    if !run.is_empty() {
        spans.push(Span::styled(run, style));
    }
    spans
}

fn apply_sgr(mut style: Style, base: Style, params: &str) -> Style {
    let mut codes = params
        .split([';', ':'])
        .map(|code| code.parse::<u16>().unwrap_or(0));
    if params.is_empty() {
        return base;
    }
    while let Some(code) = codes.next() {
        style = match code {
            0 => base,
            1 => style.add_modifier(Modifier::BOLD),
            2 => style.add_modifier(Modifier::DIM),
            3 => style.add_modifier(Modifier::ITALIC),
            4 => style.add_modifier(Modifier::UNDERLINED),
            5 => style.add_modifier(Modifier::SLOW_BLINK),
            6 => style.add_modifier(Modifier::RAPID_BLINK),
            7 => style.add_modifier(Modifier::REVERSED),
            8 => style.add_modifier(Modifier::HIDDEN),
            9 => style.add_modifier(Modifier::CROSSED_OUT),
            21 | 22 => style.remove_modifier(Modifier::BOLD | Modifier::DIM),
            23 => style.remove_modifier(Modifier::ITALIC),
            24 => style.remove_modifier(Modifier::UNDERLINED),
            25 => style.remove_modifier(Modifier::SLOW_BLINK | Modifier::RAPID_BLINK),
            27 => style.remove_modifier(Modifier::REVERSED),
            28 => style.remove_modifier(Modifier::HIDDEN),
            29 => style.remove_modifier(Modifier::CROSSED_OUT),
            30..=37 => style.fg(indexed(code - 30)),
            38 => match extended_color(&mut codes) {
                Some(color) => style.fg(color),
                None => style,
            },
            39 => Style {
                fg: base.fg,
                ..style
            },
            40..=47 => style.bg(indexed(code - 40)),
            48 => match extended_color(&mut codes) {
                Some(color) => style.bg(color),
                None => style,
            },
            49 => Style {
                bg: base.bg,
                ..style
            },
            90..=97 => style.fg(indexed(code - 90 + 8)),
            100..=107 => style.bg(indexed(code - 100 + 8)),
            _ => style,
        };
    }
    style
}

fn indexed(code: u16) -> Color {
    Color::Indexed(u8::try_from(code).unwrap_or(u8::MAX))
}

/// The color after a `38` / `48`: `5;n` (256-color) or `2;r;g;b` (24-bit).
fn extended_color(codes: &mut impl Iterator<Item = u16>) -> Option<Color> {
    let mut component = || codes.next().and_then(|c| u8::try_from(c).ok());
    match component()? {
        5 => component().map(Color::Indexed),
        2 => Some(Color::Rgb(component()?, component()?, component()?)),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn texts(spans: &[Span<'_>]) -> Vec<String> {
        spans.iter().map(|s| s.content.to_string()).collect()
    }

    #[test]
    fn strips_escape_sequences() {
        assert_eq!(
            strip_ansi("\x1b[01;34msrc\x1b[0m  a\tb\n\x1b]8;;file:///x\x07link\x1b]8;;\x1b\\\x07"),
            "src  a\tb\nlink"
        );
        assert_eq!(strip_ansi("\x1b[2Kdone\x1b(B"), "done");
    }

    #[test]
    fn colors_become_styles() {
        let base = Style::default().fg(Color::White);
        let spans = line_spans("\x1b[01;34msrc\x1b[0m  Cargo.toml", base);
        assert_eq!(texts(&spans), ["src", "  Cargo.toml"]);
        assert_eq!(
            spans[0].style,
            base.fg(Color::Indexed(4)).add_modifier(Modifier::BOLD)
        );
        assert_eq!(spans[1].style, base);
    }

    #[test]
    fn extended_colors_and_resets() {
        let base = Style::default();
        let spans = line_spans("\x1b[38;5;208ma\x1b[48;2;1;2;3mb\x1b[39;49mc", base);
        assert_eq!(texts(&spans), ["a", "b", "c"]);
        assert_eq!(spans[0].style.fg, Some(Color::Indexed(208)));
        assert_eq!(spans[1].style.bg, Some(Color::Rgb(1, 2, 3)));
        assert_eq!(spans[2].style, base);
    }

    #[test]
    fn bright_colors_and_tabs() {
        let spans = line_spans("ab\t\x1b[91mc", Style::default());
        assert_eq!(texts(&spans), ["ab      ", "c"]);
        assert_eq!(spans[1].style.fg, Some(Color::Indexed(9)));
    }

    #[test]
    fn truncated_sequence_is_dropped() {
        assert_eq!(texts(&line_spans("ok\x1b[01;3", Style::default())), ["ok"]);
    }
}
