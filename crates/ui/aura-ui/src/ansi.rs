//! ANSI escape sequences in captured terminal output: walk the text as
//! printable characters and SGR parameters, or strip every escape sequence.
//!
//! Renderer-agnostic: turning SGR parameters into styles is up to the
//! frontend.

/// One piece of terminal output.
pub enum Piece<'a> {
    /// A printable character, `\n` or `\t`.
    Char(char),
    /// Parameters of an SGR (`ESC [ ... m`) sequence.
    Sgr(&'a str),
}

/// Walk `text`, yielding characters and SGR parameters and skipping every
/// other escape sequence and control character.
pub fn walk<'a>(text: &'a str, mut on_piece: impl FnMut(Piece<'a>)) {
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strips_escape_sequences() {
        assert_eq!(
            strip_ansi("\x1b[01;34msrc\x1b[0m  a\tb\n\x1b]8;;file:///x\x07link\x1b]8;;\x1b\\\x07"),
            "src  a\tb\nlink"
        );
        assert_eq!(strip_ansi("\x1b[2Kdone\x1b(B"), "done");
    }
}
