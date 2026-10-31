//! Decide whether submitted input is a shell command or a chat message.

/// Prefix that forces the rest of the line to run as a command.
const FORCE_PREFIX: char = '!';

/// Characters that join or redirect commands.
const OPERATORS: &[char] = &['|', '&', ';', '<', '>'];

/// Return the command line to run when `text` is a shell command.
///
/// `!cmd` always runs `cmd`. Unprefixed input runs when its first word
/// is an executable on `PATH` and the line does not read like prose.
pub(super) fn parse(text: &str) -> Option<String> {
    parse_with(text, on_path)
}

pub(super) fn on_path(program: &str) -> bool {
    which::which(program).is_ok()
}

/// What a piece of a command line is, for highlighting.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum TokenKind {
    /// Whitespace between tokens
    Space,
    /// The `!` that forces a command
    ForcePrefix,
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
        } else if c == FORCE_PREFIX && tokens.iter().all(|(k, _)| *k == TokenKind::Space) {
            (TokenKind::ForcePrefix, 1)
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

pub(super) fn parse_with(text: &str, resolves: impl FnOnce(&str) -> bool) -> Option<String> {
    let text = text.trim();
    if let Some(forced) = text.strip_prefix(FORCE_PREFIX) {
        let forced = forced.trim();
        return (!forced.is_empty()).then(|| forced.to_string());
    }
    let program = text.split_whitespace().next()?;
    if looks_like_prose(text) || !resolves(program) {
        return None;
    }
    Some(text.to_string())
}

/// A question, or a line of plain words and numbers (no flags, paths,
/// quotes or operators) with at least three words, is treated as a
/// message to the agent.
fn looks_like_prose(text: &str) -> bool {
    if text.ends_with('?') {
        return true;
    }
    let words: Vec<&str> = text
        .split_whitespace()
        .map(|word| word.trim_end_matches(['.', ',', '!']))
        .collect();
    words.iter().all(|word| is_word(word) || is_number(word))
        && words.iter().filter(|word| is_word(word)).count() >= 3
}

fn is_word(word: &str) -> bool {
    !word.is_empty() && word.chars().all(|c| c.is_alphabetic() || c == '\'')
}

fn is_number(word: &str) -> bool {
    !word.is_empty() && word.chars().all(|c| c.is_ascii_digit())
}

#[cfg(test)]
mod tests {
    use super::{parse_with, tokens, TokenKind};

    fn on_path(program: &str) -> bool {
        matches!(
            program,
            "ls" | "git" | "make" | "echo" | "cargo" | "write" | "seq" | "kill"
        )
    }

    fn parse(text: &str) -> Option<String> {
        parse_with(text, on_path)
    }

    #[test]
    fn plain_commands_run() {
        assert_eq!(parse("ls").as_deref(), Some("ls"));
        assert_eq!(parse("ls -la").as_deref(), Some("ls -la"));
        assert_eq!(parse("git status").as_deref(), Some("git status"));
        assert_eq!(parse("ls | head -n 3").as_deref(), Some("ls | head -n 3"));
        assert_eq!(
            parse("cargo test -p aura-terminal").as_deref(),
            Some("cargo test -p aura-terminal")
        );
    }

    #[test]
    fn prefix_forces_command() {
        assert_eq!(
            parse("!echo hi there friend").as_deref(),
            Some("echo hi there friend")
        );
        assert_eq!(
            parse("! unknown-tool --x").as_deref(),
            Some("unknown-tool --x")
        );
        assert_eq!(parse("!"), None);
        assert_eq!(parse("!   "), None);
    }

    #[test]
    fn prose_goes_to_the_agent() {
        assert_eq!(parse("make it faster"), None);
        assert_eq!(parse("ls what is here?"), None);
        assert_eq!(parse("what is this?"), None);
        assert_eq!(parse("hello"), None);
        assert_eq!(parse(""), None);
        assert_eq!(parse("write a 5 paragrah story"), None);
        assert_eq!(parse("make 3 small examples."), None);
    }

    #[test]
    fn numeric_arguments_still_run() {
        assert_eq!(parse("seq 1 10").as_deref(), Some("seq 1 10"));
        assert_eq!(parse("kill 1234 5678").as_deref(), Some("kill 1234 5678"));
        assert_eq!(parse("echo 1 2").as_deref(), Some("echo 1 2"));
    }

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
        use TokenKind::{Arg, Flag, ForcePrefix, Operator, Program, Quoted};
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
            kinds("!echo \"a b\" 'c"),
            [
                (ForcePrefix, "!"),
                (Program, "echo"),
                (Quoted, "\"a b\""),
                (Quoted, "'c"),
            ]
        );
    }
}
