use crate::agent_loop::iteration::append_paragraph;

fn joined(blocks: &[&str]) -> String {
    let mut out = String::new();
    for block in blocks {
        append_paragraph(&mut out, block);
    }
    out
}

#[test]
fn first_block_into_empty_text_is_appended_unchanged() {
    assert_eq!(joined(&["Hello there."]), "Hello there.");
}

#[test]
fn two_blocks_are_separated_by_a_paragraph_break() {
    assert_eq!(
        joined(&[
            "I'll update the prompt text myself.",
            "All 3 clippy errors are fixed."
        ]),
        "I'll update the prompt text myself.\n\nAll 3 clippy errors are fixed."
    );
}

#[test]
fn existing_trailing_and_new_leading_newlines_are_not_doubled() {
    let cases: &[(&[&str], &str)] = &[
        (&["First.\n\n", "Second."], "First.\n\nSecond."),
        (&["First.\n", "\nSecond."], "First.\n\nSecond."),
        (&["First.  \n", "  \n\nSecond."], "First.\n\nSecond."),
    ];
    for (blocks, expected) in cases {
        assert_eq!(joined(blocks), *expected, "blocks: {blocks:?}");
    }
}

#[test]
fn whitespace_only_blocks_are_ignored() {
    assert_eq!(joined(&["", "  \n\t", "Only."]), "Only.");
    assert_eq!(joined(&["One.", "\n\n", "", "Two."]), "One.\n\nTwo.");
    assert_eq!(joined(&["Kept as is.\n", "   "]), "Kept as is.\n");
}

#[test]
fn internal_formatting_of_a_block_is_preserved() {
    assert_eq!(
        joined(&["Intro:\n- a\n- b", "Next:\n\n```\ncode\n```"]),
        "Intro:\n- a\n- b\n\nNext:\n\n```\ncode\n```"
    );
}
