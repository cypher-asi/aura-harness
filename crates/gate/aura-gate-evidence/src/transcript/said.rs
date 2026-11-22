//! Gate's own messages in a turn: how the ones that ask the model to
//! check its change start, to tell them apart from the user's prompts.

/// How the message starts that asks a model to check a change it ran no
/// test or build on, in every version of it.
pub const NUDGE_OPENING: &str =
    "You changed files but did not run any test or build to check the change.";

/// How the message starts that asks a model to test what it added that
/// no test names.
pub const UNCOVERED_OPENING: &str = "The tests you ran passed, but no test names ";

/// Whether `text`, a message to the model, is one of Gate's asking it to
/// check its change, in this version's words or an earlier one's.
#[must_use]
pub fn is_nudge(text: &str) -> bool {
    text.contains(NUDGE_OPENING) || text.contains(UNCOVERED_OPENING)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_nudge_from_an_earlier_version_still_reads() {
        let earlier = "[harness] You changed files but did not run any test or build to check \
                       the change. Before you finish, run the tests that cover what you changed \
                       (...). If no test covers it, run the build or type check. Then reply \
                       with what you ran and how it went.";
        assert!(is_nudge(earlier));
        assert!(!is_nudge("Fix the parser."));
    }
}
