use super::*;
use crate::app::plans::PLAN_HEADER_END;

fn parse(text: &str) -> PlanDoc {
    PlanDoc::parse(
        text,
        Path::new("/w/.aura/plans/p_00000001.plan.md"),
        ".aura/plans/p_00000001.plan.md",
    )
}

fn todo(state: TodoState, content: &str, assignee: Option<&str>) -> PlanTodo {
    PlanTodo {
        state,
        content: content.to_string(),
        assignee: assignee.map(str::to_string),
    }
}

#[test]
fn a_generated_plan_reads_as_title_overview_todos_and_body() {
    let text = format!(
        "---\nname: Plans that read like a document\n\
             overview: Change the guidance so a plan reads as a document.\n\
             todos:\n\
             - id: doc-view\n  content: Add a rendered plan view\n  status: in_progress\n  \
             assignee: rust-engineer\n\
             - id: review\n  content: Review the change\n  status: pending\n---\n\n\
             # Plans that read like a document\n\n\
             Change the guidance so a plan reads as a document.\n\n\
             ## Todos\n\n\
             - [~] Add a rendered plan view (@rust-engineer)\n\
             - [ ] Review the change\n\n\
             {PLAN_HEADER_END}\n\n\
             ## Why plans read badly today\n\nThe agent writes plans.\n\n\
             ```rust\nfn main() {{}}\n```\n"
    );
    let doc = parse(&text);
    assert_eq!(doc.title, "Plans that read like a document");
    assert_eq!(
        doc.overview,
        "Change the guidance so a plan reads as a document."
    );
    assert_eq!(
        doc.todos,
        [
            todo(
                TodoState::InProgress,
                "Add a rendered plan view",
                Some("rust-engineer")
            ),
            todo(TodoState::Pending, "Review the change", None),
        ]
    );
    assert_eq!(
        doc.body,
        "## Why plans read badly today\n\nThe agent writes plans.\n\n\
             ```rust\nfn main() {}\n```"
    );
    assert_eq!(doc.progress(), (0, 2));
    assert_eq!(doc.path, ".aura/plans/p_00000001.plan.md");
}

#[test]
fn a_plan_without_a_generated_header_reads_its_frontmatter() {
    let doc = parse(
        "---\nname: 'It''s old'\noverview: |-\n  First line.\n  Second line.\n\
             todos:\n- id: a\n  content: \"Do \\\"it\\\"\"\n  status: completed\n\
             - id: b\n  content: >-\n    Folded\n    text\n  status: cancelled\n---\n\n\
             # It's old\n\nBody text.\n",
    );
    assert_eq!(doc.title, "It's old");
    assert_eq!(doc.overview, "First line.\nSecond line.");
    assert_eq!(
        doc.todos,
        [
            todo(TodoState::Completed, "Do \"it\"", None),
            todo(TodoState::Cancelled, "Folded text", None),
        ]
    );
    assert_eq!(doc.body, "Body text.", "the title heading is not repeated");
    assert_eq!(doc.progress(), (2, 2));
}

#[test]
fn a_file_with_neither_takes_its_title_from_a_heading_or_its_name() {
    let doc = parse("Intro.\n\n# Heading\n\nMore.\n");
    assert_eq!(doc.title, "Heading");
    assert!(doc.overview.is_empty() && doc.todos.is_empty());
    assert_eq!(doc.body, "Intro.\n\n# Heading\n\nMore.");

    let doc = parse("just text\n");
    assert_eq!(doc.title, "p_00000001");
    assert_eq!(doc.body, "just text");
}

#[test]
fn multi_line_and_cancelled_todos_join_their_continuation_lines() {
    let text = format!(
        "---\nname: P\ntodos: []\n---\n\n# P\n\nOverview one.\n\nOverview two.\n\n\
             ## Todos\n\n\
             - [x] done\n\
             - [~] second\n  with details (@rust-engineer)\n\
             - [ ] third\n\n  after a blank\n\
             - [-] dropped\n\n\
             {PLAN_HEADER_END}\n\nBody.\n{PLAN_HEADER_END}\nMore.\n"
    );
    let doc = parse(&text);
    assert_eq!(doc.overview, "Overview one.\n\nOverview two.");
    assert_eq!(
        doc.todos,
        [
            todo(TodoState::Completed, "done", None),
            todo(
                TodoState::InProgress,
                "second\nwith details",
                Some("rust-engineer")
            ),
            todo(TodoState::Pending, "third\n\nafter a blank", None),
            todo(TodoState::Cancelled, "dropped", None),
        ]
    );
    assert_eq!(doc.body, "Body.\nMore.", "marker lines never show");
    assert_eq!(doc.progress(), (2, 4));
}

#[test]
fn a_marker_quoted_in_the_overview_does_not_end_the_header() {
    let text = format!(
        "---\nname: P\n---\n\n# P\n\nSee {PLAN_HEADER_END}\n{PLAN_HEADER_END}\n\n\
             ## Todos\n\n- [ ] one\n\n{PLAN_HEADER_END}\n\nBody.\n"
    );
    let doc = parse(&text);
    assert_eq!(doc.todos.len(), 1);
    assert_eq!(doc.body, "Body.");
}

#[test]
fn focus_finds_todos_by_id_and_by_position_or_text() {
    let text = format!(
        "---\nname: P\ntodos:\n- id: x\n  content: first\n- id: y\n  content: second\n\
             ---\n\n# P\n\n## Todos\n\n- [ ] first\n- [ ] second\n\n{PLAN_HEADER_END}\n"
    );
    let doc = parse(&text);
    assert_eq!(doc.todo_index(TodoFocus::Id("y")), Some(1));
    assert_eq!(doc.todo_index(TodoFocus::Id("z")), None);
    let nth = |index, count, content| TodoFocus::Nth {
        index,
        count,
        content,
    };
    assert_eq!(doc.todo_index(nth(0, 2, "whatever")), Some(0));
    assert_eq!(
        doc.todo_index(nth(2, 3, "second")),
        Some(1),
        "a count that disagrees falls back to the text"
    );
    assert_eq!(doc.todo_index(nth(2, 3, "gone")), None);
}

#[test]
fn a_marker_quoted_in_the_overview_of_a_plan_without_todos_is_passed_over() {
    // As `plan_tools` writes a plan whose overview quotes the marker.
    let text = format!(
            "---\nname: Tricky\noverview: \"mentions\\n{PLAN_HEADER_END}\\ninside\"\ntodos: []\n---\n\n\
             # Tricky\n\nmentions\n{PLAN_HEADER_END}\ninside\n\n{PLAN_HEADER_END}\n\nBody.\n"
        );
    let doc = parse(&text);
    assert_eq!(doc.title, "Tricky");
    assert_eq!(doc.overview, format!("mentions\n{PLAN_HEADER_END}\ninside"));
    assert_eq!(doc.body, "Body.");
}

#[test]
fn the_frontmatter_todos_win_over_a_stale_checklist() {
    let text = format!(
        "---\nname: P\ntodos:\n\
             - id: x\n  content: first\n  status: completed\n\
             - id: y\n  content: second\n  status: in_progress\n---\n\n\
             # P\n\n## Todos\n\n- [ ] second\n- [ ] first\n\n{PLAN_HEADER_END}\n"
    );
    let doc = parse(&text);
    assert_eq!(
        doc.todos,
        [
            todo(TodoState::Completed, "first", None),
            todo(TodoState::InProgress, "second", None),
        ]
    );
    assert_eq!(doc.todo_index(TodoFocus::Id("y")), Some(1));
}

#[test]
fn control_characters_never_reach_the_terminal() {
    let doc = parse("---\nname: P\n---\r\n\nBody \u{1b}[31mred\u{7}\r\n");
    assert_eq!(doc.body, "Body \u{fffd}[31mred\u{fffd}");
}

#[test]
fn plan_files_are_recognized_by_their_suffix() {
    assert!(is_plan_file(Path::new(".aura/plans/a_1.plan.md")));
    assert!(!is_plan_file(Path::new("README.md")));
}

#[test]
fn the_origin_keys_are_read_and_old_plans_have_none() {
    let doc = parse(
        "---\nname: Meta\ncreated_at: '2026-11-28T14:32:00Z'\nupdated_at: \"2026-11-28T15:00:00+01:00\"\n\
         author: ab12\nmodel: claude-opus\nmachine: aura-rus199-12b4\ntodos: []\n---\n\nBody.\n",
    );
    let at =
        |h, m| chrono::TimeZone::with_ymd_and_hms(&chrono::Utc, 2026, 11, 28, h, m, 0).unwrap();
    assert_eq!(
        doc.origin,
        PlanOrigin {
            created_at: Some(at(14, 32)),
            updated_at: Some(at(14, 0)),
            author: Some("ab12".to_string()),
            model: Some("claude-opus".to_string()),
            machine: Some("aura-rus199-12b4".to_string()),
        }
    );
    let old = parse("---\nname: Old\ntodos:\n- id: a\n  content: first\n---\n\nBody.\n");
    assert_eq!(old.origin, PlanOrigin::default());
    let bad = parse("---\nname: Bad\ncreated_at: yesterday\nauthor: ''\n---\n");
    assert_eq!(
        bad.origin,
        PlanOrigin::default(),
        "unreadable keys are left out"
    );
}
