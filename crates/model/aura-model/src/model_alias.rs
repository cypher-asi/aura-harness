//! Resolve a short model alias (`opus`, `deepseek`, `sonnet 4.5`,
//! `openrouter opus`) to one model id among a list of candidates.
//!
//! The resolver is pure: it reads only its arguments, so the same query
//! over the same candidates always names the same model.
//!
//! ## Matching
//!
//! 1. A candidate whose id equals the trimmed query, ignoring case, wins
//!    at once. Otherwise, among the candidates whose label equals it
//!    (labels such as `Claude Opus 4.7` are shared across routes), the
//!    best by the ranking below wins. This is the only way to name
//!    [`GATE_MODEL_ID`], which is not a model and must never be picked by
//!    a fuzzy query. [`resolve_exact`] applies only this step.
//! 2. A query containing `/` is an explicit model id: without an exact
//!    match it resolves to nothing, so callers keep using the raw id.
//! 3. The query is split into lowercase tokens (on whitespace, `-`, `_`,
//!    `.`, `:`). Tokens such as `qwen3` or `gpt5` stay whole. A candidate
//!    matches when the tokens appear in order among the segments of its
//!    id, or among the segments of its label:
//!    - a number matches only an equal segment, so `5` does not match
//!      `50`;
//!    - a number that follows another number in the query must match the
//!      segment right after the previous one, so `gpt 5.4` matches
//!      `gpt-5-4` and not `gpt-4-5`;
//!    - a number that follows a word in the query must match the first
//!      number segment after the word's segment (word segments may come
//!      between), so it names the version: `opus 4` matches
//!      `claude-opus-4-7` and `gpt 4` does not match `gpt-5-4`;
//!    - any other token matches an equal segment, or, from three
//!      characters on, a segment it starts (`deep` -> `deepseek`); `op`
//!      does not match `openrouter`, and `o3` matches only `o3`.
//! 4. Only when no candidate matched that way, the query and each
//!    candidate's [`model_key`] are compared without separators: a key
//!    containing a compact query of at least three characters matches
//!    (`gpt54` -> `gpt-5-4`).
//!
//! ## Ranking
//!
//! Among the matches the best one wins, comparing in this order:
//! available to the user now; served by a linked plan (Claude or
//! ChatGPT), so with a Claude Max plan linked `opus` names the plan's
//! Opus rather than OpenRouter's or aura-router's, even when another
//! route lists a newer version or the current model is on another
//! route; a release rather than a preview, experimental, distilled or
//! free variant; higher [`ModelTier`]; higher version (the numbers in its
//! key, a dated snapshot's date left out); served on the route of the
//! current model (so switching from Sonnet to Haiku on aura-router stays
//! there, even when OpenRouter lists the same model); shorter key, a
//! trailing date left out (the base model before its `-mini` or `-pro`
//! siblings); then the route order. For an
//! available model that order is Claude plan, ChatGPT plan, aura-router,
//! OpenRouter, local: subscriptions the user already pays for come
//! first. For an unavailable one aura-router comes first (then Claude
//! plan, ChatGPT plan, OpenRouter, local), since its one `/login`
//! unlocks most models. The earlier candidate in the list wins any
//! remaining tie, so the result is deterministic. Naming the route
//! (`openrouter opus`) or the full id still picks another route.
//!
//! An unavailable model is still returned when no available one
//! matches: the caller can then tell the user which sign-in it needs.

use std::cmp::Reverse;

use crate::model_catalog::{backend_for, model_key, ModelBackend, ModelTier, GATE_MODEL_ID};

/// One model the alias may resolve to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AliasCandidate<'a> {
    /// Full model id, as sent in a request.
    pub id: &'a str,
    /// Human-readable label.
    pub label: &'a str,
    /// How capable the model is.
    pub tier: ModelTier,
    /// Whether the user's sign-ins let them run it now.
    pub available: bool,
}

/// Id segments marking a model that is not the plain release of its
/// version.
const VARIANT_WORDS: &[&str] = &[
    "preview",
    "exp",
    "experimental",
    "beta",
    "alpha",
    "distill",
    "free",
];

/// Shortest compact query the separator-free fallback accepts; shorter
/// ones would match almost every key.
const MIN_COMPACT_QUERY_CHARS: usize = 3;

/// Shortest word token that may match a segment it starts; shorter ones
/// (`op`, `o3`) must equal a segment.
const MIN_PREFIX_CHARS: usize = 3;

/// The id `query` names among `candidates`, or `None` when it matches none.
/// `current` is the selected model id (used to prefer staying on its route).
///
/// See [the module docs](self) for the matching and ranking rules.
#[must_use]
pub fn resolve_alias<'a>(
    query: &str,
    candidates: &[AliasCandidate<'a>],
    current: &str,
) -> Option<&'a str> {
    let query = query.trim().to_lowercase();
    let current_route = route_of(current);
    if let Some(exact) = exact_match(&query, candidates, current_route) {
        return Some(exact);
    }
    if query.is_empty() || query.contains('/') {
        return None;
    }
    let tokens = words(&query);
    if tokens.is_empty() {
        return None;
    }

    let fuzzy = || {
        candidates
            .iter()
            .enumerate()
            .filter(|(_, c)| !c.id.eq_ignore_ascii_case(GATE_MODEL_ID))
    };
    best(
        fuzzy().filter(|(_, c)| names_match(&tokens, c)),
        current_route,
    )
    .or_else(|| {
        let compact_query = compact(&query);
        if compact_query.chars().count() < MIN_COMPACT_QUERY_CHARS {
            return None;
        }
        best(
            fuzzy().filter(|(_, c)| compact(&model_key(c.id)).contains(&compact_query)),
            current_route,
        )
    })
}

/// The id `query` names exactly among `candidates`: the candidate with
/// that id, else the best-ranked one with that label (both ignoring case
/// and surrounding whitespace). `None` when no id or label equals it.
/// `current` is the selected model id, as for [`resolve_alias`].
#[must_use]
pub fn resolve_exact<'a>(
    query: &str,
    candidates: &[AliasCandidate<'a>],
    current: &str,
) -> Option<&'a str> {
    exact_match(&query.trim().to_lowercase(), candidates, route_of(current))
}

/// [`resolve_exact`] for a trimmed, lowercase `query`.
fn exact_match<'a>(
    query: &str,
    candidates: &[AliasCandidate<'a>],
    current_route: Option<ModelBackend>,
) -> Option<&'a str> {
    if query.is_empty() {
        return None;
    }
    if let Some(by_id) = candidates.iter().find(|c| c.id.to_lowercase() == query) {
        return Some(by_id.id);
    }
    best(
        candidates
            .iter()
            .enumerate()
            .filter(|(_, c)| c.label.to_lowercase() == query),
        current_route,
    )
}

/// Route of the current model, or `None` when there is none to stay on
/// (nothing selected, or Gate, which picks a route per turn).
fn route_of(current: &str) -> Option<ModelBackend> {
    let current = current.trim();
    (!current.is_empty() && !current.eq_ignore_ascii_case(GATE_MODEL_ID))
        .then(|| backend_for(current))
}

/// The id of the highest-ranked candidate among `matches`, each paired
/// with its position in the candidate list.
fn best<'a, 'c>(
    matches: impl Iterator<Item = (usize, &'c AliasCandidate<'a>)>,
    current_route: Option<ModelBackend>,
) -> Option<&'a str>
where
    'a: 'c,
{
    matches
        .max_by_key(|(position, candidate)| Rank::of(candidate, *position, current_route))
        .map(|(_, candidate)| candidate.id)
}

/// How good a match a candidate is; the greater rank wins. Fields are
/// compared in declaration order.
#[derive(Debug, PartialEq, Eq, PartialOrd, Ord)]
struct Rank {
    available: bool,
    on_linked_plan: bool,
    is_release: bool,
    tier: ModelTier,
    version: Vec<u32>,
    on_current_route: bool,
    shorter_key: Reverse<usize>,
    route_preference: u8,
    earlier: Reverse<usize>,
}

impl Rank {
    fn of(candidate: &AliasCandidate<'_>, position: usize, current: Option<ModelBackend>) -> Self {
        let key = model_key(candidate.id);
        let backend = backend_for(candidate.id);
        Self {
            available: candidate.available,
            on_linked_plan: candidate.available
                && matches!(backend, ModelBackend::Claude | ModelBackend::Chatgpt),
            is_release: is_release(candidate.id),
            tier: candidate.tier,
            version: version(&key),
            on_current_route: current == Some(backend),
            shorter_key: Reverse(without_date(&key).len()),
            route_preference: route_preference(backend, candidate.available),
            earlier: Reverse(position),
        }
    }
}

/// Fixed route order for a model off the current route; greater is
/// preferred. An available model prefers the subscriptions the user
/// already pays for; an unavailable one prefers aura-router, whose one
/// `/login` unlocks most models, so the sign-in hint names that.
const fn route_preference(backend: ModelBackend, available: bool) -> u8 {
    match (backend, available) {
        (ModelBackend::Router, false) => 5,
        (ModelBackend::Claude, _) => 4,
        (ModelBackend::Chatgpt, _) => 3,
        (ModelBackend::Router, true) => 2,
        (ModelBackend::OpenRouter, _) => 1,
        (ModelBackend::Local, _) => 0,
    }
}

/// Whether `id` is a plain release rather than a preview, experimental,
/// distilled or free variant.
fn is_release(id: &str) -> bool {
    let id = id.to_lowercase();
    !words(&id).iter().any(|word| VARIANT_WORDS.contains(word))
}

/// Version numbers in `key` (a [`model_key`]), in order: plain numbers of
/// up to three digits, and the digits after a letter prefix (`v3`, `r1`,
/// `qwen3`). A number of four or more digits starts a dated snapshot and
/// ends the version. Size tags (`32b`) and words are skipped.
fn version(key: &str) -> Vec<u32> {
    let mut version = Vec::new();
    for segment in words(key) {
        if is_number(segment) {
            if segment.len() >= 4 {
                break;
            }
            version.extend(segment.parse::<u32>().ok());
        } else if let Some(number) = suffix_number(segment) {
            version.push(number);
        }
    }
    version
}

/// `key` without a trailing date snapshot: a last segment of four or
/// more digits (`-20251001`), or `YYYY-MM-DD` as its last three segments
/// (`-2026-03-05`). Any other key is returned whole.
fn without_date(key: &str) -> &str {
    let (rest, last) = split_last_segment(key);
    if is_number(last) && last.len() >= 4 {
        return rest;
    }
    let (rest, month) = split_last_segment(rest);
    let (rest, year) = split_last_segment(rest);
    let is_ymd = [(year, 4), (month, 2), (last, 2)]
        .iter()
        .all(|&(segment, len)| is_number(segment) && segment.len() == len);
    if is_ymd {
        rest
    } else {
        key
    }
}

/// `text` split at its last separator: what comes before (without the
/// separators there) and the last segment. Text without a separator is
/// all last segment.
fn split_last_segment(text: &str) -> (&str, &str) {
    text.rsplit_once(is_separator)
        .map_or(("", text), |(rest, last)| {
            (rest.trim_end_matches(is_separator), last)
        })
}

/// The number ending `segment` when it is letters followed by one to
/// three digits (`v3` -> 3, `qwen3` -> 3).
fn suffix_number(segment: &str) -> Option<u32> {
    let letters = segment.bytes().take_while(u8::is_ascii_alphabetic).count();
    let (_, digits) = segment.split_at_checked(letters)?;
    let is_number = letters > 0
        && (1..=3).contains(&digits.len())
        && digits.bytes().all(|b| b.is_ascii_digit());
    if is_number {
        digits.parse().ok()
    } else {
        None
    }
}

/// Whether `tokens` match the segments of the candidate's id or label.
fn names_match(tokens: &[&str], candidate: &AliasCandidate<'_>) -> bool {
    let id = candidate.id.to_lowercase();
    let label = candidate.label.to_lowercase();
    tokens_match(tokens, &words(&id), 0, Previous::First)
        || tokens_match(tokens, &words(&label), 0, Previous::First)
}

/// The query token before the one being matched.
#[derive(Debug, Clone, Copy)]
enum Previous {
    /// None: the token is the query's first.
    First,
    /// A word, matched just before the segments still to search.
    Word,
    /// A number, matched at this segment.
    Number(usize),
}

/// Whether `tokens` appear in order among `segments[start..]`, the token
/// before them being `previous`. A number after a number must match the
/// next segment; a number after a word must match the first number
/// segment left, so it is bound to the word's version.
///
/// Backtracks over every place a word or a leading number can match, so
/// an early partial match cannot hide a later full one (`4 5` in
/// `4-1-4-5`, `opus 4` in `opus-3-opus-4`).
fn tokens_match(tokens: &[&str], segments: &[&str], start: usize, previous: Previous) -> bool {
    let Some((&token, rest)) = tokens.split_first() else {
        return true;
    };
    if !is_number(token) {
        return segments
            .iter()
            .enumerate()
            .skip(start)
            .any(|(at, &segment)| {
                word_matches(token, segment) && tokens_match(rest, segments, at + 1, Previous::Word)
            });
    }
    match previous {
        Previous::First => {
            (start..segments.len()).any(|at| number_matches_at(token, rest, segments, at))
        }
        Previous::Number(at) => number_matches_at(token, rest, segments, at + 1),
        Previous::Word => segments
            .iter()
            .enumerate()
            .skip(start)
            .find(|(_, segment)| is_number(segment))
            .is_some_and(|(at, _)| number_matches_at(token, rest, segments, at)),
    }
}

/// Whether the number `token` equals `segments[at]` and the `rest` of the
/// tokens match after it.
fn number_matches_at(token: &str, rest: &[&str], segments: &[&str], at: usize) -> bool {
    segments.get(at).is_some_and(|&segment| segment == token)
        && tokens_match(rest, segments, at + 1, Previous::Number(at))
}

/// Whether the word `token` matches one segment: exactly or, from
/// [`MIN_PREFIX_CHARS`] characters on, as a prefix.
fn word_matches(token: &str, segment: &str) -> bool {
    segment == token || (token.chars().count() >= MIN_PREFIX_CHARS && segment.starts_with(token))
}

fn is_number(token: &str) -> bool {
    !token.is_empty() && token.bytes().all(|b| b.is_ascii_digit())
}

/// Whether `c` separates the words of a query, model id or label.
fn is_separator(c: char) -> bool {
    c.is_whitespace() || matches!(c, '/' | '-' | '_' | '.' | ':')
}

/// The non-empty words of `text`, split on whitespace and the separators
/// model ids and labels use.
fn words(text: &str) -> Vec<&str> {
    text.split(is_separator)
        .filter(|word| !word.is_empty())
        .collect()
}

/// `text` without separators: only its letters and digits.
fn compact(text: &str) -> String {
    text.chars().filter(|c| c.is_alphanumeric()).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn candidate(
        id: &'static str,
        label: &'static str,
        tier: ModelTier,
    ) -> AliasCandidate<'static> {
        AliasCandidate {
            id,
            label,
            tier,
            available: true,
        }
    }

    fn unavailable(mut candidate: AliasCandidate<'static>) -> AliasCandidate<'static> {
        candidate.available = false;
        candidate
    }

    fn catalog() -> Vec<AliasCandidate<'static>> {
        vec![
            candidate(
                "aura-claude-opus-4-7",
                "Claude Opus 4.7",
                ModelTier::Frontier,
            ),
            candidate(
                "aura-claude-sonnet-4-6",
                "Claude Sonnet 4.6",
                ModelTier::Standard,
            ),
            candidate("aura-claude-haiku-4-5", "Claude Haiku 4.5", ModelTier::Fast),
            candidate(
                "claude/claude-opus-4-7",
                "Claude Opus 4.7",
                ModelTier::Frontier,
            ),
            candidate(
                "claude/claude-sonnet-4-6",
                "Claude Sonnet 4.6",
                ModelTier::Standard,
            ),
            candidate(
                "openrouter/anthropic/claude-opus-4.7",
                "Claude Opus 4.7",
                ModelTier::Frontier,
            ),
            candidate(
                "openrouter/anthropic/claude-opus-4.1",
                "Claude Opus 4.1",
                ModelTier::Frontier,
            ),
            candidate(
                "openrouter/anthropic/claude-haiku-4.5",
                "Claude Haiku 4.5",
                ModelTier::Fast,
            ),
        ]
    }

    fn only_openrouter_available(
        candidates: Vec<AliasCandidate<'static>>,
    ) -> Vec<AliasCandidate<'static>> {
        candidates
            .into_iter()
            .map(|mut c| {
                c.available = c.id.starts_with("openrouter/");
                c
            })
            .collect()
    }

    #[test]
    fn opus_resolves_to_an_opus_4_7() {
        let resolved = resolve_alias("opus", &catalog(), "");
        assert!(
            resolved.is_some_and(|id| model_key(id) == "claude-opus-4-7"),
            "{resolved:?}"
        );
    }

    #[test]
    fn opus_stays_on_the_claude_plan_route_of_the_current_model() {
        assert_eq!(
            resolve_alias("opus", &catalog(), "claude/claude-sonnet-4-6"),
            Some("claude/claude-opus-4-7")
        );
    }

    #[test]
    fn opus_prefers_the_linked_claude_plan_over_the_current_router_route() {
        assert_eq!(
            resolve_alias("opus", &catalog(), "aura-claude-sonnet-4-6"),
            Some("claude/claude-opus-4-7")
        );
        assert_eq!(
            resolve_alias("opus", &catalog(), "openrouter/anthropic/claude-haiku-4.5"),
            Some("claude/claude-opus-4-7")
        );
    }

    #[test]
    fn opus_stays_on_the_router_route_of_the_current_model_without_a_plan() {
        let candidates: Vec<_> = catalog()
            .into_iter()
            .map(|c| {
                if c.id.starts_with("claude/") {
                    unavailable(c)
                } else {
                    c
                }
            })
            .collect();
        assert_eq!(
            resolve_alias("opus", &candidates, "aura-claude-sonnet-4-6"),
            Some("aura-claude-opus-4-7")
        );
    }

    #[test]
    fn linked_plan_beats_a_newer_version_on_openrouter() {
        let candidates = [
            candidate(
                "openrouter/anthropic/claude-opus-4.8",
                "Claude Opus 4.8",
                ModelTier::Frontier,
            ),
            candidate(
                "aura-claude-opus-4-8",
                "Claude Opus 4.8",
                ModelTier::Frontier,
            ),
            candidate(
                "claude/claude-opus-4-7",
                "Claude Opus 4.7",
                ModelTier::Frontier,
            ),
        ];
        assert_eq!(
            resolve_alias("opus", &candidates, GATE_MODEL_ID),
            Some("claude/claude-opus-4-7")
        );
        // Naming the route still reaches it.
        assert_eq!(
            resolve_alias("openrouter opus", &candidates, GATE_MODEL_ID),
            Some("openrouter/anthropic/claude-opus-4.8")
        );
    }

    #[test]
    fn opus_with_gate_current_and_only_openrouter_available_picks_openrouter_4_7() {
        let candidates = only_openrouter_available(catalog());
        assert_eq!(
            resolve_alias("opus", &candidates, GATE_MODEL_ID),
            Some("openrouter/anthropic/claude-opus-4.7")
        );
    }

    #[test]
    fn available_older_model_beats_unavailable_newer_one() {
        let candidates = [
            unavailable(candidate(
                "claude/claude-opus-4-7",
                "Claude Opus 4.7",
                ModelTier::Frontier,
            )),
            candidate(
                "aura-claude-opus-4-5",
                "Claude Opus 4.5",
                ModelTier::Frontier,
            ),
        ];
        assert_eq!(
            resolve_alias("opus", &candidates, "claude/claude-sonnet-4-6"),
            Some("aura-claude-opus-4-5")
        );
    }

    #[test]
    fn best_unavailable_match_prefers_the_router_route() {
        let candidates: Vec<_> = catalog().into_iter().map(unavailable).collect();
        assert_eq!(
            resolve_alias("opus", &candidates, ""),
            Some("aura-claude-opus-4-7")
        );
        assert_eq!(
            resolve_alias("haiku", &candidates, GATE_MODEL_ID),
            Some("aura-claude-haiku-4-5")
        );
    }

    #[test]
    fn dated_plan_id_stays_on_the_current_route() {
        let candidates = [
            candidate("aura-claude-haiku-4-5", "Claude Haiku 4.5", ModelTier::Fast),
            candidate(
                "claude/claude-haiku-4-5-20251001",
                "Claude Haiku 4.5",
                ModelTier::Fast,
            ),
        ];
        assert_eq!(
            resolve_alias("haiku", &candidates, "claude/claude-sonnet-4-6"),
            Some("claude/claude-haiku-4-5-20251001")
        );
        // Off any route, the date does not make the key longer either,
        // so the route order picks the plan.
        assert_eq!(
            resolve_alias("haiku", &candidates, ""),
            Some("claude/claude-haiku-4-5-20251001")
        );
        // The linked plan wins over the current route.
        assert_eq!(
            resolve_alias("haiku", &candidates, "aura-claude-sonnet-4-6"),
            Some("claude/claude-haiku-4-5-20251001")
        );
    }

    #[test]
    fn without_date_drops_only_a_trailing_snapshot() {
        let cases = [
            ("claude-haiku-4-5-20251001", "claude-haiku-4-5"),
            ("gpt-5-4-2026-03-05", "gpt-5-4"),
            ("claude-opus-4-1234", "claude-opus-4"),
            ("claude-opus-4-7", "claude-opus-4-7"),
            ("gpt-5-4-mini", "gpt-5-4-mini"),
            ("model-123-04-05", "model-123-04-05"),
            ("qwen3:4b", "qwen3:4b"),
        ];
        for (key, expected) in cases {
            assert_eq!(without_date(key), expected, "{key}");
        }
    }

    #[test]
    fn deepseek_prefers_latest_release_over_variants_and_older_versions() {
        let candidates = [
            candidate(
                "openrouter/deepseek/deepseek-chat-v3.1",
                "DeepSeek V3.1",
                ModelTier::Standard,
            ),
            candidate(
                "openrouter/deepseek/deepseek-v3.2",
                "DeepSeek V3.2",
                ModelTier::Standard,
            ),
            candidate(
                "openrouter/deepseek/deepseek-v3.2-exp",
                "DeepSeek V3.2 Exp",
                ModelTier::Standard,
            ),
            candidate(
                "openrouter/deepseek/deepseek-r1",
                "DeepSeek R1",
                ModelTier::Standard,
            ),
            candidate(
                "openrouter/deepseek/deepseek-r1-distill-qwen-32b",
                "DeepSeek R1 Distill Qwen 32B",
                ModelTier::Standard,
            ),
        ];
        for query in ["deepseek", "deep", "openrouter deepseek", "DeepSeek"] {
            assert_eq!(
                resolve_alias(query, &candidates, ""),
                Some("openrouter/deepseek/deepseek-v3.2"),
                "{query}"
            );
        }
    }

    fn gpt_catalog() -> Vec<AliasCandidate<'static>> {
        vec![
            candidate(
                "openrouter/openai/gpt-5.4-mini",
                "GPT-5.4 Mini",
                ModelTier::Fast,
            ),
            candidate("chatgpt/gpt-5.4", "GPT-5.4", ModelTier::Frontier),
            candidate("openrouter/openai/gpt-4.5", "GPT-4.5", ModelTier::Frontier),
        ]
    }

    #[test]
    fn gpt_prefers_frontier_over_fast_mini() {
        assert_eq!(
            resolve_alias("gpt", &gpt_catalog(), ""),
            Some("chatgpt/gpt-5.4")
        );
    }

    #[test]
    fn gpt_numbers_must_be_adjacent_and_in_order() {
        assert_eq!(
            resolve_alias("gpt 5.4", &gpt_catalog(), ""),
            Some("chatgpt/gpt-5.4")
        );
        assert_eq!(
            resolve_alias("gpt 4.5", &gpt_catalog(), ""),
            Some("openrouter/openai/gpt-4.5")
        );
        let older_only = [candidate(
            "openrouter/openai/gpt-4.5",
            "GPT-4.5",
            ModelTier::Frontier,
        )];
        assert_eq!(resolve_alias("gpt 5.4", &older_only, ""), None);
    }

    #[test]
    fn number_after_a_word_names_the_version() {
        assert_eq!(
            resolve_alias("gpt 4", &gpt_catalog(), ""),
            Some("openrouter/openai/gpt-4.5")
        );
        let newer_only = [candidate("chatgpt/gpt-5.4", "GPT-5.4", ModelTier::Frontier)];
        assert_eq!(resolve_alias("gpt 4", &newer_only, ""), None);

        let sonnet = [candidate(
            "claude/claude-sonnet-4-5",
            "Claude Sonnet 4.5",
            ModelTier::Standard,
        )];
        assert_eq!(
            resolve_alias("claude 4.5", &sonnet, ""),
            Some("claude/claude-sonnet-4-5")
        );
        let opus = [candidate(
            "claude/claude-opus-4-7",
            "Claude Opus 4.7",
            ModelTier::Frontier,
        )];
        assert_eq!(
            resolve_alias("opus 4", &opus, ""),
            Some("claude/claude-opus-4-7")
        );
    }

    #[test]
    fn a_later_placement_of_a_word_still_binds_its_number() {
        let candidates = [candidate(
            "aura-opus-3-opus-4",
            "Mixed",
            ModelTier::Frontier,
        )];
        assert_eq!(
            resolve_alias("opus 4", &candidates, ""),
            Some("aura-opus-3-opus-4")
        );
        assert_eq!(resolve_alias("opus 5", &candidates, ""), None);
    }

    #[test]
    fn prefix_matching_needs_three_characters() {
        assert_eq!(resolve_alias("op", &catalog(), ""), None);
        assert_eq!(
            resolve_alias("ope", &catalog(), ""),
            Some("openrouter/anthropic/claude-opus-4.7")
        );
        let candidates = [
            candidate("chatgpt/o4-mini", "OpenAI o4 Mini", ModelTier::Fast),
            candidate("openrouter/openai/o3", "OpenAI o3", ModelTier::Frontier),
        ];
        assert_eq!(
            resolve_alias("o3", &candidates, ""),
            Some("openrouter/openai/o3")
        );
        let longer_only = [candidate(
            "openrouter/openai/o3x",
            "OpenAI o3x",
            ModelTier::Frontier,
        )];
        assert_eq!(resolve_alias("o3", &longer_only, ""), None);
    }

    #[test]
    fn compact_query_falls_back_to_separator_free_key() {
        assert_eq!(
            resolve_alias("gpt5", &gpt_catalog(), ""),
            Some("chatgpt/gpt-5.4")
        );
        assert_eq!(
            resolve_alias("gpt54", &gpt_catalog(), ""),
            Some("chatgpt/gpt-5.4")
        );
        assert_eq!(
            resolve_alias("opus47", &catalog(), "claude/claude-sonnet-4-6"),
            Some("claude/claude-opus-4-7")
        );
    }

    #[test]
    fn sonnet_with_version_picks_that_version() {
        let candidates = [
            candidate(
                "claude/claude-sonnet-4-6",
                "Claude Sonnet 4.6",
                ModelTier::Standard,
            ),
            candidate(
                "claude/claude-sonnet-4-5-20250929",
                "Claude Sonnet 4.5",
                ModelTier::Standard,
            ),
        ];
        assert_eq!(
            resolve_alias("sonnet 4.5", &candidates, ""),
            Some("claude/claude-sonnet-4-5-20250929")
        );
        assert_eq!(
            resolve_alias("son", &candidates, ""),
            Some("claude/claude-sonnet-4-6")
        );
    }

    #[test]
    fn opus_with_version_matches_only_that_version() {
        let candidates = [
            candidate(
                "claude/claude-opus-4-7",
                "Claude Opus 4.7",
                ModelTier::Frontier,
            ),
            candidate(
                "claude/claude-opus-4-6",
                "Claude Opus 4.6",
                ModelTier::Frontier,
            ),
        ];
        assert_eq!(
            resolve_alias("opus 4.6", &candidates, ""),
            Some("claude/claude-opus-4-6")
        );
    }

    #[test]
    fn openrouter_opus_picks_the_openrouter_route() {
        assert_eq!(
            resolve_alias("openrouter opus", &catalog(), "claude/claude-sonnet-4-6"),
            Some("openrouter/anthropic/claude-opus-4.7")
        );
    }

    #[test]
    fn qwen_picks_highest_local_version() {
        let candidates = [
            candidate("local/qwen2.5:7b", "qwen2.5:7b", ModelTier::Local),
            candidate("local/qwen3:4b", "qwen3:4b", ModelTier::Local),
            candidate("local/llama3.2:3b", "llama3.2:3b", ModelTier::Local),
        ];
        assert_eq!(
            resolve_alias("qwen", &candidates, ""),
            Some("local/qwen3:4b")
        );
    }

    #[test]
    fn exact_id_matches_case_insensitively() {
        assert_eq!(
            resolve_alias("  OpenRouter/Anthropic/Claude-Opus-4.1 ", &catalog(), ""),
            Some("openrouter/anthropic/claude-opus-4.1")
        );
    }

    #[test]
    fn exact_label_picks_the_best_ranked_candidate_with_that_label() {
        assert_eq!(
            resolve_alias("claude opus 4.1", &catalog(), ""),
            Some("openrouter/anthropic/claude-opus-4.1")
        );
        assert_eq!(
            resolve_alias("CLAUDE OPUS 4.7", &catalog(), "claude/claude-sonnet-4-6"),
            Some("claude/claude-opus-4-7")
        );
        assert_eq!(
            resolve_alias("Claude Opus 4.7", &catalog(), "aura-claude-sonnet-4-6"),
            Some("claude/claude-opus-4-7")
        );
        let candidates = only_openrouter_available(catalog());
        assert_eq!(
            resolve_alias("Claude Opus 4.7", &candidates, "aura-claude-sonnet-4-6"),
            Some("openrouter/anthropic/claude-opus-4.7")
        );
    }

    #[test]
    fn resolve_exact_takes_only_exact_ids_and_labels() {
        assert_eq!(resolve_exact("opus", &catalog(), ""), None);
        assert_eq!(resolve_exact("claude/opus", &catalog(), ""), None);
        assert_eq!(
            resolve_exact(
                " Aura-Claude-Opus-4-7 ",
                &catalog(),
                "claude/claude-sonnet-4-6"
            ),
            Some("aura-claude-opus-4-7")
        );
        assert_eq!(
            resolve_exact("claude haiku 4.5", &catalog(), "aura-claude-sonnet-4-6"),
            Some("aura-claude-haiku-4-5")
        );
    }

    #[test]
    fn gate_resolves_only_by_exact_match() {
        let mut candidates = catalog();
        candidates.insert(0, candidate(GATE_MODEL_ID, "Gate", ModelTier::Frontier));
        assert_eq!(resolve_alias("Gate", &candidates, ""), Some(GATE_MODEL_ID));
        assert_eq!(resolve_alias("gat", &candidates, ""), None);
        let only_gate = [candidate(GATE_MODEL_ID, "Gate", ModelTier::Frontier)];
        assert_eq!(resolve_alias("ga", &only_gate, ""), None);
        assert_eq!(resolve_alias("gate model", &only_gate, ""), None);
    }

    #[test]
    fn query_with_slash_resolves_only_by_exact_id() {
        assert_eq!(resolve_alias("anthropic/opus", &catalog(), ""), None);
        assert_eq!(resolve_alias("claude/opus", &catalog(), ""), None);
    }

    #[test]
    fn unmatched_queries_resolve_to_none() {
        for query in ["", "   ", "zzzz", "-.-", "llama"] {
            assert_eq!(resolve_alias(query, &catalog(), ""), None, "{query:?}");
        }
    }

    #[test]
    fn number_does_not_match_a_longer_number() {
        let candidates = [candidate("aura-gpt-50", "GPT 50", ModelTier::Frontier)];
        assert_eq!(resolve_alias("5", &candidates, ""), None);

        let mut with_five = candidates.to_vec();
        with_five.push(candidate("chatgpt/gpt-5.4", "GPT-5.4", ModelTier::Frontier));
        assert_eq!(resolve_alias("5", &with_five, ""), Some("chatgpt/gpt-5.4"));
    }

    #[test]
    fn version_skips_size_tags_and_stops_at_dates() {
        let cases: &[(&str, &[u32])] = &[
            ("claude-opus-4-7", &[4, 7]),
            ("claude-sonnet-4-5-20250929", &[4, 5]),
            ("deepseek-chat-v3-1", &[3, 1]),
            ("deepseek-r1-distill-qwen-32b", &[1]),
            ("qwen3:4b", &[3]),
            ("qwen2-5:7b", &[2, 5]),
            ("gpt-4o", &[]),
            ("claude-opus", &[]),
        ];
        for (key, expected) in cases {
            assert_eq!(version(key), *expected, "{key}");
        }
    }
}
