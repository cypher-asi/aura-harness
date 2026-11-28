//! Subtask names from a `task` call's description, or from its prompt
//! with the boilerplate skipped, cut to fit a subtask row.

use super::*;

#[test]
fn a_description_is_the_subtitle_cleaned_and_cut_to_fit() {
    assert_eq!(
        described_subtitle(Some("  \"Merge queue store tests.\"  ")).as_deref(),
        Some("Merge queue store tests")
    );
    let long = described_subtitle(Some(
        "Merge queue store tests and the processor retry logic",
    ))
    .unwrap();
    assert!(long.chars().count() <= MAX_SUBTITLE_CHARS, "{long}");
    assert_eq!(long, "Merge queue store tests");
    assert_eq!(described_subtitle(Some("   ")), None);
    assert_eq!(described_subtitle(None), None);
}

#[test]
fn a_spawn_is_titled_by_its_description_before_its_brief() {
    let brief = || Some("You are working in /workspace. Fix the queue tab layout.".to_string());
    assert_eq!(
        subtitle_of_spawn(Some("Queue tab layout"), || panic!("read")).as_deref(),
        Some("Queue tab layout")
    );
    assert_eq!(
        subtitle_of_spawn(None, brief).as_deref(),
        Some("Queue tab layout")
    );
    assert_eq!(subtitle_of_spawn(Some(""), || None), None);
}

#[test]
fn the_fallback_skips_boilerplate_to_the_work() {
    let cases = [
        (
            "In the Rust workspace at /workspace, find where subagent titles are set.",
            "Find where subagent titles",
        ),
        (
            "You are working in the aura-harness repo at /workspace. Add tests for the \
             merge queue store.",
            "Tests for the merge queue",
        ),
        (
            "## Context\nThe repo is at /workspace/main.rs.\n## Task\nImplement the queue \
             tab layout in the terminal UI.",
            "Queue tab layout",
        ),
        (
            "Task: review PR #56's merge queue",
            "Review PR #56's merge queue",
        ),
        ("Your task is to write the inbox tests.", "Inbox tests"),
        (
            "Context: the build is slow.\n- **Profile** the `cargo` build",
            "Profile the cargo build",
        ),
        (
            "1. Do not commit.\n2. Fix the parser escapes.",
            "Parser escapes",
        ),
        ("In the Rust workspace.\nUpdate the README", "README"),
    ];
    for (prompt, title) in cases {
        assert_eq!(fallback_subtitle(prompt), title, "{prompt:?}");
    }
}

#[test]
fn a_prompt_of_only_boilerplate_is_named_from_its_first_line() {
    assert_eq!(
        fallback_subtitle("You are a helpful agent.\nDo not commit."),
        "You are a helpful agent"
    );
    assert_eq!(fallback_subtitle("\n\n"), "Subtask");
}

#[test]
fn fallback_titles_fit_a_subtask_row() {
    let prompts = [
        "Write the integration tests for every endpoint of the cloud gateway's session API",
        &"é".repeat(80),
        "In the workspace, supercalifragilisticexpialidociousandmorewordsthatnevercut",
    ];
    for prompt in prompts {
        let title = fallback_subtitle(prompt);
        assert!(title.chars().count() <= MAX_SUBTITLE_CHARS, "{title}");
        assert!(!title.ends_with(' '), "{title}");
    }
    assert_eq!(fallback_subtitle(prompts[0]), "Integration tests for every");
}
