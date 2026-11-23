//! Code names of registered agents: short, unique and stable, such as
//! `RE36` for the permanent-agent instance `rust-engineer #36`.
//!
//! A code is `<PREFIX><N>` with no separator: `PREFIX` is two or three
//! uppercase ASCII letters and `N` a positive integer without leading
//! zeros, so the prefix always ends at the first digit.
//!
//! Invariants, over every agent in the registry, archived and removed
//! ones included:
//! - no two agents have the same code;
//! - every agent with the same source key (its definition, or
//!   [`GENERIC_SOURCE`] for agents without one) has the same prefix, and
//!   no two source keys share a prefix.
//!
//! A code is given once, when the agent is registered or, for registries
//! written before codes existed, when the registry is loaded; it is never
//! changed after that, except by [`migrate`]. The same registry always
//! backfills to the same codes, since agents are coded in registry order
//! from what came before.
//!
//! Each source key's prefix is kept in a [`PrefixTable`], so a definition
//! can have its abbreviation before its first instance exists. Once a key
//! is in the table its prefix never changes, and no two keys in it share
//! a prefix.
//!
//! # Numbers
//!
//! A number is never handed out twice for a prefix, even after the agent
//! that had it is dropped from the registry (`forget`, `remove_project`).
//! [`LastNumbers`] keeps, per prefix, the highest number ever given; it is
//! saved with the registry, raised on every assignment and seeded from the
//! codes a registry has when it is loaded, and never lowered. A new agent
//! is numbered above it; see [`assign`] and [`backfill`].
//!
//! # Prefixes
//!
//! A key reserves the first of its [`candidates`] that no other key holds.
//! The key is split into words on anything that is not an ASCII letter or
//! digit; digits are then dropped, and words left without letters are
//! skipped. A key with no letters at all is treated as [`GENERIC_SOURCE`].
//! The candidates, in order:
//!
//! 1. Two letters, the only two-letter candidate: the initials of the
//!    first two words, or the first two letters of a one-word key (the
//!    letter twice for a one-letter key).
//! 2. Three letters, so only when that one is taken. With two or more
//!    words: the initials of the first three words (with three or more),
//!    the first word's first two letters and the second word's initial,
//!    the first word's initial and the second word's first two letters,
//!    then the first three letters of the key. With one word: its first
//!    three letters, then its first two letters followed by each later
//!    letter in turn.
//! 3. The fallback: the first candidate's initial followed by `AA` to
//!    `ZZ`, then every three letters from `AAA` to `ZZZ`.
//!
//! Reserved in this order, `rust-engineer` gets `RE`, `reviewer` `REV`
//! (`RE` is taken), `research-engineer` `REE`, `explorer` `EX`,
//! `frontend-engineer` `FE` and `agent` `AG`.
//!
//! # Migration
//!
//! Older registries wrote `<PREFIX>-<N>` codes with prefixes of any length
//! and digits in them. On load, [`migrate`] joins up `RE-36` as `RE36`
//! when its prefix is two or three letters, keeping the number and the
//! key's claim on the prefix, and drops every other code, which the
//! backfill then replaces. Reserved prefixes that are not two or three
//! letters are dropped the same way.

use super::RegisteredAgent;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet, HashSet};

/// Source key of the agents that are no permanent agent's instance, so
/// `Agent`, `Agent 2`, … are `AG1`, `AG2`, ….
const GENERIC_SOURCE: &str = "agent";

/// What `agent`'s prefix is derived from: its definition, else
/// [`GENERIC_SOURCE`].
fn source_key(agent: &RegisteredAgent) -> &str {
    agent.definition.as_deref().unwrap_or(GENERIC_SOURCE)
}

/// Whether `prefix` is two or three uppercase ASCII letters.
fn is_prefix(prefix: &str) -> bool {
    (2..=3).contains(&prefix.len()) && prefix.bytes().all(|b| b.is_ascii_uppercase())
}

/// `code` split into its prefix and number, or `None` when it is not
/// `<PREFIX><N>` (a hand-edited or older registry could hold anything).
fn parse(code: &str) -> Option<(&str, u64)> {
    let digits = code.find(|c: char| c.is_ascii_digit())?;
    parse_parts(code.get(..digits)?, code.get(digits..)?)
}

/// `prefix` and `number` as a code's parts, when `prefix` is valid and
/// `number` is a positive integer written without leading zeros.
fn parse_parts<'a>(prefix: &'a str, number: &str) -> Option<(&'a str, u64)> {
    let canonical = !number.starts_with('0') && number.bytes().all(|b| b.is_ascii_digit());
    let number: u64 = number.parse().ok().filter(|_| canonical)?;
    (is_prefix(prefix) && number > 0).then_some((prefix, number))
}

/// `code` in the current format: as it is when it is `<PREFIX><N>`,
/// joined up when it is an older `<PREFIX>-<N>` with a valid prefix, else
/// `None`.
fn current_form(code: &str) -> Option<String> {
    if parse(code).is_some() {
        return Some(code.to_string());
    }
    let (prefix, number) = code.split_once('-')?;
    let (prefix, number) = parse_parts(prefix, number)?;
    Some(format!("{prefix}{number}"))
}

/// The `n` of an instance named `<definition> #<n>`.
fn instance_number(agent: &RegisteredAgent) -> Option<u64> {
    number_in_name(&agent.name, agent.definition.as_deref()?)
}

/// The `n` of `name` when it is `<definition> #<n>`, `n` positive.
pub(super) fn number_in_name(name: &str, definition: &str) -> Option<u64> {
    let number: u64 = name
        .strip_prefix(definition)?
        .strip_prefix(" #")?
        .parse()
        .ok()?;
    (number > 0).then_some(number)
}

/// The words of `key` as uppercase letters: split on anything that is not
/// an ASCII letter or digit, digits dropped, words left empty skipped.
/// [`GENERIC_SOURCE`]'s words when `key` has no letters, so neither the
/// list nor any word in it is empty.
fn words(key: &str) -> Vec<Vec<char>> {
    let split = |key: &str| -> Vec<Vec<char>> {
        key.split(|c: char| !c.is_ascii_alphanumeric())
            .map(|word| {
                word.chars()
                    .filter(char::is_ascii_alphabetic)
                    .map(|c| c.to_ascii_uppercase())
                    .collect::<Vec<char>>()
            })
            .filter(|word| !word.is_empty())
            .collect()
    };
    let words = split(key);
    if words.is_empty() {
        split(GENERIC_SOURCE)
    } else {
        words
    }
}

/// The first `n` letters of `word`, or `None` when it is shorter.
fn start(word: &[char], n: usize) -> Option<String> {
    word.get(..n).map(|letters| letters.iter().collect())
}

/// `head` then `tail`, when both exist.
fn join(head: Option<String>, tail: Option<String>) -> Option<String> {
    Some(head? + &tail?)
}

/// The prefixes made from `key`'s own letters, best first and without
/// repeats: the two-letter candidate, then the three-letter ones (steps 1
/// and 2 of the module docs). The first is always two letters.
fn derived(key: &str) -> Vec<String> {
    let words = words(key);
    let letters = words.concat();
    let first = words.first().map_or(&[][..], Vec::as_slice);
    let mut out: Vec<String> = Vec::new();
    let mut add = |prefix: Option<String>| {
        if let Some(prefix) = prefix.filter(|prefix| !out.contains(prefix)) {
            out.push(prefix);
        }
    };
    match words.get(1) {
        Some(second) => {
            add(join(start(first, 1), start(second, 1)));
            if words.len() >= 3 {
                add(words.iter().take(3).map(|word| start(word, 1)).collect());
            }
            add(join(start(first, 2), start(second, 1)));
            add(join(start(first, 1), start(second, 2)));
            add(start(&letters, 3));
        }
        None => {
            add(start(first, 2).or_else(|| start(first, 1).map(|letter| letter.repeat(2))));
            add(start(first, 3));
            if let Some(head) = start(first, 2) {
                for later in first.iter().skip(2) {
                    add(Some(format!("{head}{later}")));
                }
            }
        }
    }
    out
}

/// The fallback prefixes for a key whose candidates start with `initial`:
/// `initial` followed by `AA` to `ZZ`, then `AAA` to `ZZZ`.
fn fallback(initial: char) -> impl Iterator<Item = String> {
    let alphabet = || 'A'..='Z';
    let after_initial =
        alphabet().flat_map(move |b| alphabet().map(move |c| format!("{initial}{b}{c}")));
    let any = alphabet().flat_map(move |a| {
        alphabet().flat_map(move |b| alphabet().map(move |c| format!("{a}{b}{c}")))
    });
    after_initial.chain(any)
}

/// Every prefix `key` may take, best first: its [`derived`] prefixes,
/// then the [`fallback`]. Finite and never empty; the fallback may repeat
/// a derived prefix, which is harmless when looking for the first free
/// one.
fn candidates(key: &str) -> impl Iterator<Item = String> {
    let derived = derived(key);
    let initial = derived
        .first()
        .and_then(|prefix| prefix.chars().next())
        .unwrap_or('A');
    derived.into_iter().chain(fallback(initial))
}

/// Every coded agent of `agents` as its source key and code prefix, in
/// registry order. Agents whose code is not `<PREFIX><N>` are left out.
fn coded(agents: &[RegisteredAgent]) -> impl Iterator<Item = (&str, &str)> {
    agents.iter().filter_map(|agent| {
        let (prefix, _) = parse(agent.code.as_deref()?)?;
        Some((source_key(agent), prefix))
    })
}

/// The prefix reserved by each source key (definition name, or
/// [`GENERIC_SOURCE`]), persisted with the registry.
///
/// Invariants: every prefix is two or three uppercase letters, an entry,
/// once made, is never changed or removed (except by [`migrate`], for
/// entries older registries wrote), and no two keys hold the same prefix.
#[derive(Debug, Default, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub(super) struct PrefixTable(BTreeMap<String, String>);

impl PrefixTable {
    /// Whether no key has reserved a prefix yet.
    pub(super) fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    /// The prefix `key` has reserved, without reserving one.
    pub(super) fn get(&self, key: &str) -> Option<&str> {
        self.0.get(key).map(String::as_str)
    }

    /// How many keys have reserved a prefix.
    pub(super) fn len(&self) -> usize {
        self.0.len()
    }

    /// Whether a key other than `key` holds `prefix`.
    fn held_by_other(&self, key: &str, prefix: &str) -> bool {
        self.0
            .iter()
            .any(|(holder, held)| holder != key && held == prefix)
    }

    /// Drop the entries whose prefix is not two or three uppercase
    /// letters, as older registries allowed, and, in key order, any entry
    /// whose prefix an earlier key holds (only a hand edit makes one).
    /// Returns whether any entry was dropped.
    fn retain_valid(&mut self) -> bool {
        let before = self.len();
        let mut held = BTreeSet::new();
        self.0
            .retain(|_, prefix| is_prefix(prefix) && held.insert(prefix.clone()));
        self.len() != before
    }

    /// Reserve for every key of `agents` not in the table yet the prefix
    /// of its first code in registry order, unless another key holds that
    /// prefix already. This rebuilds the table of a registry written
    /// before it existed, giving every key the prefix its codes have.
    /// Returns whether any key was added.
    pub(super) fn fill_from_codes(&mut self, agents: &[RegisteredAgent]) -> bool {
        let before = self.len();
        for (key, prefix) in coded(agents) {
            if !self.0.contains_key(key) && !self.held_by_other(key, prefix) {
                self.0.insert(key.to_string(), prefix.to_string());
            }
        }
        self.len() != before
    }

    /// The prefix of the agents with source key `key`, reserving it when
    /// `key` has none yet: the first of its [`candidates`] that no other
    /// key holds and no agent of another key in `agents` uses.
    pub(super) fn prefix_for(&mut self, key: &str, agents: &[RegisteredAgent]) -> String {
        if let Some(prefix) = self.0.get(key) {
            return prefix.clone();
        }
        let taken = |prefix: &str| {
            self.held_by_other(key, prefix)
                || coded(agents).any(|(source, used)| source != key && used == prefix)
        };
        // Some candidate is free unless other keys hold all 17,576
        // three-letter prefixes; only then is the best one shared.
        let chosen = candidates(key)
            .find(|prefix| !taken(prefix))
            .or_else(|| candidates(key).next())
            .unwrap_or_default();
        self.0.insert(key.to_string(), chosen.clone());
        chosen
    }
}

/// Bring the codes of `agents` and the entries of `prefixes`, as an older
/// or hand-edited registry has them, to the current format and
/// invariants. Returns whether anything changed.
///
/// Reserved prefixes that are not two or three letters are dropped (see
/// [`PrefixTable::retain_valid`]). Then, in registry order, each code is
/// joined up (`RE-36` becomes `RE36`) and kept when it is valid, no
/// earlier agent has it, and its prefix is its key's: the one the key
/// reserved, else the prefix of the key's first kept code, which no other
/// key may have reserved or used before. Every other code is dropped, for
/// the backfill to replace. Codes already in the current format and
/// consistent are left as they are, so this changes nothing the second
/// time.
pub(super) fn migrate(agents: &mut [RegisteredAgent], prefixes: &mut PrefixTable) -> bool {
    let mut changed = prefixes.retain_valid();
    // Claims as they stand while walking the agents. The table itself is
    // left to `fill_from_codes`, so it only holds what was reserved.
    let mut prefix_of_key = prefixes.0.clone();
    let mut key_of_prefix: BTreeMap<String, String> = prefix_of_key
        .iter()
        .map(|(key, prefix)| (prefix.clone(), key.clone()))
        .collect();
    let mut seen: HashSet<String> = HashSet::new();
    for agent in agents.iter_mut() {
        let Some(code) = agent.code.as_deref() else {
            continue;
        };
        let key = source_key(agent).to_string();
        let kept = current_form(code).filter(|code| {
            let Some((prefix, _)) = parse(code) else {
                return false;
            };
            let is_keys = match prefix_of_key.get(&key) {
                Some(own) => own == prefix,
                None => !key_of_prefix.contains_key(prefix),
            };
            is_keys && !seen.contains(code)
        });
        if let Some(code) = &kept {
            if let Some((prefix, _)) = parse(code) {
                prefix_of_key
                    .entry(key.clone())
                    .or_insert_with(|| prefix.to_string());
                key_of_prefix.entry(prefix.to_string()).or_insert(key);
            }
            seen.insert(code.clone());
        }
        if kept.as_deref() != agent.code.as_deref() {
            agent.code = kept;
            changed = true;
        }
    }
    changed
}

/// The highest number each code prefix has ever been given, persisted
/// with the registry so a number freed by dropping its agent is never
/// handed out again.
///
/// Invariants: an entry is never lowered or removed, and, once the
/// registry is loaded, no code in it has a number above its prefix's
/// entry (see [`LastNumbers::raise_to_codes`] and [`assign`]).
#[derive(Debug, Default, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub(super) struct LastNumbers(BTreeMap<String, u64>);

impl LastNumbers {
    /// Whether no prefix has been given a number yet.
    pub(super) fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    /// The highest number `prefix` has been given, `0` when none.
    pub(super) fn last(&self, prefix: &str) -> u64 {
        self.0.get(prefix).copied().unwrap_or(0)
    }

    /// Raise `prefix`'s entry to `number` when it is lower. Returns
    /// whether it changed.
    fn raise(&mut self, prefix: &str, number: u64) -> bool {
        if number <= self.last(prefix) {
            return false;
        }
        self.0.insert(prefix.to_string(), number);
        true
    }

    /// Raise every prefix's entry to the highest number its codes in
    /// `agents` have, as a registry written before the entries existed
    /// needs. Codes that are not `<PREFIX><N>` are skipped. Returns
    /// whether any entry changed.
    pub(super) fn raise_to_codes(&mut self, agents: &[RegisteredAgent]) -> bool {
        let mut changed = false;
        for (prefix, number) in agents.iter().filter_map(|a| parse(a.code.as_deref()?)) {
            changed |= self.raise(prefix, number);
        }
        changed
    }
}

/// The code `agent` gets when it joins `agents`, which it must not be in
/// yet. Raises the prefix's entry in `last_numbers` to the number given.
///
/// The prefix comes from [`PrefixTable::prefix_for`], which reserves it in
/// `prefixes` when the agent's key had none. The number is one more than
/// the highest its prefix has ever had, so it was never anyone's; an
/// instance named `<definition> #<n>` with `n` above that keeps `n`, so
/// its name and code agree. When that code is taken already (only a hand
/// edit makes one) the number goes up until it is free.
pub(super) fn assign(
    agent: &RegisteredAgent,
    agents: &[RegisteredAgent],
    prefixes: &mut PrefixTable,
    last_numbers: &mut LastNumbers,
) -> String {
    let prefix = prefixes.prefix_for(source_key(agent), agents);
    let next = last_numbers.last(&prefix).saturating_add(1);
    let start = instance_number(agent).map_or(next, |own| own.max(next));
    give(&prefix, start, agents, last_numbers)
}

/// The code `agent`, registered already but without a code (a registry
/// written before codes existed), gets while the registry is loaded.
/// Raises the prefix's entry in `last_numbers` to the number given.
///
/// As [`assign`], except that an instance named `<definition> #<n>` keeps
/// `n` whenever no agent has that code yet, even at or below the highest
/// number given: the agent has carried `n` in its name all along, so this
/// hands out nothing new, and its name and code agree however the
/// registry orders its instances.
pub(super) fn backfill(
    agent: &RegisteredAgent,
    agents: &[RegisteredAgent],
    prefixes: &mut PrefixTable,
    last_numbers: &mut LastNumbers,
) -> String {
    let prefix = prefixes.prefix_for(source_key(agent), agents);
    let own = instance_number(agent).filter(|own| {
        let code = format!("{prefix}{own}");
        !agents
            .iter()
            .any(|a| a.code.as_deref() == Some(code.as_str()))
    });
    match own {
        Some(own) => give(&prefix, own, agents, last_numbers),
        None => assign(agent, agents, prefixes, last_numbers),
    }
}

/// `<prefix><n>` for the first `n` from `start` no agent of `agents` has,
/// with `prefix`'s entry in `last_numbers` raised to `n`.
fn give(
    prefix: &str,
    start: u64,
    agents: &[RegisteredAgent],
    last_numbers: &mut LastNumbers,
) -> String {
    let codes: HashSet<&str> = agents.iter().filter_map(|a| a.code.as_deref()).collect();
    // Only finitely many codes are taken, so a free number is found long
    // before the end of the range.
    let number = (start..=u64::MAX)
        .find(|number| !codes.contains(format!("{prefix}{number}").as_str()))
        .unwrap_or(start);
    last_numbers.raise(prefix, number);
    format!("{prefix}{number}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn derived_prefixes_follow_the_documented_order() {
        let cases: [(&str, &[&str]); 11] = [
            ("rust-engineer", &["RE", "RUE", "REN", "RUS"]),
            ("frontend-engineer", &["FE", "FRE", "FEN", "FRO"]),
            ("research-engineer", &["RE", "REE", "REN", "RES"]),
            ("big data_job", &["BD", "BDJ", "BID", "BDA", "BIG"]),
            ("gpt4-engineer", &["GE", "GPE", "GEN", "GPT"]),
            ("explorer", &["EX", "EXP", "EXL", "EXO", "EXR", "EXE"]),
            ("reviewer", &["RE", "REV", "REI", "REE", "REW", "RER"]),
            ("agent", &["AG", "AGE", "AGN", "AGT"]),
            ("--", &["AG", "AGE", "AGN", "AGT"]),
            ("re", &["RE"]),
            ("x", &["XX"]),
        ];
        for (key, expected) in cases {
            assert_eq!(derived(key), expected, "{key}");
        }
    }

    #[test]
    fn candidates_end_in_the_initial_then_any_three_letters() {
        let got: Vec<String> = candidates("re").take(4).collect();
        assert_eq!(got, ["RE", "RAA", "RAB", "RAC"]);
        let all: Vec<String> = candidates("x").collect();
        assert_eq!(all.len(), 1 + 26 * 26 + 26 * 26 * 26);
        assert_eq!(all[1], "XAA");
        assert_eq!(all[26 * 26], "XZZ");
        assert_eq!(all[26 * 26 + 1], "AAA");
        assert_eq!(all.last().map(String::as_str), Some("ZZZ"));
        assert!(all.iter().all(|prefix| is_prefix(prefix)));
    }

    #[test]
    fn parse_accepts_only_letters_then_a_positive_number() {
        assert_eq!(parse("RE36"), Some(("RE", 36)));
        assert_eq!(parse("REV1"), Some(("REV", 1)));
        assert_eq!(parse("AG2"), Some(("AG", 2)));
        let bad = [
            "RE-36", "R1", "REVI1", "RE0", "RE036", "re1", "RE", "36", "RE2-1", "RE 1", "RE1X",
            "ÉE1", "",
        ];
        for code in bad {
            assert_eq!(parse(code), None, "{code}");
        }
    }

    #[test]
    fn older_codes_are_joined_up_when_their_prefix_is_two_or_three_letters() {
        let cases = [
            ("RE-36", Some("RE36")),
            ("REV-1", Some("REV1")),
            ("AG5", Some("AG5")),
            ("REVI-1", None),
            ("RE2-1", None),
            ("R-1", None),
            ("RE-0", None),
            ("RE-x", None),
            ("re-1", None),
        ];
        for (code, expected) in cases {
            assert_eq!(current_form(code).as_deref(), expected, "{code}");
        }
    }

    /// 100 distinct keys whose initials and starts clash a lot: every pair
    /// of a few words, and each word alone.
    fn clashing_keys() -> Vec<String> {
        let words = [
            "rust", "research", "review", "re", "engineer", "explorer", "e", "agent", "x", "test",
        ];
        (0..100)
            .map(|i| match (words[i / 10], words[i % 10]) {
                (a, b) if a == b => a.to_string(),
                (a, b) => format!("{a}-{b}"),
            })
            .collect()
    }

    #[test]
    fn prefixes_are_unique_letters_of_two_or_three_over_many_clashing_keys() {
        let keys = clashing_keys();
        let mut table = PrefixTable::default();
        let prefixes: Vec<String> = keys.iter().map(|key| table.prefix_for(key, &[])).collect();

        let unique: HashSet<&String> = prefixes.iter().collect();
        assert_eq!(unique.len(), keys.len(), "{prefixes:?}");
        for (i, (key, prefix)) in keys.iter().zip(&prefixes).enumerate() {
            assert!((2..=3).contains(&prefix.len()), "{key}: {prefix}");
            assert!(prefix.chars().all(|c| c.is_ascii_uppercase()), "{prefix}");
            if prefix.len() == 3 {
                let two = &derived(key)[0];
                assert!(prefixes[..i].contains(two), "{key} took {prefix}");
            }
        }
        // Asking again, or starting over, gives the same.
        let again: Vec<String> = keys.iter().map(|key| table.prefix_for(key, &[])).collect();
        assert_eq!(again, prefixes);
        let mut fresh = PrefixTable::default();
        let fresh: Vec<String> = keys.iter().map(|key| fresh.prefix_for(key, &[])).collect();
        assert_eq!(fresh, prefixes);
    }

    #[test]
    fn the_examples_in_the_module_docs_hold() {
        let mut table = PrefixTable::default();
        let keys = [
            "rust-engineer",
            "reviewer",
            "research-engineer",
            "explorer",
            "frontend-engineer",
            "agent",
        ];
        let got: Vec<String> = keys.iter().map(|key| table.prefix_for(key, &[])).collect();
        assert_eq!(got, ["RE", "REV", "REE", "EX", "FE", "AG"]);
    }

    #[test]
    fn invalid_reserved_prefixes_are_dropped() {
        let entries = [
            ("rust-engineer", "RE"),
            ("reviewer", "REVI"),
            ("research-engineer", "RE2"),
            ("tests", "re"),
            ("test", "TE"),
            ("testing", "TE"),
        ];
        let mut table = PrefixTable(
            entries
                .iter()
                .map(|(key, prefix)| (key.to_string(), prefix.to_string()))
                .collect(),
        );
        assert!(table.retain_valid());
        let kept: Vec<(&str, &str)> = table
            .0
            .iter()
            .map(|(key, prefix)| (key.as_str(), prefix.as_str()))
            .collect();
        assert_eq!(kept, [("rust-engineer", "RE"), ("test", "TE")]);
        assert!(!table.retain_valid());
    }

    /// An agent named `name`, an instance of `definition` when given,
    /// coded `code`.
    fn agent(name: &str, definition: Option<&str>, code: Option<&str>) -> RegisteredAgent {
        let mut agent = super::super::AgentRegistry::new_agent(name.to_string());
        agent.definition = definition.map(str::to_string);
        agent.code = code.map(str::to_string);
        agent
    }

    #[test]
    fn a_new_agent_is_numbered_above_the_highest_number_ever_given() {
        let mut prefixes = PrefixTable::default();
        let mut last = LastNumbers::default();
        last.raise("RE", 5);
        last.raise("AG", 2);
        let cases = [
            // Below the mark: the number may have been someone's.
            (
                agent("rust-engineer #2", Some("rust-engineer"), None),
                "RE6",
            ),
            // Above it: kept, so name and code agree.
            (
                agent("rust-engineer #9", Some("rust-engineer"), None),
                "RE9",
            ),
            (agent("Agent", None, None), "AG3"),
        ];
        for (new, expected) in cases {
            let mut marks = last.clone();
            assert_eq!(assign(&new, &[], &mut prefixes, &mut marks), expected);
            let (prefix, number) = parse(expected).unwrap();
            assert_eq!(marks.last(prefix), number, "{expected}");
        }
    }

    #[test]
    fn the_highest_number_given_is_never_lowered() {
        let mut last = LastNumbers::default();
        assert!(last.is_empty());
        assert!(last.raise("RE", 3));
        assert!(!last.raise("RE", 2));
        assert!(!last.raise("RE", 3));
        assert_eq!(last.last("RE"), 3);
        assert_eq!(last.last("AG"), 0);

        let agents = [
            agent("rust-engineer #1", Some("rust-engineer"), Some("RE1")),
            agent("rust-engineer #7", Some("rust-engineer"), Some("RE7")),
            agent("Agent", None, Some("AG4")),
            agent("Agent 2", None, Some("not a code")),
        ];
        assert!(last.raise_to_codes(&agents));
        assert!(!last.raise_to_codes(&agents));
        assert_eq!((last.last("RE"), last.last("AG")), (7, 4));
        assert!(!last.raise_to_codes(&agents[..1]), "RE1 lowers nothing");
        assert_eq!(last.last("RE"), 7);
    }

    #[test]
    fn a_backfilled_instance_keeps_its_number_while_its_code_is_free() {
        let mut prefixes = PrefixTable::default();
        let mut last = LastNumbers::default();
        last.raise("RE", 5);
        let agents = [agent(
            "rust-engineer #4",
            Some("rust-engineer"),
            Some("RE4"),
        )];
        let own = agent("rust-engineer #3", Some("rust-engineer"), None);
        assert_eq!(backfill(&own, &agents, &mut prefixes, &mut last), "RE3");
        assert_eq!(last.last("RE"), 5);
        // Its code is taken: numbered as a new agent would be.
        let clash = agent("rust-engineer #4", Some("rust-engineer"), None);
        assert_eq!(backfill(&clash, &agents, &mut prefixes, &mut last), "RE6");
        assert_eq!(last.last("RE"), 6);
    }

    #[test]
    fn the_largest_instance_number_does_not_overflow() {
        let mut prefixes = PrefixTable::default();
        let mut last = LastNumbers::default();
        let huge = agent(
            &format!("rust-engineer #{}", u64::MAX),
            Some("rust-engineer"),
            None,
        );
        let code = assign(&huge, &[], &mut prefixes, &mut last);
        assert_eq!(code, format!("RE{}", u64::MAX));
        assert_eq!(last.last("RE"), u64::MAX);
    }
}
