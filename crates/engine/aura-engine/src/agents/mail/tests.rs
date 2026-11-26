use super::*;

#[test]
fn normalization_strips_markers_case_and_spacing() {
    let cases = [
        ("Re: RE:  Foo", "foo"),
        ("  fwd: Re:fw: Foo   Bar ", "foo bar"),
        ("FOO", "foo"),
        ("Reply needed", "reply needed"),
        ("re:", ""),
        ("   ", ""),
    ];
    for (input, expected) in cases {
        assert_eq!(normalize_subject(input), expected, "{input:?}");
    }
}

#[test]
fn same_subject_in_the_same_tree_joins_the_thread() {
    let mut threads = Threads::default();
    let (root, a, b, c) = ids();
    let first = threads.resolve(root, a, b, Some("Parser review"), "hi");
    let again = threads.resolve(root, c, a, Some("Re: RE:  parser REVIEW"), "x");
    assert_eq!(first.thread, again.thread);
    assert_eq!(first.subject, "Parser review");
    assert_eq!(again.subject, "Parser review");
    assert_eq!(first.thread, ThreadId("t1".into()));
}

#[test]
fn different_subject_or_tree_starts_a_new_thread() {
    let mut threads = Threads::default();
    let (root, a, b, _) = ids();
    let other_root = AgentId::generate();
    let first = threads.resolve(root, a, b, Some("Parser"), "hi").thread;
    let other_subject = threads.resolve(root, a, b, Some("Benchmarks"), "hi").thread;
    let other_tree = threads
        .resolve(other_root, a, b, Some("Parser"), "hi")
        .thread;
    assert_ne!(first, other_subject);
    assert_ne!(first, other_tree);
    assert_ne!(other_subject, other_tree);
}

#[test]
fn no_subject_continues_the_last_thread_between_the_pair() {
    let mut threads = Threads::default();
    let (root, a, b, c) = ids();
    let first = threads.resolve(root, a, b, Some("Parser"), "hi");
    threads.record(a, b, &first.thread, &first.subject);

    let reply = threads.resolve(root, b, a, None, "answer");
    assert_eq!(reply.thread, first.thread);
    assert_eq!(reply.subject, "Parser");

    let fresh = threads.resolve(root, a, c, None, "\n  Check the lexer\nmore");
    assert_ne!(fresh.thread, first.thread);
    assert_eq!(fresh.subject, "Check the lexer");
}

#[test]
fn the_latest_recorded_thread_wins_for_the_pair() {
    let mut threads = Threads::default();
    let (root, a, b, _) = ids();
    let first = threads.resolve(root, a, b, Some("One"), "x");
    threads.record(a, b, &first.thread, &first.subject);
    let second = threads.resolve(root, a, b, Some("Two"), "x");
    threads.record(b, a, &second.thread, &second.subject);
    assert_eq!(threads.resolve(root, a, b, None, "x").thread, second.thread);
}

#[test]
fn new_thread_always_creates_and_can_be_joined_by_subject() {
    let mut threads = Threads::default();
    let (root, a, b, _) = ids();
    let first = threads.new_thread(root, "Look at the parser");
    let second = threads.new_thread(root, "Look at the parser");
    assert_ne!(first.thread, second.thread);
    assert_eq!(first.subject, "Look at the parser");
    let joined = threads.resolve(root, b, a, Some("re: look at the parser"), "x");
    assert_eq!(
        joined.thread, second.thread,
        "the newest thread with that subject"
    );
}

#[test]
fn only_a_thread_created_from_the_text_is_marked() {
    let mut threads = Threads::default();
    let (root, a, b, c) = ids();

    let started = threads.new_thread(root, "Look at the parser");
    assert!(
        started.from_text,
        "new_thread names the thread after the text"
    );

    let fallback = threads.resolve(root, a, b, None, "Check the lexer");
    assert!(
        fallback.from_text,
        "no pair and no subject: named from text"
    );
    threads.record(a, b, &fallback.thread, &fallback.subject);

    let given = threads.resolve(root, a, c, Some("Benchmarks"), "x");
    assert!(!given.from_text, "an explicit subject is not derived");

    let continued = threads.resolve(root, b, a, None, "answer");
    assert_eq!(continued.thread, fallback.thread);
    assert!(!continued.from_text, "continuing the pair creates nothing");

    let joined = threads.resolve(root, c, b, Some("re: look at the parser"), "x");
    assert_eq!(joined.thread, started.thread);
    assert!(
        !joined.from_text,
        "joining an existing subject creates nothing"
    );
}

#[test]
fn derived_subjects_are_cut_on_char_boundaries() {
    let long = "é".repeat(80);
    let subject = derived_subject(&long);
    assert_eq!(subject.chars().count(), DERIVED_SUBJECT_CHARS + 1);
    assert!(subject.ends_with('…'));
    assert_eq!(derived_subject(&"a".repeat(60)), "a".repeat(60));
    assert_eq!(derived_subject(" \n\t\n"), EMPTY_SUBJECT);
}

#[test]
fn message_ids_increase() {
    let mut threads = Threads::default();
    let (root, a, b, _) = ids();
    let Resolved {
        thread, subject, ..
    } = threads.new_thread(root, "x");
    let first = threads.record(a, b, &thread, &subject);
    let second = threads.record(b, a, &thread, &subject);
    assert!(second > first);
}

#[test]
fn index_cap_evicts_the_entry_written_longest_ago() {
    let mut index = BoundedIndex::new(2);
    index.insert("a", 1);
    index.insert("b", 2);
    index.insert("a", 3);
    index.insert("c", 4);
    assert_eq!(index.len(), 2);
    assert_eq!(index.get(&"b"), None, "b was written longest ago");
    assert_eq!(index.get(&"a"), Some(&3), "rewriting a kept it fresh");
    assert_eq!(index.get(&"c"), Some(&4));

    for round in 0..100 {
        index.insert("a", round);
    }
    assert!(index.order.len() <= 2 * 2 + 1, "stale stamps are compacted");
    assert_eq!(index.get(&"c"), Some(&4));
}

#[test]
fn capped_threads_forget_the_oldest_pair_and_subject() {
    let mut threads = Threads::with_index_cap(2);
    let (root, a, b, c) = ids();
    let d = AgentId::generate();
    let ab = threads.resolve(root, a, b, Some("First"), "x");
    threads.record(a, b, &ab.thread, &ab.subject);
    let ac = threads.resolve(root, a, c, Some("Second"), "x");
    threads.record(a, c, &ac.thread, &ac.subject);
    let ad = threads.resolve(root, a, d, Some("Third"), "x");
    threads.record(a, d, &ad.thread, &ad.subject);

    assert_eq!(
        threads.resolve(root, a, b, Some("Third"), "x").thread,
        ad.thread
    );
    assert_ne!(
        threads.resolve(root, a, b, Some("First"), "x").thread,
        ab.thread,
        "the oldest subject was evicted"
    );
    assert_ne!(
        threads.resolve(root, b, a, None, "x").thread,
        ab.thread,
        "a↔b evicted"
    );
    assert_eq!(threads.resolve(root, d, a, None, "x").thread, ad.thread);
}

fn ids() -> (AgentId, AgentId, AgentId, AgentId) {
    (
        AgentId::generate(),
        AgentId::generate(),
        AgentId::generate(),
        AgentId::generate(),
    )
}
