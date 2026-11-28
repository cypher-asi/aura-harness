//! The facts under a plan's title, as the PLAN view draws them.

use super::meta::push_facts;
use super::tests::{app_with, screen_at};
use super::*;
use crate::app::{Fact, FactTone};
use crate::events::{AgentSummary, UiCommand};
use crate::renderer::text::display_width;

fn fact(label: &'static str, value: &str) -> Fact {
    Fact {
        label,
        value: value.to_string(),
        tone: FactTone::Plain,
    }
}

fn full_facts() -> PlanFacts {
    PlanFacts {
        who: vec![
            fact("created", "14:32 · 2h ago"),
            fact("updated", "16:02 · 30m ago"),
            fact("by", "RUS199 rust-engineer"),
            fact("asked by", "COO2 in #aura"),
        ],
        state: vec![
            fact("status", "building 3/8"),
            fact("machine", "aura-rus199-12b4"),
            fact("branch", "aura/plan-view-metadata-rus199"),
            fact("project", "aura-harness"),
        ],
    }
}

fn rows_of(facts: &PlanFacts, width: usize) -> Vec<String> {
    let mut laid = Laid::default();
    laid.push(Line::from("Title"));
    push_facts(&mut laid, facts, width, &Theme::cyber());
    laid.lines
        .iter()
        .skip(2)
        .map(|line| line.spans.iter().map(|s| s.content.as_ref()).collect())
        .collect()
}

#[test]
fn facts_pack_onto_rows_by_group() {
    let rows = rows_of(&full_facts(), 120);
    assert_eq!(
        rows,
        [
            "created 14:32 · 2h ago  updated 16:02 · 30m ago  by RUS199 rust-engineer  asked by COO2 in #aura",
            "status building 3/8  machine aura-rus199-12b4  branch aura/plan-view-metadata-rus199  project aura-harness",
        ]
    );
}

#[test]
fn a_pair_that_does_not_fit_moves_down_whole() {
    let rows = rows_of(&full_facts(), 50);
    assert!(rows.len() > 2, "{rows:#?}");
    for row in &rows {
        assert!(display_width(row) <= 50, "{row}");
        assert!(!row.starts_with(' ') && !row.ends_with(' '), "{row:?}");
    }
    let joined = rows.join("\n");
    for whole in ["by RUS199 rust-engineer", "machine aura-rus199-12b4"] {
        assert!(joined.contains(whole), "{whole} split:\n{joined}");
    }
}

#[test]
fn no_row_is_wider_than_the_column_at_any_width() {
    let facts = full_facts();
    for width in 1..=120 {
        let rows = rows_of(&facts, width);
        for row in &rows {
            assert!(display_width(row) <= width, "{width}: {row:?}");
        }
        if width < 30 {
            assert!(
                rows.iter().any(|row| row.ends_with('…')),
                "{width}: {rows:#?}"
            );
        }
    }
}

#[test]
fn labels_are_muted_and_values_take_their_tone() {
    let theme = Theme::cyber();
    let facts = PlanFacts {
        who: vec![],
        state: vec![Fact {
            label: "status",
            value: "failed 2/3".to_string(),
            tone: FactTone::Error,
        }],
    };
    let mut laid = Laid::default();
    push_facts(&mut laid, &facts, 80, &theme);
    let spans = &laid.lines[0].spans;
    assert_eq!(spans[0].style.fg, Some(theme.colors.muted));
    assert_eq!(spans[1].style.fg, Some(theme.colors.error));
    assert_eq!(laid.prose, [None], "on the cell grid");
}

#[test]
fn no_facts_add_no_rows() {
    assert!(rows_of(&PlanFacts::default(), 80).is_empty());
}

const NEW_PLAN: &str = "---\nname: Plan view metadata\noverview: Show where plans come from.\n\
    created_at: '2026-11-28T14:32:00Z'\nauthor: ab12\nmachine: aura-rus199-12b4\nmodel: opus\n\
    todos:\n- id: a\n  content: first\n  status: completed\n- id: b\n  content: second\n---\n\nBody.\n";

const OLD_PLAN: &str = "---\nname: Plan view metadata\noverview: Show where plans come from.\n\
    todos:\n- id: a\n  content: first\n  status: completed\n- id: b\n  content: second\n---\n\nBody.\n";

fn plan_screen(text: &str, width: u16) -> Vec<String> {
    let (mut app, _dir) = app_with(text);
    app.process_command(UiCommand::SetAgents(vec![AgentSummary {
        id: "ab12".to_string(),
        code: Some("RUS199".to_string()),
        name: "rust-engineer".to_string(),
        ..AgentSummary::default()
    }]));
    app.open_plan_at(".aura/plans/a.plan.md", None);
    screen_at(&mut app, width)
        .lines()
        .map(str::to_string)
        .collect()
}

#[test]
fn the_facts_sit_between_the_title_and_the_overview() {
    let rows = plan_screen(NEW_PLAN, 160);
    let at = |needle: &str| {
        rows.iter()
            .position(|row| row.contains(needle))
            .unwrap_or_else(|| panic!("no {needle:?} in:\n{}", rows.join("\n")))
    };
    let title = (0..rows.len())
        .find(|&row| rows[row].contains("Plan view metadata") && !rows[row].contains("PLAN "))
        .unwrap();
    let created = at("created ");
    let status = at("status building 1/2");
    let overview = at("Show where plans come from.");
    assert_eq!(created, title + 2, "a blank row under the title");
    assert!(
        rows[created].contains("by RUS199 rust-engineer"),
        "{}",
        rows[created]
    );
    assert!(rows[created].contains("asked by you"), "{}", rows[created]);
    assert!(created < status && status < overview, "{}", rows.join("\n"));
    assert!(
        rows[status].contains("machine aura-rus199-12b4"),
        "{}",
        rows[status]
    );
    assert!(rows[status].contains("model opus"), "{}", rows[status]);
}

#[test]
fn an_old_plan_shows_only_its_status() {
    let screen = plan_screen(OLD_PLAN, 160).join("\n");
    assert!(screen.contains("status building 1/2"), "{screen}");
    for absent in ["created ", "by ", "machine ", "model "] {
        assert!(!screen.contains(absent), "{absent} in:\n{screen}");
    }
}

#[test]
fn a_narrow_screen_keeps_the_facts_inside_the_panel() {
    for width in [40, 60, 80] {
        let rows = plan_screen(NEW_PLAN, width);
        for row in &rows {
            assert_eq!(row.chars().count(), usize::from(width), "{width}: {row}");
        }
        let screen = rows.join("\n");
        assert!(screen.contains("status "), "{width}:\n{screen}");
        // Every fact row ends inside the panel's right border.
        for row in rows
            .iter()
            .filter(|row| row.contains("status ") || row.contains("created "))
        {
            let trimmed = row.trim_end();
            assert!(trimmed.ends_with('│'), "{width}: {row}");
        }
    }
}
