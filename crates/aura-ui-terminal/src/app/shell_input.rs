//! Highlighting for command lines typed at the terminal prompt.

/// Characters that join or redirect commands.
const OPERATORS: &[char] = &['|', '&', ';', '<', '>'];

/// What a piece of a command line is, for highlighting.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum TokenKind {
    /// Whitespace between tokens
    Space,
    /// A program name (first word of each command)
    Program,
    /// A word starting with `-`
    Flag,
    /// `|`, `&&`, `;`, `>`, ...
    Operator,
    /// A quoted string
    Quoted,
    /// Any other word
    Arg,
}

/// Split a command line into highlight tokens that cover all of `text`.
pub(crate) fn tokens(text: &str) -> Vec<(TokenKind, &str)> {
    let mut tokens = Vec::new();
    let mut expect_program = true;
    let mut rest = text;
    while let Some(c) = rest.chars().next() {
        let (kind, len) = if c.is_whitespace() {
            (TokenKind::Space, run_len(rest, char::is_whitespace))
        } else if OPERATORS.contains(&c) {
            let len = run_len(rest, |c| OPERATORS.contains(&c));
            // A redirect (`>`, `<`) is followed by a file, not a command.
            expect_program = rest[..len].contains(['|', '&', ';']);
            (TokenKind::Operator, len)
        } else if c == '"' || c == '\'' {
            let close = rest[1..].find(c).map_or(rest.len(), |i| i + 2);
            (TokenKind::Quoted, close)
        } else {
            let len = run_len(rest, |c| {
                !c.is_whitespace() && !OPERATORS.contains(&c) && c != '"' && c != '\''
            });
            let kind = if std::mem::take(&mut expect_program) {
                TokenKind::Program
            } else if c == '-' {
                TokenKind::Flag
            } else {
                TokenKind::Arg
            };
            (kind, len)
        };
        let (token, tail) = rest.split_at(len);
        if kind == TokenKind::Quoted {
            expect_program = false;
        }
        tokens.push((kind, token));
        rest = tail;
    }
    tokens
}

/// Byte length of the leading run of `text` matching `pred`.
fn run_len(text: &str, pred: impl Fn(char) -> bool) -> usize {
    text.find(|c| !pred(c)).unwrap_or(text.len())
}

#[cfg(test)]
mod tests {
    use super::{tokens, TokenKind};

    fn kinds(text: &str) -> Vec<(TokenKind, &str)> {
        tokens(text)
            .into_iter()
            .filter(|(kind, _)| *kind != TokenKind::Space)
            .collect()
    }

    #[test]
    fn tokens_cover_the_whole_line() {
        let text = "  ls -la  | head ";
        let joined: String = tokens(text).into_iter().map(|(_, t)| t).collect();
        assert_eq!(joined, text);
    }

    #[test]
    fn tokens_classify_words() {
        use TokenKind::{Arg, Flag, Operator, Program, Quoted};
        assert_eq!(
            kinds("ls -la src"),
            [(Program, "ls"), (Flag, "-la"), (Arg, "src")]
        );
        assert_eq!(
            kinds("ls | head -n 3 && echo done>out"),
            [
                (Program, "ls"),
                (Operator, "|"),
                (Program, "head"),
                (Flag, "-n"),
                (Arg, "3"),
                (Operator, "&&"),
                (Program, "echo"),
                (Arg, "done"),
                (Operator, ">"),
                (Arg, "out"),
            ]
        );
        assert_eq!(
            kinds("echo \"a b\" 'c"),
            [(Program, "echo"), (Quoted, "\"a b\""), (Quoted, "'c")]
        );
    }
}
