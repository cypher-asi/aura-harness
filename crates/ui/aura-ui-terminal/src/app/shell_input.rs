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
            // A redirect (`>`, `<`, `2>&1`) is followed by a file or a
            // descriptor, not a command.
            let op = &rest[..len];
            expect_program = !op.contains(['<', '>']) && op.contains(['|', '&', ';']);
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

/// `script` as the lines it reads best in: its own lines, each also
/// broken after a top-level `;` (dropped), `&&` or `||`, which leaves it
/// the same script. One that the tokenizer can't follow (a heredoc, an
/// escaped quote) is only broken at its own lines.
pub(crate) fn statements(script: &str) -> Vec<String> {
    let script = script.trim();
    if script.contains("<<") || script.contains("\\\"") || script.contains("\\'") {
        return script
            .lines()
            .map(|line| line.trim_end().to_string())
            .collect();
    }
    let mut lines = Vec::new();
    let mut line = String::new();
    let mut depth = 0usize;
    let mut escaped = false;
    let mut line_start = true;
    for (kind, token) in tokens(script) {
        match kind {
            TokenKind::Space if line_start && !token.contains('\n') => continue,
            TokenKind::Space if token.contains('\n') => {
                lines.push(std::mem::take(&mut line));
                line.push_str(token.rsplit('\n').next().unwrap_or_default());
                line_start = false;
                continue;
            }
            TokenKind::Operator if depth == 0 && !escaped && matches!(token, ";" | "&&" | "||") => {
                if token != ";" {
                    line.push_str(token);
                }
                lines.push(std::mem::take(&mut line));
                line_start = true;
                continue;
            }
            TokenKind::Program | TokenKind::Flag | TokenKind::Arg => {
                depth =
                    (depth + token.matches('(').count()).saturating_sub(token.matches(')').count());
            }
            _ => {}
        }
        escaped = token.ends_with('\\');
        line_start = false;
        line.push_str(token);
    }
    lines.push(line);
    lines
        .into_iter()
        .map(|line| line.trim_end().to_string())
        .filter(|line| !line.trim().is_empty())
        .collect()
}

/// Byte length of the leading run of `text` matching `pred`.
fn run_len(text: &str, pred: impl Fn(char) -> bool) -> usize {
    text.find(|c| !pred(c)).unwrap_or(text.len())
}

#[cfg(test)]
mod tests {
    use super::{statements, tokens, TokenKind};

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
        assert_eq!(
            kinds("make 2>&1 | head"),
            [
                (Program, "make"),
                (Arg, "2"),
                (Operator, ">&"),
                (Arg, "1"),
                (Operator, "|"),
                (Program, "head"),
            ]
        );
    }

    #[test]
    fn statements_break_after_top_level_separators() {
        assert_eq!(
            statements(
                "cargo fmt --check && echo OK; cargo clippy 2>&1 | grep -E '^(a|b); c' | head; \
                 echo DONE || true"
            ),
            [
                "cargo fmt --check &&",
                "echo OK",
                "cargo clippy 2>&1 | grep -E '^(a|b); c' | head",
                "echo DONE ||",
                "true",
            ]
        );
    }

    #[test]
    fn statements_keep_nested_and_escaped_separators() {
        assert_eq!(
            statements("echo $(date; uptime) && find . -exec rm {} \\; ; ls"),
            ["echo $(date; uptime) &&", "find . -exec rm {} \\;", "ls"]
        );
        assert_eq!(
            statements("for ((i=0; i<3; i++)); do echo $i; done"),
            ["for ((i=0; i<3; i++))", "do echo $i", "done"]
        );
        assert_eq!(
            statements("case $x in a) foo;; esac"),
            ["case $x in a) foo;; esac"]
        );
    }

    #[test]
    fn statements_keep_their_own_lines_and_indentation() {
        assert_eq!(
            statements("if true; then\n  echo hi\nfi\n"),
            ["if true", "then", "  echo hi", "fi"]
        );
        assert_eq!(
            statements("cat <<EOF\na; b\nEOF"),
            ["cat <<EOF", "a; b", "EOF"]
        );
    }
}
