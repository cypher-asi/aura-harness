use super::label::{arms, name};
use crate::events::{BenchArm, BenchTaskRow, BenchView};

/// `Compare #51`, about `title`, from `source`, Gate against Opus.
fn view(title: &str, source: &str) -> BenchView {
    let arm = |label: &str| BenchArm {
        label: label.to_string(),
        ..BenchArm::default()
    };
    BenchView {
        name: "Compare #51".to_string(),
        title: title.to_string(),
        source: source.to_string(),
        arms: [arm("gate"), arm("claude/claude-opus-5-5")],
        ..BenchView::default()
    }
}

/// [`view`] of example tasks `ids`.
fn of_tasks(ids: &[&str]) -> BenchView {
    let mut view = view("20 example coding tasks", "20 example tasks");
    view.tasks = ids
        .iter()
        .map(|id| BenchTaskRow {
            id: (*id).to_string(),
            ..BenchTaskRow::default()
        })
        .collect();
    view
}

#[test]
fn example_tasks_read_as_their_suite_and_how_many() {
    assert_eq!(
        name(&of_tasks(&["polyglot/go-bowling", "polyglot/go-connect"])),
        "#51 polyglot ×2"
    );
    let swe: Vec<String> = (0..20)
        .map(|n| format!("swe_bench_pro_hard/task-{n}"))
        .collect();
    let swe: Vec<&str> = swe.iter().map(String::as_str).collect();
    assert_eq!(name(&of_tasks(&swe)), "#51 swe-pro ×20", "its short name");
    assert_eq!(
        name(&of_tasks(&["coding/write-file", "gate_hard/retry-spec"])),
        "#51 mixed ×2",
        "several suites"
    );
}

#[test]
fn a_prompt_reads_as_its_first_line_cut_short() {
    assert_eq!(
        name(&view("fix the parser", "prompt")),
        "#51 fix the parser"
    );
    assert_eq!(
        name(&view(
            "\n  rewrite the tokenizer so it streams\nthen",
            "prompt"
        )),
        "#51 rewrite the tokeniz…",
        "the arms still fit after it"
    );
}

#[test]
fn a_session_and_a_plan_say_so() {
    assert_eq!(
        name(&view("Task build issue", "session \"Task build issue\"")),
        "#51 ↻ Task build issue"
    );
    assert_eq!(
        name(&view("Build the plan", "plan plans/parser.md")),
        "#51 plan parser.md",
        "the file without its directory"
    );
}

#[test]
fn a_name_without_compare_is_kept_whole() {
    let mut other = view("fix the parser", "prompt");
    other.name = "Parser bench".to_string();
    assert_eq!(name(&other), "Parser bench fix the parser");
    other.title.clear();
    assert_eq!(name(&other), "Parser bench", "with no work to name");
}

#[test]
fn the_arms_read_as_their_short_models() {
    assert_eq!(
        arms(&view("fix the parser", "prompt")).as_deref(),
        Some("gate vs opus-5-5")
    );
    let mut older = view("fix the parser", "prompt");
    older.arms[1].label.clear();
    assert_eq!(arms(&older), None, "an arm recorded with no label");
}
