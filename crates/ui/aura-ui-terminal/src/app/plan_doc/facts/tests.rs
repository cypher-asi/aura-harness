use super::when::ago;
use super::*;
use crate::events::{MergeQueueView, MergeStateView, UiCommand};
use crate::repo::{EventState, RepoEvent, RepoView};
use chrono::{FixedOffset, TimeZone, Utc};
use std::path::Path;

/// 16:32 on 28 Nov 2026, an hour ahead of UTC.
fn now() -> DateTime<FixedOffset> {
    FixedOffset::east_opt(3600)
        .unwrap()
        .with_ymd_and_hms(2026, 11, 28, 16, 32, 0)
        .unwrap()
}

fn plan(front: &str, todos: &str) -> PlanDoc {
    let text = format!("---\nname: Meta\n{front}todos:\n{todos}---\n\nBody.\n");
    PlanDoc::parse(&text, Path::new("/w/a.plan.md"), ".aura/plans/a.plan.md")
}

const FULL: &str = "created_at: '2026-11-28T13:32:00Z'\nupdated_at: '2026-11-28T15:02:00Z'\n\
                    author: ab12\nmodel: opus\nmachine: aura-rus199-12b4\n";

const TODOS: &str = "- id: a\n  content: first\n  status: completed\n\
                     - id: b\n  content: second\n  status: in_progress\n  assignee: REV69\n\
                     - id: c\n  content: third\n";

fn agent(id: &str, code: &str, name: &str) -> AgentSummary {
    AgentSummary {
        id: id.to_string(),
        code: Some(code.to_string()),
        name: name.to_string(),
        project: Some("aura-harness".to_string()),
        ..AgentSummary::default()
    }
}

/// An app listing the author `ab12` (RUS199) as a member of `#aura`,
/// coordinated by COO2.
fn app_in_channel() -> App {
    let mut app = App::new();
    let author = agent("ab12", "RUS199", "rust-engineer");
    app.process_command(UiCommand::SetAgents(vec![author.clone()]));
    app.process_command(UiCommand::SetChannels(vec![ChannelSummary {
        id: "c1".to_string(),
        slug: "aura".to_string(),
        coordinator_id: "co".to_string(),
        members: vec![agent("co", "COO2", "coordinator"), author],
        ..ChannelSummary::default()
    }]));
    app
}

fn queue(state: MergeStateView) -> UiCommand {
    UiCommand::SetMergeQueue {
        channel_id: "c1".to_string(),
        queue: MergeQueueView {
            entries: vec![MergeEntryView {
                branch: "aura/plan-meta".to_string(),
                agent: Some("RUS199".to_string()),
                state,
                ..MergeEntryView::default()
            }],
            ..MergeQueueView::default()
        },
    }
}

fn pairs(facts: &[Fact]) -> Vec<(&str, &str)> {
    facts
        .iter()
        .map(|fact| (fact.label, fact.value.as_str()))
        .collect()
}

#[test]
fn ages_read_in_the_largest_whole_unit() {
    for (seconds, text) in [
        (-30, "just now"),
        (0, "just now"),
        (59, "just now"),
        (60, "1m ago"),
        (3599, "59m ago"),
        (3600, "1h ago"),
        (86_399, "23h ago"),
        (86_400, "1d ago"),
        (864_000, "10d ago"),
    ] {
        assert_eq!(ago(seconds), text, "{seconds}s");
    }
}

#[test]
fn times_show_the_clock_today_and_the_date_before() {
    let at = |d, h, m| Utc.with_ymd_and_hms(2026, 11, d, h, m, 0).unwrap();
    assert_eq!(when(at(28, 13, 32), &now()), "14:32 · 2h ago");
    assert_eq!(when(at(27, 15, 32), &now()), "Nov 27 16:32 · 1d ago");
    let last_year = Utc.with_ymd_and_hms(2025, 1, 2, 9, 0, 0).unwrap();
    assert!(when(last_year, &now()).starts_with("2025-01-02 10:00 · "));
}

#[test]
fn a_channel_members_plan_says_who_asked_and_where_it_is() {
    let mut app = app_in_channel();
    app.process_command(queue(MergeStateView::Running));
    let facts = app.plan_facts(&plan(FULL, TODOS), &now());
    assert_eq!(
        pairs(&facts.who),
        [
            ("created", "14:32 · 2h ago"),
            ("updated", "16:02 · 30m ago"),
            ("by", "RUS199 rust-engineer"),
            ("asked by", "COO2 in #aura"),
            ("assigned", "REV69"),
        ]
    );
    assert_eq!(
        pairs(&facts.state),
        [
            ("status", "shipping 1/3"),
            ("machine", "aura-rus199-12b4"),
            ("branch", "aura/plan-meta"),
            ("project", "aura-harness"),
            ("model", "opus"),
        ]
    );
}

#[test]
fn the_repo_tab_gives_the_branch_and_its_pr() {
    let mut app = App::new();
    app.process_command(UiCommand::SetAgents(vec![agent(
        "ab12",
        "RUS199",
        "rust-engineer",
    )]));
    app.process_command(UiCommand::SetActiveAgent("ab12".to_string()));
    let pr = |number, state, at| RepoEvent {
        kind: EventKind::Pr,
        state,
        at,
        title: format!("#{number} Plan meta"),
        detail: "aura/plan-meta → main".to_string(),
        target: RepoTarget::Pr { number },
        url: None,
    };
    app.process_command(UiCommand::SetRepo(Box::new(RepoView {
        agent_id: "ab12".to_string(),
        branch: Some("aura/plan-meta".to_string()),
        events: vec![pr(67, EventState::Closed, 10), pr(68, EventState::Open, 20)],
        ..RepoView::default()
    })));
    let facts = app.plan_facts(&plan(FULL, TODOS), &now());
    let state = pairs(&facts.state);
    assert!(state.contains(&("branch", "aura/plan-meta")), "{state:?}");
    assert!(state.contains(&("PR", "#68 open")), "{state:?}");
    assert!(
        pairs(&facts.who).contains(&("asked by", "you")),
        "no channel"
    );
}

#[test]
fn an_unknown_author_shows_its_id_and_no_channel_facts() {
    let app = App::new();
    let facts = app.plan_facts(&plan(FULL, TODOS), &now());
    let by = facts.who.iter().find(|fact| fact.label == "by").unwrap();
    assert_eq!((by.value.as_str(), by.tone), ("ab12", FactTone::Muted));
    let labels: Vec<&str> = facts.state.iter().map(|fact| fact.label).collect();
    assert_eq!(labels, ["status", "machine", "model"]);
}

#[test]
fn an_old_plan_has_only_its_status_and_assignees() {
    let app = app_in_channel();
    let facts = app.plan_facts(&plan("", TODOS), &now());
    assert_eq!(pairs(&facts.who), [("assigned", "REV69")]);
    assert_eq!(pairs(&facts.state), [("status", "building 1/3")]);
    let bare = app.plan_facts(&plan("", "[]\n"), &now());
    assert_eq!(pairs(&bare.state), [("status", "planned")]);
}

#[test]
fn an_update_no_later_than_creation_is_not_shown() {
    let front = "created_at: '2026-11-28T13:32:00Z'\nupdated_at: '2026-11-28T13:32:00Z'\n";
    let facts = App::new().plan_facts(&plan(front, TODOS), &now());
    let labels: Vec<&str> = facts.who.iter().map(|fact| fact.label).collect();
    assert_eq!(labels, ["created", "assigned"]);
}

#[test]
fn status_follows_the_queue_then_the_todos_then_plan_mode() {
    let done = "- id: a\n  content: first\n  status: completed\n";
    let pending = "- id: a\n  content: first\n";
    let status = |app: &App, todos: &str| {
        let facts = app.plan_facts(&plan(FULL, todos), &now());
        let fact = facts.state[0].clone();
        (fact.value, fact.tone)
    };
    let mut app = app_in_channel();
    for (state, text) in [
        (MergeStateView::Queued, "shipping"),
        (MergeStateView::Merged, "merged"),
        (MergeStateView::Failed, "failed"),
    ] {
        app.process_command(queue(state));
        assert_eq!(status(&app, pending).0, format!("{text} 0/1"));
    }
    assert_eq!(status(&app, pending).1, FactTone::Error);

    let in_channel = app_in_channel();
    assert_eq!(
        status(&in_channel, done).0,
        "shipping 1/1",
        "not queued yet"
    );
    assert_eq!(status(&in_channel, pending).0, "planned 0/1");

    let mut alone = App::new();
    let mut planner = agent("ab12", "RUS199", "rust-engineer");
    planner.plan_mode = true;
    alone.process_command(UiCommand::SetAgents(vec![planner]));
    assert_eq!(status(&alone, done).0, "done 1/1");
    assert_eq!(status(&alone, pending).0, "planning 0/1");
}
