use super::*;
use CompareArm::{A, B};

fn ended(arm: CompareArm, n: u32, errored: bool) -> EndedRun {
    EndedRun { arm, n, errored }
}

/// `a-1 b-1 a-2 b-2 a-3 b-3 a-4 b-4` in that order; `a-2` and `b-3`
/// errored.
fn runs() -> [EndedRun; 8] {
    [
        ended(A, 1, false),
        ended(B, 1, false),
        ended(A, 2, true),
        ended(B, 2, false),
        ended(A, 3, false),
        ended(B, 3, true),
        ended(A, 4, false),
        ended(B, 4, false),
    ]
}

#[test]
fn without_examples_the_finished_runs_pair_in_order() {
    assert_eq!(judge_pairs(&runs(), false), [(0, 1), (4, 3), (6, 7)]);
    let more_a = [ended(A, 1, false), ended(B, 1, false), ended(A, 2, false)];
    assert_eq!(judge_pairs(&more_a, false), [(0, 1)], "a-2 has no partner");
    assert!(judge_pairs(&[], false).is_empty());
}

#[test]
fn with_examples_runs_pair_with_the_same_number() {
    assert_eq!(
        judge_pairs(&runs(), true),
        [(0, 1), (6, 7)],
        "a-3's partner errored"
    );
    let twice = [
        ended(A, 1, false),
        ended(B, 1, false),
        ended(B, 1, false),
        ended(A, 1, false),
    ];
    assert_eq!(
        judge_pairs(&twice, true),
        [(0, 1), (3, 1)],
        "the first of that number"
    );
    assert_eq!(judge_pairs(&twice, false), [(0, 1), (3, 2)]);
}

#[test]
fn the_seed_is_the_first_eight_bytes_of_the_agent_id() {
    let bytes: [u8; 32] = std::array::from_fn(|i| u8::try_from(i + 1).unwrap());
    assert_eq!(seed(AgentId::new(bytes)), 0x0807_0605_0403_0201);
    assert_eq!(seed(AgentId::new([0xff; 32])), u64::MAX);
}

#[test]
fn the_judge_is_told_the_prompt_or_every_prompt_in_order() {
    let task = |prompts: &[&str]| {
        let prompts: Vec<String> = prompts.iter().map(ToString::to_string).collect();
        judge_task(&prompts)
    };
    assert_eq!(task(&["only one"]), "only one");
    assert_eq!(task(&[]), "The user sent these messages in order:\n");
    assert_eq!(
        task(&["first", "second\nline"]),
        "The user sent these messages in order:\n\
         \n### Message 1\nfirst\n\
         \n### Message 2\nsecond\nline\n"
    );
    assert_eq!(
        task(&["ä", "b", "c"]),
        "The user sent these messages in order:\n\
         \n### Message 1\nä\n\
         \n### Message 2\nb\n\
         \n### Message 3\nc\n"
    );
}
