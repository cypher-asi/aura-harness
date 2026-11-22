//! How Gate judges a prompt's work from the signals recorded about it.
//!
//! - [`turn_signal`]: the signal a turn's own messages give it when it
//!   ends; one that ran no check gets none, and its judge's verdict.
//! - [`calls_for_stronger`]: whether that signal sends the work on to a
//!   stronger model.
//! - [`is_retry_stronger`]: whether the next prompt is the same request
//!   sent again on a stronger model, which fails the earlier turn.

use aura_core_types::OutcomeSignal;
use aura_model::model_catalog::infer_tier;

/// Two prompts are the same request when at most one character in this
/// many differs (90% alike).
const CHARS_PER_EDIT: usize = 10;

/// Characters of each prompt compared; enough to tell a repeat, and it
/// keeps the comparison cheap on long pastes.
const MAX_COMPARED_CHARS: usize = 2_000;

/// The signal for a turn that ended: `errored` when it ended in an error,
/// else what its last check said (see [`crate::checks::last_check`]).
/// `None` when it ran no check: nothing it did says how it went, so the
/// judge has to.
#[must_use]
pub const fn turn_signal(errored: bool, last_check: Option<bool>) -> Option<OutcomeSignal> {
    match (errored, last_check) {
        (true, _) => Some(OutcomeSignal::Error),
        (false, Some(false)) => Some(OutcomeSignal::ChecksFailed),
        (false, Some(true)) => Some(OutcomeSignal::ChecksPassed),
        (false, None) => None,
    }
}

/// Whether work that ended with `signal` goes on to a stronger model:
/// it ended in an error, its last check failed, or the judge found it
/// wrong.
#[must_use]
pub const fn calls_for_stronger(signal: OutcomeSignal) -> bool {
    matches!(
        signal,
        OutcomeSignal::Error | OutcomeSignal::ChecksFailed | OutcomeSignal::JudgedFail
    )
}

/// Whether `next`, sent on `next_model`, repeats `previous`, which ran on
/// `previous_model`, after switching to a stronger tier.
#[must_use]
pub fn is_retry_stronger(
    previous: &str,
    previous_model: &str,
    next: &str,
    next_model: &str,
) -> bool {
    infer_tier(next_model) > infer_tier(previous_model) && is_repeat(previous, next)
}

/// Whether `next` asks the same as `previous`: equal, or at least 90%
/// alike by edit distance, ignoring case and spacing.
#[must_use]
pub fn is_repeat(previous: &str, next: &str) -> bool {
    let a = normalize(previous);
    let b = normalize(next);
    if a.is_empty() || b.is_empty() {
        return false;
    }
    if a == b {
        return true;
    }
    let longest = a.len().max(b.len());
    let alike = |distance: usize| distance.saturating_mul(CHARS_PER_EDIT) <= longest;
    alike(a.len().abs_diff(b.len())) && alike(edit_distance(&a, &b))
}

/// `text` lowercased with its whitespace collapsed, cut to
/// [`MAX_COMPARED_CHARS`].
fn normalize(text: &str) -> Vec<char> {
    text.split_whitespace()
        .flat_map(|word| std::iter::once(' ').chain(word.chars()))
        .skip(1)
        .flat_map(char::to_lowercase)
        .take(MAX_COMPARED_CHARS)
        .collect()
}

/// Levenshtein distance between `a` and `b`.
fn edit_distance(a: &[char], b: &[char]) -> usize {
    let mut row: Vec<usize> = (0..=b.len()).collect();
    for (i, ca) in a.iter().enumerate() {
        let mut diagonal = row[0];
        row[0] = i + 1;
        for (j, cb) in b.iter().enumerate() {
            let substitute = diagonal + usize::from(ca != cb);
            diagonal = row[j + 1];
            row[j + 1] = substitute.min(row[j] + 1).min(diagonal + 1);
        }
    }
    row[b.len()]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn turn_end_signals() {
        assert_eq!(turn_signal(true, Some(true)), Some(OutcomeSignal::Error));
        assert_eq!(
            turn_signal(false, Some(false)),
            Some(OutcomeSignal::ChecksFailed)
        );
        assert_eq!(
            turn_signal(false, Some(true)),
            Some(OutcomeSignal::ChecksPassed)
        );
        assert_eq!(turn_signal(false, None), None, "the judge has to say");
    }

    #[test]
    fn repeats_are_equal_or_nearly_so() {
        assert!(is_repeat("fix the parser bug", "fix the parser bug"));
        assert!(is_repeat("Fix the  parser bug\n", "fix the parser bug"));
        assert!(is_repeat(
            "make the login test pass again please",
            "make the login tests pass again please"
        ));
        assert!(!is_repeat("fix the parser bug", "explain the parser"));
        assert!(!is_repeat("fix it", "fix them"));
        assert!(!is_repeat("", ""));
    }

    #[test]
    fn edit_distance_counts_edits() {
        let chars = |s: &str| s.chars().collect::<Vec<_>>();
        assert_eq!(edit_distance(&chars("kitten"), &chars("sitting")), 3);
        assert_eq!(edit_distance(&chars(""), &chars("abc")), 3);
        assert_eq!(edit_distance(&chars("same"), &chars("same")), 0);
    }

    #[test]
    fn a_retry_needs_a_stronger_tier() {
        let prompt = "add a --json flag to the status command";
        assert!(is_retry_stronger(
            prompt,
            "claude/claude-haiku-4-5",
            prompt,
            "claude/claude-sonnet-4-6"
        ));
        assert!(is_retry_stronger(
            prompt,
            "local/qwen3:4b",
            prompt,
            "aura-claude-opus-4-7"
        ));
        assert!(!is_retry_stronger(
            prompt,
            "claude/claude-sonnet-4-6",
            prompt,
            "aura-claude-sonnet-4-6"
        ));
        assert!(!is_retry_stronger(
            prompt,
            "claude/claude-opus-4-7",
            prompt,
            "claude/claude-haiku-4-5"
        ));
        assert!(!is_retry_stronger(
            prompt,
            "claude/claude-haiku-4-5",
            "now write the docs for it",
            "claude/claude-opus-4-7"
        ));
    }
}
