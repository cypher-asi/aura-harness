//! The subcommands: `list`, `show`, `go`, `cancel`, `dismiss`, `apply`
//! and `diff`.

use super::words::Word;
use super::{CompareCommand, USAGE};

/// The subcommand `words` are, when they are exactly one; `None` reads
/// them as a new comparison.
pub(super) fn subcommand(words: &[Word]) -> Option<Result<CompareCommand, String>> {
    let first = words.first().map(|w| w.text.to_ascii_lowercase());
    let second = words.get(1).map(|w| w.text.as_str());
    let command = match (first.as_deref(), words.len()) {
        (None, _) | (Some("list"), 1) => CompareCommand::List,
        (Some("show"), 1) => CompareCommand::Show(None),
        (Some("show"), 2) => {
            let n = second?.parse::<usize>().ok().filter(|n| *n > 0)?;
            CompareCommand::Show(Some(n))
        }
        (Some("go" | "start" | "yes" | "run"), 1) => CompareCommand::Go,
        (Some("cancel" | "stop"), 1) => CompareCommand::Cancel,
        (Some("dismiss"), 1) => CompareCommand::Dismiss,
        (Some("apply"), 2) => CompareCommand::Apply(second?.to_ascii_lowercase()),
        (Some("diff"), 1) => CompareCommand::Diff(None),
        (Some("diff"), 2) => CompareCommand::Diff(Some(second?.to_ascii_lowercase())),
        (Some("apply"), 1) => {
            return Some(Err(format!(
                "/compare {} needs a name. {USAGE}",
                words[0].text
            )));
        }
        _ => return None,
    };
    Some(Ok(command))
}
