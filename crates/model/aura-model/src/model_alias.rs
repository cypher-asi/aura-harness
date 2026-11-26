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
mod tests;
