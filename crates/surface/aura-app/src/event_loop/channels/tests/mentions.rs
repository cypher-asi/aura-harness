//! Parsing member mentions and rendering the chat digest.

use super::super::chat::{parse_member_mentions, render_digest, DIGEST_HEADER};
use super::super::*;
use super::support::agent;

#[test]
fn member_mentions_match_codes_and_leave_other_words_alone() {
    let (re, fx) = (agent(1), agent(2));
    let members = [
        (re, Some("RE36".to_string())),
        (fx, Some("FX2".to_string())),
        (agent(3), None),
    ];
    let cases: &[(&str, &[AgentId], bool)] = &[
        ("@re36, take a look", &[re], false),
        ("@FX2: and @RE36! then @fx2?", &[fx, re], false),
        ("@coordinator what now?", &[], true),
        ("@Coordinator. @RE36;", &[re], true),
        ("@foo and @src/main.rs and mail@RE36", &[], false),
        ("@ alone, @RE3 is not RE36", &[], false),
    ];
    for (text, mentioned, coordinator) in cases {
        assert_eq!(
            parse_member_mentions(text, &members),
            (mentioned.to_vec(), *coordinator),
            "{text}"
        );
    }
}

#[test]
fn the_digest_keeps_the_newest_lines_that_fit() {
    assert_eq!(render_digest(&[]), None);
    let lines: Vec<String> = (0..100)
        .map(|i| format!("{i:03}: {}", "x".repeat(95)))
        .collect();
    let digest = render_digest(&lines).unwrap();
    assert!(digest.len() <= 4096, "{}", digest.len());
    assert!(digest.starts_with(DIGEST_HEADER));
    assert!(digest.ends_with(&lines[99]), "the newest line is kept");
    assert!(!digest.contains("000: "), "the oldest lines are dropped");
    let kept: Vec<&str> = digest.lines().skip(1).collect();
    let expected: Vec<&str> = lines[lines.len() - kept.len()..]
        .iter()
        .map(String::as_str)
        .collect();
    assert_eq!(kept, expected, "oldest first");
}
