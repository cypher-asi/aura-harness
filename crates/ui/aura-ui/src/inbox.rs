//! How a message one agent sends another through its inbox reads, to the
//! model and in the chat: a first line saying who sent it and what about,
//! a second naming it, then its text. A chat folds it to the first line.

/// What an inbox message is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InboxKind {
    /// Work a channel's coordinator hands out
    Assignment,
    /// A message that asks for an answer
    Request,
    /// The answer to an assignment or a request
    Reply,
}

impl InboxKind {
    const ALL: [Self; 3] = [Self::Assignment, Self::Request, Self::Reply];

    const fn word(self) -> &'static str {
        match self {
            Self::Assignment => "assignment",
            Self::Request => "message",
            Self::Reply => "reply",
        }
    }
}

/// What the line naming a message starts with.
const NAME_OPEN: &str = "(inbox ";

/// The first two lines of message `id` of `kind` from the agent with code
/// `from_code` (`from_name` in full), about `subject`:
///
/// ```text
/// PRC1: Fix the tokenizer
/// (inbox assignment 3f9a2c1b from PRC1, coordinator)
/// ```
///
/// A reply's first line says `REN4 replied: …`. Line breaks in the
/// subject and the names are flattened, so the header stays two lines,
/// and round brackets in the sender's name become square ones, so the
/// naming line's own close its end.
#[must_use]
pub fn header(
    kind: InboxKind,
    id: &str,
    from_code: &str,
    from_name: &str,
    subject: &str,
) -> String {
    let from_code = flat(from_code);
    let said = match kind {
        InboxKind::Reply => format!("{from_code} replied"),
        InboxKind::Assignment | InboxKind::Request => from_code.clone(),
    };
    format!(
        "{said}: {}\n{}from {from_code}, {})",
        flat(subject),
        marker(kind, id),
        flat(from_name).replace('(', "[").replace(')', "]")
    )
}

/// What the line naming message `id` of `kind` starts with, to find the
/// message in a conversation.
#[must_use]
pub fn marker(kind: InboxKind, id: &str) -> String {
    format!("{NAME_OPEN}{} {id} ", kind.word())
}

/// An inbox message as a chat folds it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Folded<'a> {
    pub kind: InboxKind,
    /// The line it folds to: who sent it and what about
    pub summary: &'a str,
}

/// `text` folded, when it is an inbox message, perhaps after one line
/// that labels it (such as the harness's own label for a message sent
/// into a running turn). `None` for any other text.
#[must_use]
pub fn fold(text: &str) -> Option<Folded<'_>> {
    let mut lines = text.lines();
    let mut summary = lines.next()?;
    for name in lines.take(2) {
        if let Some(kind) = kind_named(name) {
            return Some(Folded { kind, summary });
        }
        summary = name;
    }
    None
}

/// The kind of message the naming line `line` names, if it is one.
fn kind_named(line: &str) -> Option<InboxKind> {
    let rest = line.strip_prefix(NAME_OPEN)?;
    if !line.ends_with(')') {
        return None;
    }
    InboxKind::ALL.into_iter().find(|kind| {
        rest.strip_prefix(kind.word())
            .is_some_and(|rest| rest.starts_with(' '))
    })
}

/// `text` on one line, its whitespace collapsed.
fn flat(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_message_folds_to_its_first_line() {
        let text = format!(
            "{}\n\nCheck the tokenizer.",
            header(
                InboxKind::Assignment,
                "3f9a",
                "PRC1",
                "#site coordinator",
                "Fix the\ntokenizer"
            )
        );
        assert!(text.starts_with("PRC1: Fix the tokenizer\n(inbox assignment 3f9a from PRC1"));
        assert!(text.contains(&marker(InboxKind::Assignment, "3f9a")));
        assert_eq!(
            fold(&text),
            Some(Folded {
                kind: InboxKind::Assignment,
                summary: "PRC1: Fix the tokenizer"
            })
        );

        let labelled = format!("Harness notice:\n{text}");
        assert_eq!(
            fold(&labelled).map(|f| f.summary),
            Some("PRC1: Fix the tokenizer")
        );
    }

    #[test]
    fn a_reply_says_so() {
        let text = header(
            InboxKind::Reply,
            "9",
            "REN4",
            "rust-engineer #4 (x)",
            "Parser",
        );
        assert_eq!(
            fold(&text),
            Some(Folded {
                kind: InboxKind::Reply,
                summary: "REN4 replied: Parser"
            })
        );
        assert!(text.ends_with("rust-engineer #4 [x])"));
    }

    #[test]
    fn other_text_does_not_fold() {
        assert_eq!(fold("just a message"), None);
        assert_eq!(fold("a\n(inbox nothing 1 from X, y)"), None);
        assert_eq!(fold("a\n(inbox message 1 from X, y"), None);
        assert_eq!(fold("a\nb\nc\n(inbox message 1 from X, y)"), None);
    }
}
