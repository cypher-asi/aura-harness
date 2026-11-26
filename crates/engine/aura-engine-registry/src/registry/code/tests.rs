use super::*;

#[test]
fn derived_prefixes_follow_the_documented_order() {
    let cases: [(&str, &[&str]); 14] = [
        ("rust-engineer", &["REN", "RUE", "RUS"]),
        ("frontend-engineer", &["FEN", "FRE", "FRO"]),
        ("research-engineer", &["REN", "REE", "RES"]),
        ("big data_job", &["BDJ", "BDA", "BID", "BIG"]),
        ("gpt4-engineer", &["GEN", "GPE", "GPT"]),
        ("explorer", &["EXP", "EXL", "EXO", "EXR", "EXE"]),
        ("reviewer", &["REV", "REI", "REE", "REW", "RER"]),
        ("agent", &["AGE", "AGN", "AGT"]),
        ("--", &["AGE", "AGN", "AGT"]),
        ("rust-e", &["RUE", "RUS"]),
        ("re-x", &["REX"]),
        ("re", &["REE"]),
        ("x", &["XXX"]),
        ("e-x", &["EXX"]),
    ];
    for (key, expected) in cases {
        assert_eq!(derived(key), expected, "{key}");
    }
}

#[test]
fn candidates_end_in_the_initial_then_any_three_letters() {
    let got: Vec<String> = candidates("re").take(4).collect();
    assert_eq!(got, ["REE", "RAA", "RAB", "RAC"]);
    let got: Vec<String> = candidates("e-x").take(2).collect();
    assert_eq!(got, ["EXX", "EAA"], "the first word's initial");
    let all: Vec<String> = candidates("x").collect();
    assert_eq!(all.len(), 1 + 26 * 26 + 26 * 26 * 26);
    assert_eq!(all[0], "XXX");
    assert_eq!(all[1], "XAA");
    assert_eq!(all[26 * 26], "XZZ");
    assert_eq!(all[26 * 26 + 1], "AAA");
    assert_eq!(all.last().map(String::as_str), Some("ZZZ"));
    assert!(all.iter().all(|prefix| is_prefix(prefix)));
}

#[test]
fn a_prefix_is_exactly_three_uppercase_letters() {
    for prefix in ["REN", "AGE", "XXX"] {
        assert!(is_prefix(prefix), "{prefix}");
    }
    for prefix in ["", "R", "RE", "AG", "REVI", "ren", "RE2", "ÉEE"] {
        assert!(!is_prefix(prefix), "{prefix}");
    }
}

#[test]
fn parse_accepts_only_three_letters_then_a_positive_number() {
    assert_eq!(parse("REN36"), Some(("REN", 36)));
    assert_eq!(parse("REV1"), Some(("REV", 1)));
    assert_eq!(parse("AGE2"), Some(("AGE", 2)));
    let bad = [
        "REN-36", "RE36", "AG2", "R1", "REVI1", "REN0", "REN036", "ren1", "REN", "36", "REN2-1",
        "REN 1", "REN1X", "ÉEE1", "",
    ];
    for code in bad {
        assert_eq!(parse(code), None, "{code}");
    }
}

#[test]
fn older_codes_are_joined_up_only_when_their_prefix_is_three_letters() {
    let cases = [
        ("REN-36", Some("REN36")),
        ("REV-1", Some("REV1")),
        ("AGE5", Some("AGE5")),
        ("RE-36", None),
        ("RE36", None),
        ("AG5", None),
        ("REVI-1", None),
        ("RE2-1", None),
        ("R-1", None),
        ("REN-0", None),
        ("REN-x", None),
        ("ren-1", None),
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
fn prefixes_are_unique_and_three_letters_over_many_clashing_keys() {
    let keys = clashing_keys();
    let mut table = PrefixTable::default();
    let prefixes: Vec<String> = keys.iter().map(|key| table.prefix_for(key, &[])).collect();

    let unique: HashSet<&String> = prefixes.iter().collect();
    assert_eq!(unique.len(), keys.len(), "{prefixes:?}");
    for (i, (key, prefix)) in keys.iter().zip(&prefixes).enumerate() {
        assert_eq!(prefix.len(), 3, "{key}: {prefix}");
        assert!(prefix.chars().all(|c| c.is_ascii_uppercase()), "{prefix}");
        // Every better candidate was taken by an earlier key.
        for better in candidates(key).take_while(|candidate| candidate != prefix) {
            assert!(
                prefixes[..i].contains(&better),
                "{key} took {prefix} over the free {better}"
            );
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
    assert_eq!(got, ["REN", "REV", "REE", "EXP", "FEN", "AGE"]);
}

#[test]
fn invalid_reserved_prefixes_are_dropped() {
    let entries = [
        ("rust-engineer", "RE"),
        ("agent", "AG"),
        ("reviewer", "REVI"),
        ("research-engineer", "RE2"),
        ("tests", "tes"),
        ("explorer", "EXP"),
        ("test", "TES"),
        ("testing", "TES"),
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
    assert_eq!(kept, [("explorer", "EXP"), ("test", "TES")]);
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
fn migrating_two_letter_codes_drops_them_and_their_entries() {
    let mut agents = [
        agent("Agent", None, Some("AG1")),
        agent("rust-engineer #36", Some("rust-engineer"), Some("RE36")),
        agent("rust-engineer #37", Some("rust-engineer"), Some("RE-37")),
        agent("reviewer #1", Some("reviewer"), Some("REV1")),
    ];
    let mut prefixes = PrefixTable(
        [
            ("agent", "AG"),
            ("rust-engineer", "RE"),
            ("reviewer", "REV"),
        ]
        .iter()
        .map(|(key, prefix)| (key.to_string(), prefix.to_string()))
        .collect(),
    );
    let mut last = LastNumbers::default();
    last.raise("AG", 1);
    last.raise("RE", 37);
    last.raise("REV", 1);

    assert!(migrate(&mut agents, &mut prefixes, &mut last));
    let codes: Vec<Option<&str>> = agents.iter().map(|a| a.code.as_deref()).collect();
    assert_eq!(codes, [None, None, None, Some("REV1")]);
    assert_eq!(prefixes.get("reviewer"), Some("REV"));
    assert_eq!(
        (prefixes.get("agent"), prefixes.get("rust-engineer")),
        (None, None)
    );
    assert_eq!(last, {
        let mut only = LastNumbers::default();
        only.raise("REV", 1);
        only
    });
    assert!(!migrate(&mut agents, &mut prefixes, &mut last));

    // The backfill gives them three letters, instances keeping `n`.
    for index in 0..agents.len() {
        if agents[index].code.is_none() {
            let code = backfill(&agents[index], &agents, &mut prefixes, &mut last);
            agents[index].code = Some(code);
        }
    }
    let codes: Vec<Option<&str>> = agents.iter().map(|a| a.code.as_deref()).collect();
    assert_eq!(
        codes,
        [Some("AGE1"), Some("REN36"), Some("REN37"), Some("REV1")]
    );
}

#[test]
fn a_new_agent_is_numbered_above_the_highest_number_ever_given() {
    let mut prefixes = PrefixTable::default();
    let mut last = LastNumbers::default();
    last.raise("REN", 5);
    last.raise("AGE", 2);
    let cases = [
        // Below the mark: the number may have been someone's.
        (
            agent("rust-engineer #2", Some("rust-engineer"), None),
            "REN6",
        ),
        // Above it: kept, so name and code agree.
        (
            agent("rust-engineer #9", Some("rust-engineer"), None),
            "REN9",
        ),
        (agent("Agent", None, None), "AGE3"),
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
    assert!(last.raise("REN", 3));
    assert!(!last.raise("REN", 2));
    assert!(!last.raise("REN", 3));
    assert_eq!(last.last("REN"), 3);
    assert_eq!(last.last("AGE"), 0);

    let agents = [
        agent("rust-engineer #1", Some("rust-engineer"), Some("REN1")),
        agent("rust-engineer #7", Some("rust-engineer"), Some("REN7")),
        agent("Agent", None, Some("AGE4")),
        agent("Agent 2", None, Some("not a code")),
        agent("Agent 3", None, Some("AG9")),
    ];
    assert!(last.raise_to_codes(&agents));
    assert!(!last.raise_to_codes(&agents));
    assert_eq!((last.last("REN"), last.last("AGE")), (7, 4));
    assert_eq!(last.last("AG"), 0, "AG9 is not a code");
    assert!(!last.raise_to_codes(&agents[..1]), "REN1 lowers nothing");
    assert_eq!(last.last("REN"), 7);
}

#[test]
fn a_backfilled_instance_keeps_its_number_while_its_code_is_free() {
    let mut prefixes = PrefixTable::default();
    let mut last = LastNumbers::default();
    last.raise("REN", 5);
    let agents = [agent(
        "rust-engineer #4",
        Some("rust-engineer"),
        Some("REN4"),
    )];
    let own = agent("rust-engineer #3", Some("rust-engineer"), None);
    assert_eq!(backfill(&own, &agents, &mut prefixes, &mut last), "REN3");
    assert_eq!(last.last("REN"), 5);
    // Its code is taken: numbered as a new agent would be.
    let clash = agent("rust-engineer #4", Some("rust-engineer"), None);
    assert_eq!(backfill(&clash, &agents, &mut prefixes, &mut last), "REN6");
    assert_eq!(last.last("REN"), 6);
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
    assert_eq!(code, format!("REN{}", u64::MAX));
    assert_eq!(last.last("REN"), u64::MAX);
}
