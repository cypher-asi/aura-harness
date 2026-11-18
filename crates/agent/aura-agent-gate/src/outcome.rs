//! How Gate judges a prompt's work from the signals recorded about it.
//!
//! - [`turn_signal`]: the signal written when a turn ends.
//! - [`calls_for_stronger`]: whether that signal sends the work on to a
//!   stronger model.
//! - [`is_retry_stronger`]: whether the next prompt is the same request
//!   sent again on a stronger model, which fails the earlier turn.
//! - [`effective`]: which of a prompt's outcomes counts.

use aura_core_types::{OutcomePayload, OutcomeSignal};
use aura_model::model_catalog::infer_tier;

/// Two prompts are the same request when at most one character in this
/// many differs (90% alike).
const CHARS_PER_EDIT: usize = 10;

/// Characters of each prompt compared; enough to tell a repeat, and it
/// keeps the comparison cheap on long pastes.
const MAX_COMPARED_CHARS: usize = 2_000;

/// The signal for a turn that ended: `errored` when it ended in an error,
/// else what its last check said (see [`crate::checks::last_check`]).
#[must_use]
pub const fn turn_signal(errored: bool, last_check: Option<bool>) -> OutcomeSignal {
    match (errored, last_check) {
        (true, _) => OutcomeSignal::Error,
        (false, Some(false)) => OutcomeSignal::ChecksFailed,
        (false, Some(true)) => OutcomeSignal::ChecksPassed,
        (false, None) => OutcomeSignal::None,
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

/// The outcome that counts among those recorded for one prompt, oldest
/// first: the user's latest rating if there is one, else the latest.
pub fn effective<'a>(
    outcomes: impl IntoIterator<Item = &'a OutcomePayload>,
) -> Option<&'a OutcomePayload> {
    let mut latest = None;
    let mut rating = None;
    for outcome in outcomes {
        if outcome.signal.is_rating() {
            rating = Some(outcome);
        }
        latest = Some(outcome);
    }
    rating.or(latest)
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
    use aura_core_types::{Hash, PromptCategory};

    fn outcome(signal: OutcomeSignal) -> OutcomePayload {
        OutcomePayload {
            prompt_tx: Hash::from_content(b"prompt"),
            category: PromptCategory::Debugging,
            model: "claude/claude-sonnet-4-6".to_string(),
            success: signal.success(),
            signal,
            cost_usd: 0.1,
            latency_ms: 1_000,
        }
    }

    #[test]
    fn turn_end_signals() {
        assert_eq!(turn_signal(true, Some(true)), OutcomeSignal::Error);
        assert_eq!(turn_signal(false, Some(false)), OutcomeSignal::ChecksFailed);
        assert_eq!(turn_signal(false, Some(true)), OutcomeSignal::ChecksPassed);
        assert_eq!(turn_signal(false, None), OutcomeSignal::None);
    }

    #[test]
    fn the_latest_outcome_wins() {
        let recorded = [
            outcome(OutcomeSignal::ChecksPassed),
            outcome(OutcomeSignal::Rewind),
        ];
        assert_eq!(
            effective(&recorded).map(|o| o.signal),
            Some(OutcomeSignal::Rewind)
        );
        assert_eq!(effective(&[]), None);
    }

    #[test]
    fn a_rating_overrides_automatic_signals_before_and_after_it() {
        let recorded = [
            outcome(OutcomeSignal::ChecksFailed),
            outcome(OutcomeSignal::Good),
            outcome(OutcomeSignal::RetryStronger),
        ];
        let counted = effective(&recorded).unwrap();
        assert_eq!(counted.signal, OutcomeSignal::Good);
        assert_eq!(counted.success, Some(true));

        let changed_mind = [outcome(OutcomeSignal::Good), outcome(OutcomeSignal::Bad)];
        assert_eq!(
            effective(&changed_mind).map(|o| o.signal),
            Some(OutcomeSignal::Bad)
        );
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
